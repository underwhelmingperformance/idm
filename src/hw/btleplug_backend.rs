use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use btleplug::api::{
    Central, CharPropFlags, Characteristic, Manager as _, Peripheral as _, ScanFilter, WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
use tokio::time::sleep;
use tokio_stream::StreamExt;
use tracing::{debug, info, instrument};

use super::DeviceProfile;
use super::hardware::{ConnectedBleSession, WriteMode, missing_required_endpoints};
use super::model::{
    CharacteristicInfo, EndpointPresence, FoundDevice, InspectReport, ListenStopReason,
    NotificationRunSummary, ServiceInfo, SessionMetadata,
};
use super::profile::resolve_device_profile;
use super::session::negotiate_session_endpoints;
use crate::error::InteractionError;
use crate::protocol::EndpointId;

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

    /// Connects to the first matching peripheral and prepares a session object.
    pub(crate) async fn connect_first_matching_device(
        self,
        name_prefix: &str,
    ) -> Result<RealDeviceSession, InteractionError> {
        let connected = self.find_and_connect_first_matching(name_prefix).await?;
        let (services, characteristics_by_uuid) =
            collect_services_and_characteristics(&connected.peripheral);
        let negotiated_endpoints = negotiate_session_endpoints(&services)?;
        let endpoint_presence = negotiated_endpoints.endpoint_presence();
        let characteristics_by_endpoint = characteristics_by_endpoint(
            &negotiated_endpoints.endpoint_uuids,
            &characteristics_by_uuid,
        )?;

        let missing = missing_required_endpoints(&endpoint_presence);
        if !missing.is_empty() {
            if let Err(error) = connected.peripheral.disconnect().await {
                debug!(
                    ?error,
                    "failed to disconnect after endpoint validation error"
                );
            }

            return Err(InteractionError::MissingRequiredEndpoints {
                missing: format_missing_endpoints(&missing),
            });
        }

        let write_without_response_limit = None;
        let device_profile =
            resolve_device_profile(&connected.device, &services, write_without_response_limit);
        let session_metadata =
            SessionMetadata::new(true, write_without_response_limit, device_profile)
                .with_endpoint_resolution(
                    negotiated_endpoints.gatt_profile,
                    negotiated_endpoints.endpoint_uuids.clone(),
                );

        Ok(RealDeviceSession {
            device: connected.device,
            services,
            endpoint_presence,
            session_metadata,
            characteristics_by_endpoint,
            peripheral: connected.peripheral,
        })
    }
}

/// Active session bound to a real peripheral.
#[derive(Debug)]
pub(crate) struct RealDeviceSession {
    device: FoundDevice,
    services: Vec<ServiceInfo>,
    endpoint_presence: EndpointPresence,
    session_metadata: SessionMetadata,
    characteristics_by_endpoint: HashMap<EndpointId, Characteristic>,
    peripheral: Peripheral,
}

impl RealDeviceSession {
    fn characteristic_for(
        &self,
        endpoint: EndpointId,
    ) -> Result<&Characteristic, InteractionError> {
        self.characteristics_by_endpoint
            .get(&endpoint)
            .ok_or(InteractionError::MissingEndpoint { endpoint })
    }
}

#[async_trait(?Send)]
impl ConnectedBleSession for RealDeviceSession {
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

    async fn read_endpoint(&self, endpoint: EndpointId) -> Result<Vec<u8>, InteractionError> {
        let characteristic = self.characteristic_for(endpoint)?;
        let payload = self.peripheral.read(characteristic).await?;
        Ok(payload)
    }

    async fn read_endpoint_optional(
        &self,
        endpoint: EndpointId,
    ) -> Result<Option<Vec<u8>>, InteractionError> {
        Ok(Some(self.read_endpoint(endpoint).await?))
    }

    async fn write_endpoint(
        &self,
        endpoint: EndpointId,
        payload: &[u8],
        mode: WriteMode,
    ) -> Result<(), InteractionError> {
        let characteristic = self.characteristic_for(endpoint)?;
        let write_type = match mode {
            WriteMode::WithResponse => WriteType::WithResponse,
            WriteMode::WithoutResponse => WriteType::WithoutResponse,
        };
        self.peripheral
            .write(characteristic, payload, write_type)
            .await?;
        Ok(())
    }

    async fn subscribe_endpoint(&self, endpoint: EndpointId) -> Result<(), InteractionError> {
        let characteristic = self.characteristic_for(endpoint)?;
        self.peripheral.subscribe(characteristic).await?;
        Ok(())
    }

    async fn unsubscribe_endpoint(&self, endpoint: EndpointId) -> Result<(), InteractionError> {
        let characteristic = self.characteristic_for(endpoint)?;
        self.peripheral.unsubscribe(characteristic).await?;
        Ok(())
    }

    async fn run_notifications(
        &self,
        endpoint: EndpointId,
        max_notifications: Option<usize>,
        on_notification: &mut dyn FnMut(usize, Vec<u8>),
    ) -> Result<NotificationRunSummary, InteractionError> {
        let expected_characteristic = self.characteristic_for(endpoint)?;
        let expected_uuid = expected_characteristic.uuid.to_string();
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
                            if !notification_uuid.eq_ignore_ascii_case(&expected_uuid) {
                                continue;
                            }

                            received += 1;
                            on_notification(received, notification.value);
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

        Ok(NotificationRunSummary::new(received, stop_reason))
    }

    async fn close(self: Box<Self>) -> Result<(), InteractionError> {
        if self.peripheral.is_connected().await? {
            self.peripheral.disconnect().await?;
        }
        Ok(())
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

fn collect_services_and_characteristics(
    peripheral: &Peripheral,
) -> (Vec<ServiceInfo>, HashMap<String, Characteristic>) {
    let mut services = Vec::new();
    let mut characteristics_by_uuid = HashMap::new();

    for service in peripheral.services() {
        let service_uuid = service.uuid.to_string().to_lowercase();

        let mut characteristics = Vec::new();
        for characteristic in &service.characteristics {
            let characteristic_uuid = characteristic.uuid.to_string().to_lowercase();
            characteristics_by_uuid
                .entry(characteristic_uuid.clone())
                .or_insert_with(|| characteristic.clone());

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

    (services, characteristics_by_uuid)
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

fn characteristics_by_endpoint(
    endpoint_uuids: &HashMap<EndpointId, String>,
    characteristics_by_uuid: &HashMap<String, Characteristic>,
) -> Result<HashMap<EndpointId, Characteristic>, InteractionError> {
    endpoint_uuids
        .iter()
        .filter_map(|(endpoint, uuid)| {
            if matches!(endpoint, EndpointId::ControlService) {
                return None;
            }

            Some(
                characteristics_by_uuid
                    .get(uuid)
                    .cloned()
                    .ok_or(InteractionError::MissingEndpoint {
                        endpoint: *endpoint,
                    })
                    .map(|characteristic| (*endpoint, characteristic)),
            )
        })
        .collect()
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
