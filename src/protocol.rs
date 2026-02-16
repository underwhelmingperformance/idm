use std::collections::HashMap;
use std::sync::LazyLock;

use strum::IntoEnumIterator;
use strum_macros::{Display, EnumIter};

/// Known iDotMatrix protocol endpoints.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, EnumIter, Display)]
pub enum EndpointId {
    /// iDotMatrix primary control service.
    #[strum(to_string = "control_service")]
    ControlService,
    /// Characteristic used for command/data writes.
    #[strum(to_string = "write_characteristic")]
    WriteCharacteristic,
    /// Characteristic used for reads and notifications.
    #[strum(to_string = "read_notify_characteristic")]
    ReadNotifyCharacteristic,
}

/// Endpoint category in GATT.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Display)]
pub(crate) enum EndpointKind {
    /// GATT service endpoint.
    #[strum(to_string = "service")]
    Service,
    /// GATT characteristic endpoint.
    #[strum(to_string = "characteristic")]
    Characteristic,
}

/// Descriptive metadata for one protocol endpoint.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct EndpointMetadata {
    name: &'static str,
    uuid: &'static str,
    kind: EndpointKind,
}

impl EndpointMetadata {
    /// Human-readable endpoint name.
    pub(crate) fn name(self) -> &'static str {
        self.name
    }

    /// Endpoint UUID.
    pub(crate) fn uuid(self) -> &'static str {
        self.uuid
    }

    /// Endpoint kind.
    pub(crate) fn kind(self) -> EndpointKind {
        self.kind
    }
}

/// Endpoint metadata keyed by typed endpoint IDs.
pub(crate) static ENDPOINTS_BY_ID: LazyLock<HashMap<EndpointId, EndpointMetadata>> =
    LazyLock::new(|| {
        EndpointId::iter()
            .map(|endpoint| (endpoint, metadata_for(endpoint)))
            .collect()
    });

/// Returns metadata for one endpoint.
pub(crate) fn endpoint_metadata(endpoint: EndpointId) -> EndpointMetadata {
    *ENDPOINTS_BY_ID
        .get(&endpoint)
        .unwrap_or(&metadata_for(endpoint))
}

/// Returns all known endpoints.
pub(crate) fn known_endpoints() -> impl Iterator<Item = EndpointId> {
    EndpointId::iter()
}

/// Creates a presence map initialised with all known endpoints set to `false`.
pub(crate) fn empty_presence_map() -> HashMap<EndpointId, bool> {
    known_endpoints()
        .map(|endpoint| (endpoint, false))
        .collect()
}

fn metadata_for(endpoint: EndpointId) -> EndpointMetadata {
    match endpoint {
        EndpointId::ControlService => EndpointMetadata {
            name: "iDotMatrix control service",
            uuid: "000000fa-0000-1000-8000-00805f9b34fb",
            kind: EndpointKind::Service,
        },
        EndpointId::WriteCharacteristic => EndpointMetadata {
            name: "iDotMatrix write data",
            uuid: "0000fa02-0000-1000-8000-00805f9b34fb",
            kind: EndpointKind::Characteristic,
        },
        EndpointId::ReadNotifyCharacteristic => EndpointMetadata {
            name: "iDotMatrix read/notify data",
            uuid: "0000fa03-0000-1000-8000-00805f9b34fb",
            kind: EndpointKind::Characteristic,
        },
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn endpoint_metadata_contains_expected_names() {
        let control = endpoint_metadata(EndpointId::ControlService);
        assert_eq!("iDotMatrix control service", control.name());

        let write = endpoint_metadata(EndpointId::WriteCharacteristic);
        assert_eq!("iDotMatrix write data", write.name());
    }
}
