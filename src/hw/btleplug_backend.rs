use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use btleplug::api::{
    Central, CharPropFlags, Characteristic, Manager as _, Peripheral as _, PeripheralProperties,
    ScanFilter, WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
use time::OffsetDateTime;
use tokio::time::{sleep, timeout};
use tokio_stream::StreamExt;
use tracing::{debug, info, instrument, trace};

use super::DeviceProfile;
use super::hardware::{ConnectedBleSession, WriteMode, missing_required_endpoints};
use super::model::{
    CharacteristicInfo, EndpointPresence, FoundDevice, InspectReport, LedInfoQueryOutcome,
    ListenStopReason, NotificationRunSummary, ServiceInfo, SessionMetadata,
};
use super::model_overrides::{ModelOverrideStore, ModelResolutionConfig, is_supported_led_type};
use super::model_resolution_diagnostics::{
    ManufacturerDataRecord, ScanPropertiesDebug, ServiceDataRecord, model_resolution_diagnostics,
};
use super::profile::{resolve_device_profile, resolve_device_routing_profile};
use super::scan_model::{ScanIdentity, ScanModelHandler};
use super::session::negotiate_session_endpoints;
use crate::error::InteractionError;
use crate::protocol::{self, EndpointId};

const LED_INFO_QUERY_TIMEOUT_MS: u64 = 1_000;
const GET_LED_INFO_QUERY: [u8; 4] = [0x04, 0x00, 0x01, 0x80];

#[derive(Debug)]
struct LedInfoQueryResult {
    led_info: Option<super::LedInfoResponse>,
    outcome: LedInfoQueryOutcome,
    write_modes_attempted: Vec<String>,
    sync_time_fallback_attempted: bool,
    last_payload: Option<Vec<u8>>,
}

impl LedInfoQueryResult {
    fn skipped(outcome: LedInfoQueryOutcome) -> Self {
        Self {
            led_info: None,
            outcome,
            write_modes_attempted: Vec::new(),
            sync_time_fallback_attempted: false,
            last_payload: None,
        }
    }

    fn resolved(
        led_info: super::LedInfoResponse,
        outcome: LedInfoQueryOutcome,
        write_modes_attempted: Vec<String>,
        payload: Vec<u8>,
    ) -> Self {
        Self {
            led_info: Some(led_info),
            outcome,
            write_modes_attempted,
            sync_time_fallback_attempted: false,
            last_payload: Some(payload),
        }
    }

    fn unresolved(
        outcome: LedInfoQueryOutcome,
        write_modes_attempted: Vec<String>,
        last_payload: Option<Vec<u8>>,
    ) -> Self {
        Self {
            led_info: None,
            outcome,
            write_modes_attempted,
            sync_time_fallback_attempted: false,
            last_payload,
        }
    }

    fn mark_sync_time_fallback_attempted(mut self) -> Self {
        self.sync_time_fallback_attempted = true;
        self
    }
}

#[derive(Debug)]
enum LedInfoProbeResult {
    Parsed {
        response: super::LedInfoResponse,
        payload: Vec<u8>,
    },
    InvalidPayload(Vec<u8>),
    NoResponse,
}

/// Hardware backend backed by `btleplug`.
#[derive(Debug)]
pub(crate) struct BtleplugBackend {
    manager: Manager,
    model_resolution: ModelResolutionConfig,
}

impl BtleplugBackend {
    /// Creates the real BLE backend.
    pub(crate) async fn new(
        model_resolution: ModelResolutionConfig,
    ) -> Result<Self, InteractionError> {
        let manager = Manager::new().await?;
        Ok(Self {
            manager,
            model_resolution,
        })
    }

    /// Scans indefinitely until the first matching peripheral appears, then connects.
    #[instrument(skip(self), level = "debug", fields(prefix = name_prefix))]
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
                    let scan_identity = scan_identity_from_properties(&properties);
                    let scan_properties_debug = scan_properties_debug_from_properties(&properties);

                    let rssi = properties.rssi;
                    let local_name = properties.local_name;
                    if !matches_name_prefix(local_name.as_deref(), name_prefix) {
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
                        peripheral.id().to_string(),
                        local_name,
                        rssi,
                    );
                    let device = match scan_identity {
                        Some(scan_identity) => {
                            let model_profile = ScanModelHandler::resolve_model(&scan_identity);
                            device.with_scan_model(scan_identity, model_profile)
                        }
                        None => device,
                    };
                    info!(
                        device_id = %device.device_id_display(),
                        "connected to matching peripheral"
                    );
                    return Ok(ConnectedPeripheral {
                        peripheral,
                        device,
                        scan_properties_debug,
                    });
                }
            }

            sleep(Duration::from_millis(250)).await;
        }
    }

    #[instrument(skip(self), level = "trace")]
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
    #[instrument(skip(self), level = "debug", fields(prefix = name_prefix))]
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

        let selected_led_type =
            select_led_type_override(&connected.device, &self.model_resolution)?;
        let led_info_query =
            query_led_info(&connected.peripheral, &characteristics_by_endpoint).await;
        let led_info = led_info_query.led_info;
        let device_routing_profile =
            resolve_device_routing_profile(&connected.device, led_info, selected_led_type);
        ensure_ambiguous_shape_is_resolved(&connected.device, device_routing_profile)?;
        maybe_apply_joint_mode(
            &connected.peripheral,
            &characteristics_by_endpoint,
            device_routing_profile,
        )
        .await?;
        persist_resolved_led_type(
            &connected.device,
            device_routing_profile,
            &self.model_resolution,
        )?;

        let write_without_response_limit = characteristics_by_endpoint
            .get(&EndpointId::WriteCharacteristic)
            .and_then(|characteristic| negotiated_transport_write_limit(characteristic.properties));
        let device_profile = resolve_device_profile(
            &connected.device,
            &services,
            write_without_response_limit,
            device_routing_profile,
        );
        let connection_diagnostics = model_resolution_diagnostics(
            connected.device.scan_identity().copied(),
            Some(&connected.scan_properties_debug),
            led_info_query.outcome,
            led_info_query.write_modes_attempted,
            led_info_query.sync_time_fallback_attempted,
            led_info_query.last_payload,
        );
        let session_metadata =
            SessionMetadata::new(true, write_without_response_limit, device_profile)
                .with_device_routing_profile(device_routing_profile)
                .with_connection_diagnostics(connection_diagnostics)
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

    if !super::DeviceProfileResolver::requires_led_type_selection(identity) {
        return Ok(None);
    }

    let store = ModelOverrideStore::load(model_resolution)?;
    Ok(store.led_type_for(device, identity))
}

