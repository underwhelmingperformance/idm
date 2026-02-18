use std::str::FromStr;
use std::time::Duration;

use async_trait::async_trait;
use bon::Builder;
use tokio::time::sleep;
use tracing::instrument;

use super::DeviceProfile;
use super::hardware::{ConnectedBleSession, WriteMode, missing_required_endpoints};
use super::model::{
    CharacteristicInfo, EndpointPresence, FoundDevice, InspectReport, ListenStopReason,
    NotificationRunSummary, ServiceInfo, SessionMetadata,
};
use super::model_overrides::{ModelResolutionConfig, is_supported_led_type};
use super::profile::{resolve_device_profile, resolve_device_routing_profile};
use super::scan_model::ScanModelHandler;
use super::session::{FA_SERVICE_UUID, FA_WRITE_UUID, negotiate_session_endpoints};
use crate::error::{FixtureError, InteractionError};
use crate::protocol::{self, EndpointId};

const DEFAULT_INITIAL_READ: [u8; 5] = [0x05, 0x00, 0x01, 0x00, 0x01];
const DEFAULT_NOTIFICATIONS: [[u8; 5]; 2] = [
    [0x05, 0x00, 0x03, 0x00, 0x01],
    [0x05, 0x00, 0x03, 0x00, 0x03],
];
const DEFAULT_WRITE_WITHOUT_RESPONSE_LIMIT: Option<usize> =
    Some(protocol::TRANSPORT_CHUNK_MTU_READY);

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
    model_resolution: ModelResolutionConfig,
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
    write_without_response_limit: Option<usize>,
    model_resolution: ModelResolutionConfig,
}

impl FakeBackend {
    /// Creates a fake backend from explicit settings.
    pub(crate) fn new(config: FakeBackendConfig) -> Self {
        let initial_read = config
            .initial_read
            .map(Into::into)
            .or_else(|| Some(DEFAULT_INITIAL_READ.to_vec()));
        let notifications = config
            .notifications
            .map_or_else(|| DEFAULT_NOTIFICATIONS.map(Vec::from).to_vec(), Into::into);

        Self {
            devices: config.scan_fixture.into(),
            services: default_services(),
            initial_read,
            notifications,
            discovery_delay: config.discovery_delay,
            write_without_response_limit: DEFAULT_WRITE_WITHOUT_RESPONSE_LIMIT,
            model_resolution: config.model_resolution,
        }
    }

    /// Connects to the first matching fake peripheral and returns a session.
    #[instrument(skip(self), level = "debug", fields(prefix = name_prefix))]
    pub(crate) async fn connect_first_matching_device(
        self,
        name_prefix: &str,
    ) -> Result<FakeDeviceSession, InteractionError> {
        let Self {
            devices,
            services,
            initial_read,
            notifications,
            discovery_delay,
            write_without_response_limit,
            model_resolution,
        } = self;

        let device = first_matching_device(devices, discovery_delay, name_prefix).await?;
        let negotiated_endpoints = negotiate_session_endpoints(&services)?;
        let endpoint_presence = negotiated_endpoints.endpoint_presence();
        let missing = missing_required_endpoints(&endpoint_presence);
        if !missing.is_empty() {
            return Err(InteractionError::MissingRequiredEndpoints {
                missing: format_missing_endpoints(&missing),
            });
        }

        let selected_led_type = select_led_type_override(&device, &model_resolution)?;
        let led_info = initial_read
            .as_deref()
            .and_then(super::LedInfoResponse::parse);
        let device_routing_profile =
            resolve_device_routing_profile(&device, led_info, selected_led_type);
        ensure_ambiguous_shape_is_resolved(&device, device_routing_profile)?;

        let device_profile = resolve_device_profile(
            &device,
            &services,
            write_without_response_limit,
            device_routing_profile,
        );
        let session_metadata =
            SessionMetadata::new(true, write_without_response_limit, device_profile)
                .with_endpoint_resolution(
                    negotiated_endpoints.gatt_profile,
                    negotiated_endpoints.endpoint_uuids.clone(),
                );

        Ok(FakeDeviceSession {
            device,
            services,
            endpoint_presence,
            session_metadata,
            initial_read,
            notifications,
        })
    }
}

