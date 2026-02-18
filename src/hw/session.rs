use serde_with::SerializeDisplay;
use std::collections::HashMap;
use tracing::instrument;

use super::hardware::missing_required_endpoints;
use super::model::{CharacteristicInfo, EndpointPresence, ServiceInfo};
use crate::error::InteractionError;
use crate::protocol::{self, EndpointId};

pub(crate) const FA_SERVICE_UUID: &str = "000000fa-0000-1000-8000-00805f9b34fb";
pub(crate) const FA_WRITE_UUID: &str = "0000fa02-0000-1000-8000-00805f9b34fb";
pub(crate) const FEE9_SERVICE_UUID: &str = "0000fee9-0000-1000-8000-00805f9b34fb";
pub(crate) const D44_WRITE_UUID: &str = "d44bc439-abfd-45a2-b575-925416129600";
pub(crate) const D44_NOTIFY_UUID: &str = "d44bc439-abfd-45a2-b575-925416129601";
pub(crate) const D44_READ_UUID: &str = "d44bc439-abfd-45a2-b575-925416129602";
pub(crate) const OTA_SERVICE_UUID: &str = "0000ae00-0000-1000-8000-00805f9b34fb";
pub(crate) const OTA_WRITE_UUID: &str = "0000ae01-0000-1000-8000-00805f9b34fb";
pub(crate) const OTA_NOTIFY_UUID: &str = "0000ae02-0000-1000-8000-00805f9b34fb";

#[derive(Debug, Clone, Copy, Eq, PartialEq, derive_more::Display, SerializeDisplay)]
pub enum GattProfile {
    #[display("fa_fa02")]
    FaFa02,
    #[display("fee9_d44")]
    Fee9D44,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct OtaEndpointSet {
    pub(crate) service_uuid: String,
    pub(crate) write_uuid: String,
    pub(crate) notify_uuid: String,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct NegotiatedSessionEndpoints {
    pub(crate) gatt_profile: GattProfile,
    pub(crate) endpoint_uuids: HashMap<EndpointId, String>,
    pub(crate) ota_endpoints: Option<OtaEndpointSet>,
}

impl NegotiatedSessionEndpoints {
    pub(crate) fn endpoint_presence(&self) -> EndpointPresence {
        let mut by_endpoint = protocol::empty_presence_map();
        for endpoint in self.endpoint_uuids.keys() {
            by_endpoint.insert(*endpoint, true);
        }
        EndpointPresence::new(by_endpoint)
    }
}

pub(crate) fn negotiate_session_endpoints(
    services: &[ServiceInfo],
) -> Result<NegotiatedSessionEndpoints, InteractionError> {
    negotiate_session_endpoints_inner(services)
}

#[instrument(skip(services), level = "debug", fields(service_count = services.len()))]
fn negotiate_session_endpoints_inner(
    services: &[ServiceInfo],
) -> Result<NegotiatedSessionEndpoints, InteractionError> {
    const PROFILES: [ProfileCandidate; 2] = [
        ProfileCandidate {
            gatt_profile: GattProfile::FaFa02,
            control_service_uuid: FA_SERVICE_UUID,
            write_characteristic_uuid: FA_WRITE_UUID,
        },
        ProfileCandidate {
            gatt_profile: GattProfile::Fee9D44,
            control_service_uuid: FEE9_SERVICE_UUID,
            write_characteristic_uuid: D44_WRITE_UUID,
        },
    ];

    for candidate in PROFILES {
        let Some(service) = find_service(services, candidate.control_service_uuid) else {
            continue;
        };
        let Some(write_characteristic) =
            find_characteristic(service, candidate.write_characteristic_uuid)
        else {
            continue;
        };
        if !supports_write(write_characteristic) {
            continue;
        }

        let Some(read_or_notify) = select_read_or_notify_characteristic(service) else {
            continue;
        };

        let endpoint_uuids = HashMap::from([
            (
                EndpointId::ControlService,
                service.uuid().to_ascii_lowercase(),
            ),
            (
                EndpointId::WriteCharacteristic,
                write_characteristic.uuid().to_ascii_lowercase(),
            ),
            (
                EndpointId::ReadNotifyCharacteristic,
                read_or_notify.uuid().to_ascii_lowercase(),
            ),
        ]);

        return Ok(NegotiatedSessionEndpoints {
            gatt_profile: candidate.gatt_profile,
            endpoint_uuids,
            ota_endpoints: resolve_ota_endpoints(services),
        });
    }

    let presence = infer_required_endpoint_presence(services);
    let missing = missing_required_endpoints(&presence);
    Err(InteractionError::MissingRequiredEndpoints {
        missing: format_missing_endpoints(&missing),
    })
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct ProfileCandidate {
    gatt_profile: GattProfile,
    control_service_uuid: &'static str,
    write_characteristic_uuid: &'static str,
}

fn find_service<'a>(services: &'a [ServiceInfo], uuid: &str) -> Option<&'a ServiceInfo> {
    services
        .iter()
        .find(|service| service.uuid().eq_ignore_ascii_case(uuid))
}

fn find_characteristic<'a>(service: &'a ServiceInfo, uuid: &str) -> Option<&'a CharacteristicInfo> {
    service
        .characteristics()
        .iter()
        .find(|characteristic| characteristic.uuid().eq_ignore_ascii_case(uuid))
}