fn persist_resolved_led_type(
    device: &FoundDevice,
    routing_profile: Option<super::DeviceRoutingProfile>,
    model_resolution: &ModelResolutionConfig,
) -> Result<(), InteractionError> {
    let Some(identity) = device.scan_identity() else {
        return Ok(());
    };
    if !super::DeviceProfileResolver::requires_led_type_selection(identity) {
        return Ok(());
    }

    let Some(led_type) = routing_profile.and_then(|profile| profile.led_type) else {
        return Ok(());
    };

    let mut store = ModelOverrideStore::load(model_resolution)?;
    store.persist_led_type_for(device, identity, led_type)
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

async fn maybe_apply_joint_mode(
    peripheral: &Peripheral,
    characteristics_by_endpoint: &HashMap<EndpointId, Characteristic>,
    routing_profile: Option<super::DeviceRoutingProfile>,
) -> Result<(), InteractionError> {
    let Some(joint_mode) = routing_profile.and_then(|profile| profile.joint_mode) else {
        return Ok(());
    };

    let Some(write_characteristic) =
        characteristics_by_endpoint.get(&EndpointId::WriteCharacteristic)
    else {
        return Ok(());
    };

    let payload = [0x05, 0x00, 0x0C, 0x80, joint_mode];
    peripheral
        .write(write_characteristic, &payload, WriteType::WithResponse)
        .await?;
    Ok(())
}

#[instrument(skip(peripheral, characteristics_by_endpoint), level = "debug")]
async fn query_led_info(
    peripheral: &Peripheral,
    characteristics_by_endpoint: &HashMap<EndpointId, Characteristic>,
) -> LedInfoQueryResult {
    let Some(write_characteristic) =
        characteristics_by_endpoint.get(&EndpointId::WriteCharacteristic)
    else {
        return LedInfoQueryResult::skipped(LedInfoQueryOutcome::SkippedNoWriteCharacteristic);
    };
    let Some(read_characteristic) =
        characteristics_by_endpoint.get(&EndpointId::ReadNotifyCharacteristic)
    else {
        return LedInfoQueryResult::skipped(LedInfoQueryOutcome::SkippedNoNotifyOrRead);
    };
    let supports_read = read_characteristic.properties.contains(CharPropFlags::READ);
    let supports_notify = read_characteristic
        .properties
        .intersects(CharPropFlags::NOTIFY | CharPropFlags::INDICATE);
    if !supports_read && !supports_notify {
        trace!("skipping LED-info query because endpoint is neither readable nor notifiable");
        return LedInfoQueryResult::skipped(LedInfoQueryOutcome::SkippedNoNotifyOrRead);
    }

    let write_types = write_types_for_characteristic(write_characteristic.properties);
    if write_types.is_empty() {
        trace!("skipping LED-info query because write endpoint is not writable");
        return LedInfoQueryResult::skipped(LedInfoQueryOutcome::SkippedNoWriteCharacteristic);
    }

    let mut attempted_modes = Vec::with_capacity(write_types.len().saturating_mul(2));
    let mut last_payload = None;
    for write_type in write_types.iter().copied() {
        attempted_modes.push(format!("{}:get_led_type", write_type_label(write_type)));

        if supports_notify {
            match query_led_info_via_notify(
                peripheral,
                write_characteristic,
                write_type,
                read_characteristic,
                &GET_LED_INFO_QUERY,
            )
            .await
            {
                LedInfoProbeResult::Parsed { response, payload } => {
                    return LedInfoQueryResult::resolved(
                        response,
                        LedInfoQueryOutcome::ParsedNotify,
                        attempted_modes,
                        payload,
                    );
                }
                LedInfoProbeResult::InvalidPayload(payload) => {
                    last_payload = Some(payload);
                    if !supports_read {
                        continue;
                    }
                }
                LedInfoProbeResult::NoResponse => {
                    if !supports_read {
                        continue;
                    }
                }
            }
        }

        if supports_read {
            match query_led_info_via_read(
                peripheral,
                write_characteristic,
                write_type,
                read_characteristic,
                &GET_LED_INFO_QUERY,
            )
            .await
            {
                LedInfoProbeResult::Parsed { response, payload } => {
                    return LedInfoQueryResult::resolved(
                        response,
                        LedInfoQueryOutcome::ParsedRead,
                        attempted_modes,
                        payload,
                    );
                }
                LedInfoProbeResult::InvalidPayload(payload) => {
                    last_payload = Some(payload);
                }
                LedInfoProbeResult::NoResponse => {}
            }
        }
    }

    let mut sync_time_fallback_attempted = false;
    let sync_time_query = sync_time_query_frame(OffsetDateTime::now_utc());
    if supports_notify {
        sync_time_fallback_attempted = true;
        for write_type in write_types {
            attempted_modes.push(format!("{}:sync_time", write_type_label(write_type)));
            match query_led_info_via_notify(
                peripheral,
                write_characteristic,
                write_type,
                read_characteristic,
                &sync_time_query,
            )
            .await
            {
                LedInfoProbeResult::Parsed { response, payload } => {
                    return LedInfoQueryResult::resolved(
                        response,
                        LedInfoQueryOutcome::ParsedNotifyAfterSyncTime,
                        attempted_modes,
                        payload,
                    )
                    .mark_sync_time_fallback_attempted();
                }
                LedInfoProbeResult::InvalidPayload(payload) => {
                    last_payload = Some(payload);
                }
                LedInfoProbeResult::NoResponse => {}
            }
        }
    }

    if last_payload.is_some() {
        let unresolved = LedInfoQueryResult::unresolved(
            LedInfoQueryOutcome::InvalidResponse,
            attempted_modes,
            last_payload,
        );
        if sync_time_fallback_attempted {
            unresolved.mark_sync_time_fallback_attempted()
        } else {
            unresolved
        }
    } else {
        let unresolved =
            LedInfoQueryResult::unresolved(LedInfoQueryOutcome::NoResponse, attempted_modes, None);
        if sync_time_fallback_attempted {
            unresolved.mark_sync_time_fallback_attempted()
        } else {
            unresolved
        }
    }
}

#[instrument(
    skip(peripheral, write_characteristic, read_characteristic, query),
    level = "trace",
    fields(?write_type, query_len = query.len())
)]
async fn query_led_info_via_notify(
    peripheral: &Peripheral,
    write_characteristic: &Characteristic,
    write_type: WriteType,
    read_characteristic: &Characteristic,
    query: &[u8],
) -> LedInfoProbeResult {
    let mut notifications = match peripheral.notifications().await {
        Ok(stream) => stream,
        Err(error) => {
            trace!(
                ?error,
                "failed to open notification stream for LED-info query"
            );
            return LedInfoProbeResult::NoResponse;
        }
    };

    if let Err(error) = peripheral.subscribe(read_characteristic).await {
        trace!(
            ?error,
            "failed to subscribe for LED-info query notifications"
        );
        return LedInfoProbeResult::NoResponse;
    }

    if let Err(error) = peripheral
        .write(write_characteristic, query, write_type)
        .await
    {
        let _ = peripheral.unsubscribe(read_characteristic).await;
        trace!(?error, "failed to write LED-info query");
        return LedInfoProbeResult::NoResponse;
    }

    let deadline = tokio::time::Instant::now() + Duration::from_millis(LED_INFO_QUERY_TIMEOUT_MS);
    let mut first_invalid_payload = None;
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            let _ = peripheral.unsubscribe(read_characteristic).await;
            trace!("timed out waiting for LED-info notify response");
            return first_invalid_payload.map_or(
                LedInfoProbeResult::NoResponse,
                LedInfoProbeResult::InvalidPayload,
            );
        }

        let remaining = deadline - now;
        let notification = match timeout(remaining, notifications.next()).await {
            Ok(Some(value)) => value,
            Ok(None) => {
                let _ = peripheral.unsubscribe(read_characteristic).await;
                trace!("notification stream closed while waiting for LED-info response");
                return first_invalid_payload.map_or(
                    LedInfoProbeResult::NoResponse,
                    LedInfoProbeResult::InvalidPayload,
                );
            }
            Err(_elapsed) => {
                let _ = peripheral.unsubscribe(read_characteristic).await;
                trace!("timed out waiting for LED-info notify payload");
                return first_invalid_payload.map_or(
                    LedInfoProbeResult::NoResponse,
                    LedInfoProbeResult::InvalidPayload,
                );
            }
        };

        if !notification.uuid.eq(&read_characteristic.uuid) {
            continue;
        }

        let payload = notification.value;
        if let Some(parsed) = super::LedInfoResponse::parse(&payload) {
            let _ = peripheral.unsubscribe(read_characteristic).await;
            return LedInfoProbeResult::Parsed {
                response: parsed,
                payload,
            };
        }

        if first_invalid_payload.is_none() {
            first_invalid_payload = Some(payload);
        }
    }
}

