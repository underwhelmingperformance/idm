use std::collections::HashMap;

use crate::protocol::EndpointId;

use super::DeviceProfile;
use super::session::GattProfile;

/// A discovered BLE peripheral that matched a scan predicate.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct FoundDevice {
    adapter_name: String,
    device_id: String,
    local_name: Option<String>,
    rssi: Option<i16>,
}

impl FoundDevice {
    /// Creates a new discovered-device record.
    pub(crate) fn new(
        adapter_name: String,
        device_id: String,
        local_name: Option<String>,
        rssi: Option<i16>,
    ) -> Self {
        Self {
            adapter_name,
            device_id,
            local_name,
            rssi,
        }
    }

    /// Returns the adapter name used to discover this device.
    #[must_use]
    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    /// Returns the backend-specific device identifier.
    #[must_use]
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// Returns the advertised local name, if present.
    #[must_use]
    pub fn local_name(&self) -> Option<&str> {
        self.local_name.as_deref()
    }

    /// Returns the latest observed RSSI value, if present.
    #[must_use]
    pub fn rssi(&self) -> Option<i16> {
        self.rssi
    }

    /// Returns whether the local name starts with a prefix.
    pub(crate) fn local_name_starts_with(&self, prefix: &str) -> bool {
        self.local_name
            .as_deref()
            .is_some_and(|name| name.starts_with(prefix))
    }
}

/// A characteristic description discovered on a connected peripheral.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CharacteristicInfo {
    uuid: String,
    properties: Vec<String>,
}

impl CharacteristicInfo {
    /// Creates a characteristic description.
    pub(crate) fn new(uuid: String, properties: Vec<String>) -> Self {
        Self { uuid, properties }
    }

    /// Returns the characteristic UUID.
    #[must_use]
    pub fn uuid(&self) -> &str {
        &self.uuid
    }

    /// Returns property labels for this characteristic.
    #[must_use]
    pub fn properties(&self) -> &[String] {
        &self.properties
    }
}

/// A GATT service with discovered characteristics.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ServiceInfo {
    uuid: String,
    primary: bool,
    characteristics: Vec<CharacteristicInfo>,
}

impl ServiceInfo {
    /// Creates a service description.
    pub(crate) fn new(
        uuid: String,
        primary: bool,
        characteristics: Vec<CharacteristicInfo>,
    ) -> Self {
        Self {
            uuid,
            primary,
            characteristics,
        }
    }

    /// Returns the service UUID.
    #[must_use]
    pub fn uuid(&self) -> &str {
        &self.uuid
    }

    /// Returns whether this is a primary service.
    #[must_use]
    pub fn is_primary(&self) -> bool {
        self.primary
    }

    /// Returns all characteristics in this service.
    #[must_use]
    pub fn characteristics(&self) -> &[CharacteristicInfo] {
        &self.characteristics
    }
}

/// Presence flags for the reverse-engineered iDotMatrix endpoints.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct EndpointPresence {
    by_endpoint: HashMap<EndpointId, bool>,
}

impl EndpointPresence {
    /// Creates endpoint-presence flags.
    pub(crate) fn new(by_endpoint: HashMap<EndpointId, bool>) -> Self {
        Self { by_endpoint }
    }

    /// Returns whether an endpoint is present on the connected device.
    #[must_use]
    pub fn is_present(&self, endpoint: EndpointId) -> bool {
        self.by_endpoint.get(&endpoint).copied().unwrap_or(false)
    }
}

/// Connection metadata discovered during session setup.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SessionMetadata {
    required_endpoints_verified: bool,
    write_without_response_limit: Option<usize>,
    device_profile: DeviceProfile,
    gatt_profile: Option<GattProfile>,
    resolved_endpoint_uuids: HashMap<EndpointId, String>,
}

impl SessionMetadata {
    /// Creates session metadata.
    pub(crate) fn new(
        required_endpoints_verified: bool,
        write_without_response_limit: Option<usize>,
        device_profile: DeviceProfile,
    ) -> Self {
        Self {
            required_endpoints_verified,
            write_without_response_limit,
            device_profile,
            gatt_profile: None,
            resolved_endpoint_uuids: HashMap::new(),
        }
    }

    pub(crate) fn with_endpoint_resolution(
        mut self,
        gatt_profile: GattProfile,
        resolved_endpoint_uuids: HashMap<EndpointId, String>,
    ) -> Self {
        self.gatt_profile = Some(gatt_profile);
        self.resolved_endpoint_uuids = resolved_endpoint_uuids;
        self
    }

