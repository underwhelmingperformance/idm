use std::time::Duration;

use btleplug::api::{
    Central, CharPropFlags, Characteristic, Manager as _, Peripheral as _, ScanFilter,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
use tokio::time::sleep;
use tokio_stream::StreamExt;
use tracing::{debug, info, instrument};

use super::model::{
    CharacteristicInfo, EndpointPresence, FoundDevice, InspectReport, ListenStopReason,
    ListenSummary, ServiceInfo,
};
use crate::error::InteractionError;
use crate::protocol::{self, EndpointId};

/// Hardware backend backed by `btleplug`.
#[derive(Debug)]
pub(crate) struct BtleplugBackend {
    manager: Manager,
}

impl BtleplugBackend {
    /// Creates the real BLE backend.
    pub(crate) async fn new() -> Result<Self, InteractionError> {
        let manager = Manager::new().await?;
        Ok(Self { manager })
    }

    /// Scans indefinitely until the first matching peripheral appears, then connects.
    #[instrument(skip(self), fields(prefix = name_prefix))]
    async fn find_and_connect_first_matching(
        &self,
        name_prefix: &str,
    ) -> Result<ConnectedPeripheral, InteractionError> {
        let adapters = self.adapters().await?;
        info!(
            adapter_count = adapters.len(),
            "starting indefinite BLE scan"
        );

        for adapter in &adapters {
            adapter.adapter.start_scan(ScanFilter::default()).await?;
        }

        loop {
            for adapter in &adapters {
                let peripherals = adapter.adapter.peripherals().await?;
                for peripheral in peripherals {
                    let Some(properties) = peripheral.properties().await? else {
                        continue;
                    };
                    let rssi = properties.rssi;
                    let Some(local_name) = properties.local_name else {
                        continue;
                    };
                    if !local_name.starts_with(name_prefix) {
                        continue;
                    }

                    for handle in &adapters {
                        if let Err(error) = handle.adapter.stop_scan().await {
                            debug!(?error, "failed to stop adapter scan cleanly");
                        }
                    }

                    if !peripheral.is_connected().await? {
                        peripheral.connect().await?;
                    }
                    peripheral.discover_services().await?;

                    let device = FoundDevice::new(
                        adapter.name.clone(),
                        format!("{:?}", peripheral.id()),
                        Some(local_name),
                        rssi,
                    );
                    info!(device_id = %device.device_id(), "connected to matching peripheral");
                    return Ok(ConnectedPeripheral { peripheral, device });
                }
            }

            sleep(Duration::from_millis(250)).await;
        }
    }

    async fn adapters(&self) -> Result<Vec<AdapterHandle>, InteractionError> {
        let adapters = self.manager.adapters().await?;
        if adapters.is_empty() {
            return Err(InteractionError::NoAdapters);
        }

        let mut handles = Vec::with_capacity(adapters.len());
        for adapter in adapters {
            let name = adapter.adapter_info().await?;
            handles.push(AdapterHandle { adapter, name });
        }
        Ok(handles)
    }

    /// Connects to the first matching peripheral and returns a full service inspection report.
    pub(crate) async fn inspect_first_matching_device(
        &self,
        name_prefix: &str,
    ) -> Result<InspectReport, InteractionError> {
        let connected = self.find_and_connect_first_matching(name_prefix).await?;
        let (services, endpoint_presence) = collect_services_and_presence(&connected.peripheral);
        let report = InspectReport::new(connected.device, services, endpoint_presence);

        if let Err(error) = connected.peripheral.disconnect().await {
            debug!(?error, "failed to disconnect after inspection");
        }
        Ok(report)
    }

    /// Connects to the first matching device and prepares a notification session.
    pub(crate) async fn prepare_listen_first_matching_device(
        &self,
        name_prefix: &str,
    ) -> Result<PreparedRealListen, InteractionError> {
        let connected = self.find_and_connect_first_matching(name_prefix).await?;
        let read_endpoint = EndpointId::ReadNotifyCharacteristic;
        let read_characteristic = find_characteristic(&connected.peripheral, read_endpoint).ok_or(
            InteractionError::MissingEndpoint {
                endpoint: read_endpoint,
            },
        )?;

        let initial_read = connected.peripheral.read(&read_characteristic).await?;
        connected.peripheral.subscribe(&read_characteristic).await?;

        Ok(PreparedRealListen {
            device: connected.device,
            initial_read: Some(initial_read),
            peripheral: connected.peripheral,
            read_characteristic,
        })
    }
}

/// A prepared listen session on a connected real peripheral.
#[derive(Debug)]
pub(crate) struct PreparedRealListen {
    device: FoundDevice,
    initial_read: Option<Vec<u8>>,
    peripheral: Peripheral,
    read_characteristic: Characteristic,
}

impl PreparedRealListen {
    /// Returns connected device details.
    pub(crate) fn device(&self) -> &FoundDevice {
        &self.device
    }

    /// Returns the initial read payload from `fa03`, if any.
    pub(crate) fn initial_read(&self) -> Option<&[u8]> {
        self.initial_read.as_deref()
    }

    /// Runs notification listening until interrupted, stream close, or limit reached.
    pub(crate) async fn run<F>(
        self,
        max_notifications: Option<usize>,
        mut on_notification: F,
    ) -> Result<ListenSummary, InteractionError>
    where
        F: FnMut(usize, &[u8]),
    {
        let mut notifications = self.peripheral.notifications().await?;
        let mut received = 0usize;
        let stop_reason = loop {
            tokio::select! {
                signal = tokio::signal::ctrl_c() => {
                    signal?;
                    break ListenStopReason::Interrupted;
                }
                maybe_notification = notifications.next() => {
                    match maybe_notification {
                        Some(notification) => {
                            let notification_uuid = notification.uuid.to_string();
                            if !endpoint_matches_uuid(EndpointId::ReadNotifyCharacteristic, &notification_uuid) {
                                continue;
                            }
                            received += 1;
                            on_notification(received, &notification.value);
                            if let Some(limit) = max_notifications && received >= limit {
                                break ListenStopReason::ReachedLimit(limit);
                            }
                        }
                        None => {
                            break ListenStopReason::NotificationStreamClosed;
                        }
                    }
                }
            }
        };

        if let Err(error) = self.peripheral.unsubscribe(&self.read_characteristic).await {
            debug!(?error, "failed to unsubscribe cleanly");
        }
        if let Err(error) = self.peripheral.disconnect().await {
            debug!(?error, "failed to disconnect cleanly");
        }

        Ok(ListenSummary::new(
            self.device,
            self.initial_read,
            received,
            stop_reason,
        ))
    }
}

#[derive(Debug)]
struct AdapterHandle {
    adapter: Adapter,
    name: String,
}

#[derive(Debug)]
struct ConnectedPeripheral {
    peripheral: Peripheral,
    device: FoundDevice,
}

fn find_characteristic(peripheral: &Peripheral, endpoint: EndpointId) -> Option<Characteristic> {
    peripheral
        .services()
        .iter()
        .flat_map(|service| service.characteristics.iter())
        .find(|characteristic| endpoint_matches_uuid(endpoint, &characteristic.uuid.to_string()))
        .cloned()
}

fn collect_services_and_presence(peripheral: &Peripheral) -> (Vec<ServiceInfo>, EndpointPresence) {
    let mut services = Vec::new();
    let mut presence_by_endpoint = protocol::empty_presence_map();

    for service in peripheral.services() {
        let service_uuid = service.uuid.to_string().to_lowercase();
        if let Some(endpoint) = protocol::endpoint_for_uuid(&service_uuid) {
            presence_by_endpoint.insert(endpoint, true);
        }

        let mut characteristics = Vec::new();
        for characteristic in &service.characteristics {
            let characteristic_uuid = characteristic.uuid.to_string().to_lowercase();
            if let Some(endpoint) = protocol::endpoint_for_uuid(&characteristic_uuid) {
                presence_by_endpoint.insert(endpoint, true);
            }

            characteristics.push(CharacteristicInfo::new(
                characteristic_uuid,
                property_labels(characteristic.properties),
            ));
        }
        characteristics.sort_by(|left, right| left.uuid().cmp(right.uuid()));

        services.push(ServiceInfo::new(
            service_uuid,
            service.primary,
            characteristics,
        ));
    }
    services.sort_by(|left, right| left.uuid().cmp(right.uuid()));

    (services, EndpointPresence::new(presence_by_endpoint))
}

fn property_labels(flags: CharPropFlags) -> Vec<String> {
    let labels: Vec<String> = flags
        .iter_names()
        .map(|(name, _)| name.to_lowercase())
        .collect();
    if labels.is_empty() {
        vec!["none".to_string()]
    } else {
        labels
    }
}

fn endpoint_matches_uuid(endpoint: EndpointId, value: &str) -> bool {
    let expected = protocol::endpoint_metadata(endpoint).uuid();
    value.eq_ignore_ascii_case(expected)
}