#[instrument(
    skip(peripheral, write_characteristic, read_characteristic, query),
    level = "trace",
    fields(?write_type, query_len = query.len())
)]
async fn query_led_info_via_read(
    peripheral: &Peripheral,
    write_characteristic: &Characteristic,
    write_type: WriteType,
    read_characteristic: &Characteristic,
    query: &[u8],
) -> LedInfoProbeResult {
    if let Err(error) = peripheral
        .write(write_characteristic, query, write_type)
        .await
    {
        trace!(?error, "failed to write LED-info query");
        return LedInfoProbeResult::NoResponse;
    }

    match timeout(
        Duration::from_millis(LED_INFO_QUERY_TIMEOUT_MS),
        peripheral.read(read_characteristic),
    )
    .await
    {
        Ok(Ok(payload)) => {
            if let Some(parsed) = super::LedInfoResponse::parse(&payload) {
                LedInfoProbeResult::Parsed {
                    response: parsed,
                    payload,
                }
            } else {
                LedInfoProbeResult::InvalidPayload(payload)
            }
        }
        Ok(Err(error)) => {
            trace!(?error, "failed to read LED-info response");
            LedInfoProbeResult::NoResponse
        }
        Err(_elapsed) => {
            trace!("timed out waiting for LED-info read response");
            LedInfoProbeResult::NoResponse
        }
    }
}