/// Active fake session.
#[derive(Debug)]
pub(crate) struct FakeDeviceSession {
    device: FoundDevice,
    services: Vec<ServiceInfo>,
    endpoint_presence: EndpointPresence,
    session_metadata: SessionMetadata,
    initial_read: Option<Vec<u8>>,
    notifications: Vec<Vec<u8>>,
}

#[async_trait(?Send)]
impl ConnectedBleSession for FakeDeviceSession {
    fn device(&self) -> &FoundDevice {
        &self.device
    }

    fn inspect_report(&self) -> InspectReport {
        InspectReport::new(
            self.device.clone(),
            self.services.clone(),
            self.endpoint_presence.clone(),
            self.session_metadata.clone(),
        )
    }

    fn write_without_response_limit(&self) -> Option<usize> {
        self.session_metadata.write_without_response_limit()
    }

    fn device_profile(&self) -> DeviceProfile {
        self.session_metadata.device_profile()
    }

    #[instrument(skip(self), level = "trace", fields(?endpoint))]
    async fn read_endpoint(&self, endpoint: EndpointId) -> Result<Vec<u8>, InteractionError> {
        self.read_endpoint_optional(endpoint)
            .await?
            .ok_or(InteractionError::MissingEndpoint { endpoint })
    }

    #[instrument(skip(self), level = "trace", fields(?endpoint))]
    async fn read_endpoint_optional(
        &self,
        endpoint: EndpointId,
    ) -> Result<Option<Vec<u8>>, InteractionError> {
        if endpoint != EndpointId::ReadNotifyCharacteristic {
            return Err(InteractionError::MissingEndpoint { endpoint });
        }

        Ok(self.initial_read.clone())
    }

    #[instrument(skip(self, payload), level = "trace", fields(?endpoint, ?mode, payload_len = payload.len()))]
    async fn write_endpoint(
        &self,
        endpoint: EndpointId,
        payload: &[u8],
        mode: WriteMode,
    ) -> Result<(), InteractionError> {
        let _ = (payload, mode);
        if endpoint != EndpointId::WriteCharacteristic {
            return Err(InteractionError::MissingEndpoint { endpoint });
        }

        Ok(())
    }

    #[instrument(skip(self), level = "trace", fields(?endpoint))]
    async fn subscribe_endpoint(&self, endpoint: EndpointId) -> Result<(), InteractionError> {
        if endpoint != EndpointId::ReadNotifyCharacteristic {
            return Err(InteractionError::MissingEndpoint { endpoint });
        }

        Ok(())
    }

    #[instrument(skip(self), level = "trace", fields(?endpoint))]
    async fn unsubscribe_endpoint(&self, endpoint: EndpointId) -> Result<(), InteractionError> {
        if endpoint != EndpointId::ReadNotifyCharacteristic {
            return Err(InteractionError::MissingEndpoint { endpoint });
        }

        Ok(())
    }

    #[instrument(
        skip(self, on_notification),
        level = "trace",
        fields(?endpoint, ?max_notifications)
    )]
    async fn run_notifications(
        &self,
        endpoint: EndpointId,
        max_notifications: Option<usize>,
        on_notification: &mut dyn FnMut(usize, Vec<u8>),
    ) -> Result<NotificationRunSummary, InteractionError> {
        if endpoint != EndpointId::ReadNotifyCharacteristic {
            return Err(InteractionError::MissingEndpoint { endpoint });
        }

        if let Some(limit) = max_notifications
            && limit == 0
        {
            return Ok(NotificationRunSummary::new(
                0,
                ListenStopReason::ReachedLimit(0),
            ));
        }

        let mut received = 0usize;
        let mut stop_reason = ListenStopReason::NotificationStreamClosed;
        for payload in &self.notifications {
            received += 1;
            on_notification(received, payload.clone());

            if let Some(limit) = max_notifications
                && received >= limit
            {
                stop_reason = ListenStopReason::ReachedLimit(limit);
                break;
            }
        }

        Ok(NotificationRunSummary::new(received, stop_reason))
    }

    #[instrument(skip(self), level = "debug")]
    async fn close(self: Box<Self>) -> Result<(), InteractionError> {
        let _ = self;
        Ok(())
    }
}