fn supports_write(characteristic: &CharacteristicInfo) -> bool {
    characteristic_has_property(characteristic, "write")
        || characteristic_has_property(characteristic, "write_without_response")
}

fn select_read_or_notify_characteristic(service: &ServiceInfo) -> Option<&CharacteristicInfo> {
    if let Some(characteristic) = find_characteristic(service, D44_READ_UUID)
        && (supports_read(characteristic) || supports_notify(characteristic))
    {
        return Some(characteristic);
    }

    if let Some(characteristic) = find_characteristic(service, D44_NOTIFY_UUID)
        && supports_notify(characteristic)
    {
        return Some(characteristic);
    }

    service
        .characteristics()
        .iter()
        .find(|characteristic| supports_notify(characteristic))
}

fn supports_read(characteristic: &CharacteristicInfo) -> bool {
    characteristic_has_property(characteristic, "read")
}

fn supports_notify(characteristic: &CharacteristicInfo) -> bool {
    characteristic_has_property(characteristic, "notify")
        || characteristic_has_property(characteristic, "indicate")
}

fn characteristic_has_property(characteristic: &CharacteristicInfo, property: &str) -> bool {
    characteristic
        .properties()
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(property))
}

fn resolve_ota_endpoints(services: &[ServiceInfo]) -> Option<OtaEndpointSet> {
    let service = find_service(services, OTA_SERVICE_UUID)?;
    let write_characteristic = find_characteristic(service, OTA_WRITE_UUID)?;
    let notify_characteristic = find_characteristic(service, OTA_NOTIFY_UUID)?;

    Some(OtaEndpointSet {
        service_uuid: service.uuid().to_ascii_lowercase(),
        write_uuid: write_characteristic.uuid().to_ascii_lowercase(),
        notify_uuid: notify_characteristic.uuid().to_ascii_lowercase(),
    })
}

fn infer_required_endpoint_presence(services: &[ServiceInfo]) -> EndpointPresence {
    const PROFILES: [ProfileCandidate; 2] = [
        ProfileCandidate {
            gatt_profile: GattProfile::FaFa02,
            control_service_uuid: FA_SERVICE_UUID,
            write_characteristic_uuid: FA_WRITE_UUID,
        },
        ProfileCandidate {
            gatt_profile: GattProfile::Fee9D44,
            control_service_uuid: FEE9_SERVICE_UUID,
            write_characteristic_uuid: D44_WRITE_UUID,
        },
    ];

    let mut has_control_service = false;
    let mut has_write_characteristic = false;
    let mut has_read_notify_characteristic = false;

    for candidate in PROFILES {
        let Some(service) = find_service(services, candidate.control_service_uuid) else {
            continue;
        };
        has_control_service = true;

        let Some(write_characteristic) =
            find_characteristic(service, candidate.write_characteristic_uuid)
        else {
            continue;
        };
        if !supports_write(write_characteristic) {
            continue;
        }
        has_write_characteristic = true;

        if select_read_or_notify_characteristic(service).is_some() {
            has_read_notify_characteristic = true;
        }
    }

    EndpointPresence::new(HashMap::from([
        (EndpointId::ControlService, has_control_service),
        (EndpointId::WriteCharacteristic, has_write_characteristic),
        (
            EndpointId::ReadNotifyCharacteristic,
            has_read_notify_characteristic,
        ),
    ]))
}