fn write_type_label(write_type: WriteType) -> &'static str {
    match write_type {
        WriteType::WithResponse => "with_response",
        WriteType::WithoutResponse => "without_response",
    }
}

fn sync_time_query_frame(timestamp: OffsetDateTime) -> [u8; 11] {
    let year = u8::try_from(timestamp.year().rem_euclid(100))
        .expect("year modulo 100 should always fit in u8");
    let month = timestamp.month() as u8;
    let day = timestamp.day();
    let weekday = timestamp.weekday().number_from_monday();
    let hour = timestamp.hour();
    let minute = timestamp.minute();
    let second = timestamp.second();

    [
        0x0B, 0x00, 0x01, 0x80, year, month, day, weekday, hour, minute, second,
    ]
}

fn scan_identity_from_properties(properties: &PeripheralProperties) -> Option<ScanIdentity> {
    properties
        .manufacturer_data
        .iter()
        .find_map(|(company_id, payload)| {
            parse_scan_identity_from_manufacturer_record(*company_id, payload)
        })
}

fn parse_scan_identity_from_manufacturer_record(
    company_id: u16,
    payload: &[u8],
) -> Option<ScanIdentity> {
    ScanModelHandler::parse_identity_from_manufacturer_payload(payload).or_else(|| {
        let mut reconstructed_payload = Vec::with_capacity(payload.len() + 2);
        reconstructed_payload.extend_from_slice(&company_id.to_le_bytes());
        reconstructed_payload.extend_from_slice(payload);
        ScanModelHandler::parse_identity_from_manufacturer_payload(&reconstructed_payload)
    })
}

