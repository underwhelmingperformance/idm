use std::collections::HashMap;

use crate::protocol::EndpointId;

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
#[derive(Debug, Eq, PartialEq)]
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
#[derive(Debug, Eq, PartialEq)]
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
#[derive(Debug, Eq, PartialEq)]
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

/// Result of inspecting a connected iDotMatrix peripheral.
#[derive(Debug, Eq, PartialEq)]
pub struct InspectReport {
    device: FoundDevice,
    services: Vec<ServiceInfo>,
    endpoint_presence: EndpointPresence,
}

impl InspectReport {
    /// Creates an inspect report.
    pub(crate) fn new(
        device: FoundDevice,
        services: Vec<ServiceInfo>,
        endpoint_presence: EndpointPresence,
    ) -> Self {
        Self {
            device,
            services,
            endpoint_presence,
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
}

/// Why a listening session ended.
#[derive(Debug, Eq, PartialEq, derive_more::Display)]
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