    /// Returns whether required iDotMatrix endpoints were verified at connect time.
    #[must_use]
    pub fn required_endpoints_verified(&self) -> bool {
        self.required_endpoints_verified
    }

    /// Returns the negotiated write-without-response payload limit, when known.
    #[must_use]
    pub fn write_without_response_limit(&self) -> Option<usize> {
        self.write_without_response_limit
    }

    /// Returns the resolved device profile used for handler behaviour.
    #[must_use]
    pub fn device_profile(&self) -> DeviceProfile {
        self.device_profile
    }

    /// Returns the resolved GATT profile selected during session setup.
    #[must_use]
    pub fn gatt_profile(&self) -> Option<GattProfile> {
        self.gatt_profile
    }

    /// Returns the concrete UUID bound to an endpoint role for this session.
    #[must_use]
    pub fn resolved_endpoint_uuid(&self, endpoint: EndpointId) -> Option<&str> {
        self.resolved_endpoint_uuids
            .get(&endpoint)
            .map(String::as_str)
    }
}

/// Result of inspecting a connected iDotMatrix peripheral.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct InspectReport {
    device: FoundDevice,
    services: Vec<ServiceInfo>,
    endpoint_presence: EndpointPresence,
    session_metadata: SessionMetadata,
}

impl InspectReport {
    /// Creates an inspect report.
    pub(crate) fn new(
        device: FoundDevice,
        services: Vec<ServiceInfo>,
        endpoint_presence: EndpointPresence,
        session_metadata: SessionMetadata,
    ) -> Self {
        Self {
            device,
            services,
            endpoint_presence,
            session_metadata,
        }
    }

    /// Returns the connected device details.
    #[must_use]
    pub fn device(&self) -> &FoundDevice {
        &self.device
    }

    /// Returns discovered services.
    #[must_use]
    pub fn services(&self) -> &[ServiceInfo] {
        &self.services
    }

    /// Returns expected iDotMatrix endpoint presence.
    #[must_use]
    pub fn endpoint_presence(&self) -> &EndpointPresence {
        &self.endpoint_presence
    }

    /// Returns session metadata discovered while connecting.
    #[must_use]
    pub fn session_metadata(&self) -> &SessionMetadata {
        &self.session_metadata
    }
}

/// Why a listening session ended.
#[derive(Debug, Clone, Eq, PartialEq, derive_more::Display)]
pub enum ListenStopReason {
    /// The listener reached the requested max notification count.
    #[display("reached max notifications ({_0})")]
    ReachedLimit(usize),
    /// The user interrupted the listener.
    #[display("interrupted by user")]
    Interrupted,
    /// The notification stream ended naturally.
    #[display("notification stream closed")]
    NotificationStreamClosed,
}

/// Summary of a notification stream run.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct NotificationRunSummary {
    received_notifications: usize,
    stop_reason: ListenStopReason,
}

impl NotificationRunSummary {
    /// Creates a notification run summary.
    pub(crate) fn new(received_notifications: usize, stop_reason: ListenStopReason) -> Self {
        Self {
            received_notifications,
            stop_reason,
        }
    }

    /// Returns the number of notifications received.
    #[must_use]
    pub fn received_notifications(&self) -> usize {
        self.received_notifications
    }

    /// Returns why notification listening ended.
    #[must_use]
    pub fn stop_reason(&self) -> &ListenStopReason {
        &self.stop_reason
    }
}

/// Summary returned when a listen session exits.
#[derive(Debug, Eq, PartialEq)]
pub struct ListenSummary {
    device: FoundDevice,
    initial_read: Option<Vec<u8>>,
    received_notifications: usize,
    stop_reason: ListenStopReason,
}

impl ListenSummary {
    /// Creates a listen summary.
    pub(crate) fn new(
        device: FoundDevice,
        initial_read: Option<Vec<u8>>,
        received_notifications: usize,
        stop_reason: ListenStopReason,
    ) -> Self {
        Self {
            device,
            initial_read,
            received_notifications,
            stop_reason,
        }
    }

    /// Returns connected device details.
    #[must_use]
    pub fn device(&self) -> &FoundDevice {
        &self.device
    }

    /// Returns the initial read payload from `fa03`, if any.
    #[must_use]
    pub fn initial_read(&self) -> Option<&[u8]> {
        self.initial_read.as_deref()
    }

    /// Returns the number of notifications received.
    #[must_use]
    pub fn received_notifications(&self) -> usize {
        self.received_notifications
    }

    /// Returns the reason the listen session ended.
    #[must_use]
    pub fn stop_reason(&self) -> &ListenStopReason {
        &self.stop_reason
    }
}