fn scan_properties_debug_from_properties(properties: &PeripheralProperties) -> ScanPropertiesDebug {
    let mut manufacturer_data: Vec<ManufacturerDataRecord> = properties
        .manufacturer_data
        .iter()
        .map(|(company_id, payload)| ManufacturerDataRecord::new(*company_id, payload.clone()))
        .collect();
    manufacturer_data.sort_by_key(ManufacturerDataRecord::company_id);

    let mut service_data: Vec<ServiceDataRecord> = properties
        .service_data
        .iter()
        .map(|(uuid, payload)| {
            ServiceDataRecord::new(uuid.to_string().to_lowercase(), payload.clone())
        })
        .collect();
    service_data.sort_by(|left, right| left.uuid().cmp(right.uuid()));

    let mut service_uuids: Vec<String> = properties
        .services
        .iter()
        .map(|uuid| uuid.to_string().to_lowercase())
        .collect();
    service_uuids.sort();

    ScanPropertiesDebug::new(manufacturer_data, service_data, service_uuids)
}

fn write_types_for_characteristic(properties: CharPropFlags) -> Vec<WriteType> {
    let mut write_types = Vec::with_capacity(2);
    if properties.contains(CharPropFlags::WRITE_WITHOUT_RESPONSE) {
        write_types.push(WriteType::WithoutResponse);
    }
    if properties.contains(CharPropFlags::WRITE) {
        write_types.push(WriteType::WithResponse);
    }
    write_types
}

