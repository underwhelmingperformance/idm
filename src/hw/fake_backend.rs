use std::str::FromStr;
use std::time::Duration;

use bon::Builder;
use tokio::time::sleep;

use super::model::{
    CharacteristicInfo, EndpointPresence, FoundDevice, InspectReport, ListenStopReason,
    ListenSummary, ServiceInfo,
};
use crate::error::{FixtureError, InteractionError};
use crate::protocol::{self, EndpointId};

const DEFAULT_INITIAL_READ: [u8; 5] = [0x05, 0x00, 0x01, 0x00, 0x01];
const DEFAULT_NOTIFICATIONS: [[u8; 5]; 2] = [
    [0x05, 0x00, 0x01, 0x00, 0x01],
    [0x05, 0x00, 0x01, 0x00, 0x03],
];

/// Parsed fake scan fixture records.
#[derive(Debug, Clone, derive_more::Into)]
pub(crate) struct ScanFixture {
    devices: Vec<FoundDevice>,
}

impl FromStr for ScanFixture {
    type Err = FixtureError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let devices = parse_scan_fixture(value)?;
        Ok(Self { devices })
    }
}

/// Parsed fake hex payload.
#[derive(Debug, Clone, derive_more::Into)]
pub(crate) struct HexPayload {
    payload: Vec<u8>,
}

impl FromStr for HexPayload {
    type Err = FixtureError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let payload = parse_hex(value)?;
        Ok(Self { payload })
    }
}

/// Parsed fake notification payload fixtures.
#[derive(Debug, Clone, derive_more::Into)]
pub(crate) struct NotificationPayloads {
    payloads: Vec<Vec<u8>>,
}

impl FromStr for NotificationPayloads {
    type Err = FixtureError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let payloads = parse_notifications(value)?;
        Ok(Self { payloads })
    }
}

/// Settings for constructing a fake hardware backend.
#[derive(Debug, Builder)]
pub(crate) struct FakeBackendConfig {
    scan_fixture: ScanFixture,
    initial_read: Option<HexPayload>,
    notifications: Option<NotificationPayloads>,
    #[builder(default)]
    discovery_delay: Duration,
}

/// Fake backend used in tests and non-hardware environments.
#[derive(Debug)]
pub(crate) struct FakeBackend {
    devices: Vec<FoundDevice>,
    services: Vec<ServiceInfo>,
    initial_read: Option<Vec<u8>>,
    notifications: Vec<Vec<u8>>,
    discovery_delay: Duration,
}

impl FakeBackend {
    /// Creates a fake backend from explicit settings.
    pub(crate) fn new(config: FakeBackendConfig) -> Self {
        let initial_read = config
            .initial_read
            .map(Into::into)
            .or_else(|| Some(DEFAULT_INITIAL_READ.to_vec()));
        let notifications = config.notifications.map_or_else(
            || DEFAULT_NOTIFICATIONS.map(Vec::from).to_vec(),
            Into::into,
        );

        Self {
            devices: config.scan_fixture.into(),
            services: default_services(),
            initial_read,
            notifications,
            discovery_delay: config.discovery_delay,
        }
    }

    /// Returns an inspect report for the first matching fake peripheral.
    pub(crate) async fn inspect_first_matching_device(
        self,
        name_prefix: &str,
    ) -> Result<InspectReport, InteractionError> {
        let Self {
            devices,
            services,
            initial_read: _,
            notifications: _,
            discovery_delay,
        } = self;
        let device = first_matching_device(devices, discovery_delay, name_prefix).await?;
        let endpoint_presence = endpoint_presence(&services);
        Ok(InspectReport::new(device, services, endpoint_presence))
    }

    /// Prepares a fake listen session for the first matching device.
    pub(crate) async fn prepare_listen_first_matching_device(
        self,
        name_prefix: &str,
    ) -> Result<PreparedFakeListen, InteractionError> {
        let Self {
            devices,
            services: _,
            initial_read,
            notifications,
            discovery_delay,
        } = self;
        let device = first_matching_device(devices, discovery_delay, name_prefix).await?;
        Ok(PreparedFakeListen {
            device,
            initial_read,
            notifications,
        })
    }
}

/// A prepared fake listen session.
#[derive(Debug)]
pub(crate) struct PreparedFakeListen {
    device: FoundDevice,
    initial_read: Option<Vec<u8>>,
    notifications: Vec<Vec<u8>>,
}

impl PreparedFakeListen {
    /// Returns connected device details.
    pub(crate) fn device(&self) -> &FoundDevice {
        &self.device
    }

    /// Returns the initial read payload from `fa03`, if any.
    pub(crate) fn initial_read(&self) -> Option<&[u8]> {
        self.initial_read.as_deref()
    }

    /// Emits fixture notifications and returns a session summary.
    pub(crate) fn run<F>(
        self,
        max_notifications: Option<usize>,
        mut on_notification: F,
    ) -> ListenSummary
    where
        F: FnMut(usize, &[u8]),
    {
        if let Some(limit) = max_notifications
            && limit == 0
        {
            return ListenSummary::new(
                self.device,
                self.initial_read,
                0,
                ListenStopReason::ReachedLimit(0),
            );
        }

        let mut received = 0usize;
        let mut stop_reason = ListenStopReason::NotificationStreamClosed;
        for payload in self.notifications {
            received += 1;
            on_notification(received, &payload);

            if let Some(limit) = max_notifications
                && received >= limit
            {
                stop_reason = ListenStopReason::ReachedLimit(limit);
                break;
            }
        }

        ListenSummary::new(self.device, self.initial_read, received, stop_reason)
    }
}