fn format_missing_endpoints(endpoints: &[EndpointId]) -> String {
    endpoints
        .iter()
        .map(|endpoint| {
            let metadata = protocol::endpoint_metadata(*endpoint);
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
    use crate::hw::{CharacteristicInfo, ServiceInfo};

    fn characteristic(uuid: &str, properties: &[&str]) -> CharacteristicInfo {
        CharacteristicInfo::new(
            uuid.to_string(),
            properties
                .iter()
                .map(|property| (*property).to_string())
                .collect(),
        )
    }

    fn service(uuid: &str, characteristics: Vec<CharacteristicInfo>) -> ServiceInfo {
        ServiceInfo::new(uuid.to_string(), true, characteristics)
    }

    #[rstest]
    #[case(
        vec![service(
            FA_SERVICE_UUID,
            vec![
                characteristic(FA_WRITE_UUID, &["write"]),
                characteristic("0000fa03-0000-1000-8000-00805f9b34fb", &["notify"]),
            ],
        )],
        GattProfile::FaFa02,
        EndpointId::ReadNotifyCharacteristic,
        "0000fa03-0000-1000-8000-00805f9b34fb"
    )]
    #[case(
        vec![service(
            FEE9_SERVICE_UUID,
            vec![
                characteristic(D44_WRITE_UUID, &["write_without_response"]),
                characteristic(D44_NOTIFY_UUID, &["notify"]),
            ],
        )],
        GattProfile::Fee9D44,
        EndpointId::WriteCharacteristic,
        D44_WRITE_UUID
    )]
    fn negotiation_selects_expected_profile_and_endpoint(
        #[case] services: Vec<ServiceInfo>,
        #[case] expected_profile: GattProfile,
        #[case] endpoint: EndpointId,
        #[case] expected_uuid: &str,
    ) {
        let negotiated =
            negotiate_session_endpoints(&services).expect("session negotiation should resolve");

        assert_eq!(expected_profile, negotiated.gatt_profile);
        assert_eq!(
            Some(expected_uuid),
            negotiated.endpoint_uuids.get(&endpoint).map(String::as_str)
        );
    }

    #[rstest]
    #[case(
        vec![
            characteristic(D44_WRITE_UUID, &["write"]),
            characteristic(D44_NOTIFY_UUID, &["notify"]),
            characteristic(D44_READ_UUID, &["read", "notify"]),
        ],
        D44_READ_UUID
    )]
    #[case(
        vec![
            characteristic(D44_WRITE_UUID, &["write"]),
            characteristic(D44_NOTIFY_UUID, &["notify"]),
        ],
        D44_NOTIFY_UUID
    )]
    fn negotiation_prefers_known_read_notify_uuids(
        #[case] characteristics: Vec<CharacteristicInfo>,
        #[case] expected_read_uuid: &str,
    ) {
        let services = vec![service(FEE9_SERVICE_UUID, characteristics)];
        let negotiated =
            negotiate_session_endpoints(&services).expect("read/notify endpoint should resolve");

        assert_eq!(
            Some(expected_read_uuid),
            negotiated
                .endpoint_uuids
                .get(&EndpointId::ReadNotifyCharacteristic)
                .map(String::as_str)
        );
    }

    #[rstest]
    #[case(
        vec![service(
            FA_SERVICE_UUID,
            vec![characteristic(FA_WRITE_UUID, &["write"])],
        )],
        "read/notify"
    )]
    #[case(
        vec![service(
            FA_SERVICE_UUID,
            vec![characteristic("0000fa03-0000-1000-8000-00805f9b34fb", &["notify"])],
        )],
        "write data"
    )]
    fn negotiation_returns_missing_endpoints_error(
        #[case] services: Vec<ServiceInfo>,
        #[case] expected_fragment: &str,
    ) {
        let error =
            negotiate_session_endpoints(&services).expect_err("read/notify should be required");

        assert_matches!(
            error,
            InteractionError::MissingRequiredEndpoints { missing }
            if missing.contains(expected_fragment)
        );
    }
}