fn negotiated_transport_write_limit(properties: CharPropFlags) -> Option<usize> {
    if properties.contains(CharPropFlags::WRITE_WITHOUT_RESPONSE) {
        let write_limit = if protocol::REQUESTED_ATT_MTU >= protocol::MTU_READY_THRESHOLD {
            protocol::TRANSPORT_CHUNK_MTU_READY
        } else {
            protocol::TRANSPORT_CHUNK_FALLBACK
        };
        return Some(write_limit);
    }

    if properties.contains(CharPropFlags::WRITE) {
        return Some(protocol::TRANSPORT_CHUNK_FALLBACK);
    }

    None
}

fn matches_name_prefix(local_name: Option<&str>, name_prefix: &str) -> bool {
    if name_prefix.is_empty() {
        return true;
    }

    local_name.is_some_and(|value| value.starts_with(name_prefix))
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

    fn device_routing_profile(&self) -> Option<super::DeviceRoutingProfile> {
        self.session_metadata.device_routing_profile()
    }

    #[instrument(skip(self), level = "trace", fields(?endpoint))]
    async fn read_endpoint(&self, endpoint: EndpointId) -> Result<Vec<u8>, InteractionError> {
        let characteristic = self.characteristic_for(endpoint)?;
        let payload = self.peripheral.read(characteristic).await?;
        Ok(payload)
    }

    #[instrument(skip(self), level = "trace", fields(?endpoint))]
    async fn read_endpoint_optional(
        &self,
        endpoint: EndpointId,
    ) -> Result<Option<Vec<u8>>, InteractionError> {
        Ok(Some(self.read_endpoint(endpoint).await?))
    }

    #[instrument(skip(self, payload), level = "trace", fields(?endpoint, ?mode, payload_len = payload.len()))]
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

    #[instrument(skip(self), level = "trace", fields(?endpoint))]
    async fn subscribe_endpoint(&self, endpoint: EndpointId) -> Result<(), InteractionError> {
        let characteristic = self.characteristic_for(endpoint)?;
        self.peripheral.subscribe(characteristic).await?;
        Ok(())
    }

    #[instrument(skip(self), level = "trace", fields(?endpoint))]
    async fn unsubscribe_endpoint(&self, endpoint: EndpointId) -> Result<(), InteractionError> {
        let characteristic = self.characteristic_for(endpoint)?;
        self.peripheral.unsubscribe(characteristic).await?;
        Ok(())
    }

    #[instrument(
        skip(self, on_notification),
        level = "debug",
        fields(?endpoint, ?max_notifications)
    )]
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
                    signal.map_err(|source| InteractionError::CtrlC { source })?;
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

    #[instrument(skip(self), level = "debug")]
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
    scan_properties_debug: ScanPropertiesDebug,
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use btleplug::api::{PeripheralProperties, bleuuid::uuid_from_u16};
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use time::{Date, Month, PrimitiveDateTime, Time, UtcOffset};

    use super::*;

    fn manufacturer_properties(company_id: u16, payload: &[u8]) -> PeripheralProperties {
        let mut manufacturer_data = HashMap::new();
        manufacturer_data.insert(company_id, payload.to_vec());

        PeripheralProperties {
            manufacturer_data,
            ..PeripheralProperties::default()
        }
    }

    #[rstest]
    #[case(
        0x0000,
        &[0x54, 0x52, 0x00, 0x70, 0x04, 0x10, 0x11, 0x00, 0x01, 0x05, 0x20, 0x00, 0x20, 0x00],
        Some((4, 1, 5))
    )]
    #[case(
        0x5254,
        &[0x00, 0x70, 0x04, 0x10, 0x11, 0x00, 0x01, 0x05, 0x20, 0x00, 0x20, 0x00],
        Some((4, 1, 5))
    )]
    #[case(0x004C, &[0x00, 0x70, 0x04, 0x10, 0x11, 0x00], None)]
    fn scan_identity_from_properties_handles_company_id_split_payloads(
        #[case] company_id: u16,
        #[case] payload: &[u8],
        #[case] expected: Option<(i8, u8, u8)>,
    ) {
        let properties = manufacturer_properties(company_id, payload);
        let identity = scan_identity_from_properties(&properties);

        let observed = identity.map(|value| (value.shape, value.cid, value.pid));
        assert_eq!(expected, observed);
    }

    #[rstest]
    #[case(CharPropFlags::WRITE_WITHOUT_RESPONSE, vec![WriteType::WithoutResponse])]
    #[case(CharPropFlags::WRITE, vec![WriteType::WithResponse])]
    #[case(
        CharPropFlags::WRITE_WITHOUT_RESPONSE | CharPropFlags::WRITE,
        vec![WriteType::WithoutResponse, WriteType::WithResponse]
    )]
    #[case(CharPropFlags::READ, vec![])]
    fn write_types_for_characteristic_prefers_without_response_when_available(
        #[case] properties: CharPropFlags,
        #[case] expected: Vec<WriteType>,
    ) {
        let resolved = write_types_for_characteristic(properties);
        assert_eq!(expected, resolved);
    }

    #[rstest]
    #[case(
        CharPropFlags::WRITE_WITHOUT_RESPONSE,
        Some(protocol::TRANSPORT_CHUNK_MTU_READY)
    )]
    #[case(CharPropFlags::WRITE, Some(protocol::TRANSPORT_CHUNK_FALLBACK))]
    #[case(
        CharPropFlags::WRITE_WITHOUT_RESPONSE | CharPropFlags::WRITE,
        Some(protocol::TRANSPORT_CHUNK_MTU_READY)
    )]
    #[case(CharPropFlags::READ, None)]
    fn negotiated_transport_write_limit_resolves_expected_values(
        #[case] properties: CharPropFlags,
        #[case] expected: Option<usize>,
    ) {
        let resolved = negotiated_transport_write_limit(properties);
        assert_eq!(expected, resolved);
    }

    #[test]
    fn scan_properties_debug_preserves_raw_advertisement_fields() {
        let mut properties = PeripheralProperties::default();
        properties
            .manufacturer_data
            .insert(0x004C, vec![0x00, 0x70]);
        properties
            .manufacturer_data
            .insert(0x5254, vec![0x00, 0x70, 0x04, 0x01, 0x02]);
        let service_data_uuid = uuid_from_u16(0xFA03);
        properties
            .service_data
            .insert(service_data_uuid, vec![0x01, 0x80, 0x04]);
        properties.services = vec![uuid_from_u16(0xAE00), uuid_from_u16(0x00FA)];

        let debug = scan_properties_debug_from_properties(&properties);
        let manufacturer: Vec<(u16, Vec<u8>)> = debug
            .manufacturer_data()
            .iter()
            .map(|record| (record.company_id(), record.payload().to_vec()))
            .collect();
        let service_data: Vec<(String, Vec<u8>)> = debug
            .service_data()
            .iter()
            .map(|record| (record.uuid().to_string(), record.payload().to_vec()))
            .collect();

        assert_eq!(
            vec![
                (0x004C, vec![0x00, 0x70]),
                (0x5254, vec![0x00, 0x70, 0x04, 0x01, 0x02])
            ],
            manufacturer
        );
        assert_eq!(
            vec![(
                "0000fa03-0000-1000-8000-00805f9b34fb".to_string(),
                vec![0x01, 0x80, 0x04]
            )],
            service_data
        );
        assert_eq!(
            vec![
                "000000fa-0000-1000-8000-00805f9b34fb".to_string(),
                "0000ae00-0000-1000-8000-00805f9b34fb".to_string(),
            ],
            debug.service_uuids().to_vec()
        );
    }

    #[test]
    fn sync_time_query_frame_matches_short_frame_shape() {
        let date = Date::from_calendar_date(2026, Month::February, 16)
            .expect("test calendar date should be valid");
        let time = Time::from_hms(9, 30, 45).expect("test wall-clock time should be valid");
        let timestamp = PrimitiveDateTime::new(date, time).assume_offset(UtcOffset::UTC);

        let frame = sync_time_query_frame(timestamp);
        assert_eq!(
            [
                0x0B, 0x00, 0x01, 0x80, 0x1A, 0x02, 0x10, 0x01, 0x09, 0x1E, 0x2D
            ],
            frame
        );
    }
}