fn select_led_type_override(
    device: &FoundDevice,
    model_resolution: &ModelResolutionConfig,
) -> Result<Option<u8>, InteractionError> {
    let Some(identity) = device.scan_identity() else {
        return Ok(None);
    };

    if let Some(override_led_type) = model_resolution.led_type_override() {
        if !is_supported_led_type(override_led_type) {
            return Err(InteractionError::InvalidLedTypeOverride {
                value: override_led_type,
            });
        }
        return Ok(Some(override_led_type));
    }

    if super::DeviceProfileResolver::requires_led_type_selection(identity) {
        return Ok(None);
    }

    Ok(None)
}

fn ensure_ambiguous_shape_is_resolved(
    device: &FoundDevice,
    routing_profile: Option<super::DeviceRoutingProfile>,
) -> Result<(), InteractionError> {
    let Some(identity) = device.scan_identity() else {
        return Ok(());
    };

    if !super::DeviceProfileResolver::requires_led_type_selection(identity) {
        return Ok(());
    }
    if routing_profile
        .and_then(|profile| profile.led_type)
        .is_some()
    {
        return Ok(());
    }

    Err(InteractionError::AmbiguousShapeSelectionRequired {
        device_id: device.device_id_display().to_string(),
        shape: identity.shape,
    })
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

#[instrument(skip(devices), level = "trace", fields(prefix = name_prefix))]
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
    if fields.len() != 4 && fields.len() != 5 {
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

    let device = FoundDevice::new(
        fields[0].to_string(),
        fields[1].to_string(),
        local_name,
        rssi,
    );

    let scan_model = match fields.get(4).copied().filter(|value| *value != "-") {
        Some(value) => {
            let scan_payload = parse_hex(value)?;
            let scan_identity = ScanModelHandler::parse_identity(&scan_payload)
                .ok_or(FixtureError::InvalidScanModelPayload)?;
            let model_profile = ScanModelHandler::resolve_model(&scan_identity);
            Some((scan_identity, model_profile))
        }
        None => None,
    };

    Ok(match scan_model {
        Some((scan_identity, model_profile)) => {
            device.with_scan_model(scan_identity, model_profile)
        }
        None => device,
    })
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
    vec![ServiceInfo::new(
        FA_SERVICE_UUID.to_string(),
        true,
        vec![
            CharacteristicInfo::new(FA_WRITE_UUID.to_string(), vec!["write".to_string()]),
            CharacteristicInfo::new(
                "0000fa03-0000-1000-8000-00805f9b34fb".to_string(),
                vec!["read".to_string(), "notify".to_string()],
            ),
        ],
    )]
}

fn format_missing_endpoints(endpoints: &[EndpointId]) -> String {
    endpoints
        .iter()
        .map(|endpoint| {
            let metadata = crate::protocol::endpoint_metadata(*endpoint);
            format!("{} ({})", metadata.name(), metadata.uuid())
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case("hci0|AA:BB|IDM-Cube|-43", 1)]
    #[case(
        "hci0|AA:BB|IDM-Cube|-43|0FFF5452007004010200010520002000;hci1|CC:DD|Speaker|-55",
        2
    )]
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

    #[test]
    fn parse_scan_fixture_rejects_invalid_scan_model_payload() {
        let result = parse_scan_fixture("hci0|AA:BB|IDM-Cube|-43|DEADBEEF");
        assert_matches!(result, Err(FixtureError::InvalidScanModelPayload));
    }
}