fn parse_scan_fixture(raw_fixture: &str) -> Result<Vec<FoundDevice>, FixtureError> {
    if raw_fixture.trim().is_empty() {
        return Err(FixtureError::EmptyFixture);
    }

    raw_fixture
        .split(';')
        .map(parse_scan_record)
        .collect::<Result<Vec<_>, _>>()
}

async fn first_matching_device(
    devices: Vec<FoundDevice>,
    discovery_delay: Duration,
    name_prefix: &str,
) -> Result<FoundDevice, InteractionError> {
    if !discovery_delay.is_zero() {
        sleep(discovery_delay).await;
    }

    devices
        .into_iter()
        .find(|device| device.local_name_starts_with(name_prefix))
        .ok_or_else(|| InteractionError::NoMatchingFixtureDevice {
            prefix: name_prefix.to_string(),
        })
}

fn parse_scan_record(raw_record: &str) -> Result<FoundDevice, FixtureError> {
    let fields: Vec<&str> = raw_record.split('|').map(str::trim).collect();
    if fields.len() != 4 {
        return Err(FixtureError::InvalidRecordFieldCount);
    }
    if fields[0].is_empty() || fields[1].is_empty() || fields[2].is_empty() || fields[3].is_empty()
    {
        return Err(FixtureError::EmptyRecordField);
    }

    let local_name = if fields[2] == "-" {
        None
    } else {
        Some(fields[2].to_string())
    };
    let rssi = if fields[3] == "-" {
        None
    } else {
        Some(fields[3].parse::<i16>()?)
    };

    Ok(FoundDevice::new(
        fields[0].to_string(),
        fields[1].to_string(),
        local_name,
        rssi,
    ))
}

fn parse_notifications(raw_value: &str) -> Result<Vec<Vec<u8>>, FixtureError> {
    if raw_value.trim().is_empty() {
        return Ok(Vec::new());
    }
    raw_value.split(',').map(parse_hex).collect()
}

fn parse_hex(raw_value: &str) -> Result<Vec<u8>, FixtureError> {
    let cleaned: String = raw_value.chars().filter(|c| !c.is_whitespace()).collect();
    if !cleaned.len().is_multiple_of(2) {
        return Err(FixtureError::InvalidHexLength);
    }
    let mut payload = Vec::with_capacity(cleaned.len() / 2);
    let bytes = cleaned.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        let value = std::str::from_utf8(&bytes[index..index + 2]).map_err(|_| {
            FixtureError::InvalidHexByte {
                value: String::from_utf8_lossy(&bytes[index..index + 2]).to_string(),
            }
        })?;
        let parsed = u8::from_str_radix(value, 16).map_err(|_| FixtureError::InvalidHexByte {
            value: value.to_string(),
        })?;
        payload.push(parsed);
        index += 2;
    }
    Ok(payload)
}

fn default_services() -> Vec<ServiceInfo> {
    let service = protocol::endpoint_metadata(EndpointId::ControlService);
    let write_characteristic = protocol::endpoint_metadata(EndpointId::WriteCharacteristic);
    let read_notify_characteristic =
        protocol::endpoint_metadata(EndpointId::ReadNotifyCharacteristic);

    vec![ServiceInfo::new(
        service.uuid().to_string(),
        true,
        vec![
            CharacteristicInfo::new(
                write_characteristic.uuid().to_string(),
                vec!["write".to_string()],
            ),
            CharacteristicInfo::new(
                read_notify_characteristic.uuid().to_string(),
                vec!["read".to_string(), "notify".to_string()],
            ),
        ],
    )]
}

fn endpoint_presence(services: &[ServiceInfo]) -> EndpointPresence {
    let mut presence_by_endpoint = protocol::empty_presence_map();

    for service in services {
        if let Some(endpoint) = protocol::endpoint_for_uuid(service.uuid()) {
            presence_by_endpoint.insert(endpoint, true);
        }
        for characteristic in service.characteristics() {
            if let Some(endpoint) = protocol::endpoint_for_uuid(characteristic.uuid()) {
                presence_by_endpoint.insert(endpoint, true);
            }
        }
    }

    EndpointPresence::new(presence_by_endpoint)
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case("hci0|AA:BB|IDM-Cube|-43", 1)]
    #[case("hci0|AA:BB|IDM-Cube|-43;hci1|CC:DD|Speaker|-55", 2)]
    fn parse_scan_fixture_parses_records(#[case] fixture: &str, #[case] expected_count: usize) {
        let devices = parse_scan_fixture(fixture).expect("fixture should parse");
        assert_eq!(expected_count, devices.len());
    }

    #[test]
    fn parse_scan_fixture_rejects_invalid_field_count() {
        let result = parse_scan_fixture("hci0|AA:BB|IDM-Cube");
        assert_matches!(result, Err(FixtureError::InvalidRecordFieldCount));
    }

    #[test]
    fn parse_hex_rejects_odd_length() {
        let result = parse_hex("A");
        assert_matches!(result, Err(FixtureError::InvalidHexLength));
    }
}
