use idm_macros::{DiagnosticsSection, HasDiagnostics};

use super::diagnostic_value::{HexBytes, JoinedStrings, NoneOr, YesNo};
use super::diagnostics::{ConnectionDiagnostics, ConnectionDiagnosticsBuilder};
use super::model::LedInfoQueryOutcome;
use super::scan_model::ScanIdentity;
use crate::utils::format_hex;

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct ManufacturerDataRecord {
    company_id: u16,
    payload: Vec<u8>,
}

impl ManufacturerDataRecord {
    pub(crate) fn new(company_id: u16, payload: Vec<u8>) -> Self {
        Self {
            company_id,
            payload,
        }
    }

    pub(crate) fn company_id(&self) -> u16 {
        self.company_id
    }

    pub(crate) fn payload(&self) -> &[u8] {
        &self.payload
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct ServiceDataRecord {
    uuid: String,
    payload: Vec<u8>,
}

impl ServiceDataRecord {
    pub(crate) fn new(uuid: String, payload: Vec<u8>) -> Self {
        Self { uuid, payload }
    }

    pub(crate) fn uuid(&self) -> &str {
        &self.uuid
    }

    pub(crate) fn payload(&self) -> &[u8] {
        &self.payload
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct ScanPropertiesDebug {
    manufacturer_data: Vec<ManufacturerDataRecord>,
    service_data: Vec<ServiceDataRecord>,
    service_uuids: Vec<String>,
}

impl ScanPropertiesDebug {
    pub(crate) fn new(
        manufacturer_data: Vec<ManufacturerDataRecord>,
        service_data: Vec<ServiceDataRecord>,
        service_uuids: Vec<String>,
    ) -> Self {
        Self {
            manufacturer_data,
            service_data,
            service_uuids,
        }
    }

    pub(crate) fn manufacturer_data(&self) -> &[ManufacturerDataRecord] {
        &self.manufacturer_data
    }

    pub(crate) fn service_data(&self) -> &[ServiceDataRecord] {
        &self.service_data
    }

    pub(crate) fn service_uuids(&self) -> &[String] {
        &self.service_uuids
    }
}

#[derive(Debug, Clone, Eq, PartialEq, DiagnosticsSection)]
#[diagnostics(id = "scan_identity", section = "Scan identity")]
struct ScanIdentitySection {
    #[diagnostic(name = "Identity present")]
    identity_present: YesNo,
    #[diagnostic(name = "Shape")]
    shape: NoneOr<i8>,
    #[diagnostic(name = "CID")]
    cid: NoneOr<u8>,
    #[diagnostic(name = "PID")]
    pid: NoneOr<u8>,
}

impl ScanIdentitySection {
    fn from_scan_identity(scan_identity: Option<ScanIdentity>) -> Self {
        Self {
            identity_present: YesNo(scan_identity.is_some()),
            shape: NoneOr(scan_identity.map(|identity| identity.shape)),
            cid: NoneOr(scan_identity.map(|identity| identity.cid)),
            pid: NoneOr(scan_identity.map(|identity| identity.pid)),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, DiagnosticsSection)]
#[diagnostics(id = "advertisement_data", section = "Advertisement data")]
struct AdvertisementDataSection {
    #[diagnostic(name = "Manufacturer data")]
    manufacturer_data: NoneOr<JoinedStrings>,
    #[diagnostic(name = "Service data")]
    service_data: NoneOr<JoinedStrings>,
    #[diagnostic(name = "Services")]
    services: NoneOr<JoinedStrings>,
}

impl AdvertisementDataSection {
    fn from_scan_properties(scan_properties_debug: Option<&ScanPropertiesDebug>) -> Self {
        let Some(scan_debug) = scan_properties_debug else {
            return Self {
                manufacturer_data: NoneOr(None),
                service_data: NoneOr(None),
                services: NoneOr(None),
            };
        };

        let manufacturer_data = JoinedStrings::semicolon(
            scan_debug
                .manufacturer_data()
                .iter()
                .map(|record| {
                    format!(
                        "0x{:04X}={}",
                        record.company_id(),
                        format_hex(record.payload())
                    )
                })
                .collect(),
        )
        .into_option();

        let service_data = JoinedStrings::semicolon(
            scan_debug
                .service_data()
                .iter()
                .map(|record| format!("{}={}", record.uuid(), format_hex(record.payload())))
                .collect(),
        )
        .into_option();

        let services = JoinedStrings::comma(scan_debug.service_uuids().to_vec()).into_option();

        Self {
            manufacturer_data: NoneOr(manufacturer_data),
            service_data: NoneOr(service_data),
            services: NoneOr(services),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, DiagnosticsSection)]
#[diagnostics(id = "led_info_probe", section = "LED-info probe")]
struct LedInfoProbeSection {
    #[diagnostic(name = "Query outcome")]
    query_outcome: LedInfoQueryOutcome,
    #[diagnostic(name = "Write modes attempted")]
    write_modes_attempted: NoneOr<JoinedStrings>,
    #[diagnostic(name = "Sync-time fallback attempted")]
    sync_time_fallback_attempted: YesNo,
    #[diagnostic(name = "Last payload")]
    last_payload: NoneOr<HexBytes>,
}

impl LedInfoProbeSection {
    fn from_led_info_probe(
        outcome: LedInfoQueryOutcome,
        write_modes_attempted: Vec<String>,
        sync_time_fallback_attempted: bool,
        last_payload: Option<Vec<u8>>,
    ) -> Self {
        Self {
            query_outcome: outcome,
            write_modes_attempted: NoneOr(
                JoinedStrings::comma(write_modes_attempted).into_option(),
            ),
            sync_time_fallback_attempted: YesNo(sync_time_fallback_attempted),
            last_payload: NoneOr(last_payload.map(HexBytes)),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, HasDiagnostics)]
struct ModelResolutionSections {
    #[diagnostic]
    scan_identity: ScanIdentitySection,
    #[diagnostic]
    advertisement_data: AdvertisementDataSection,
    #[diagnostic]
    led_info_probe: LedInfoProbeSection,
}

/// Constructs connect-time model resolution diagnostics.
pub(crate) fn model_resolution_diagnostics(
    scan_identity: Option<ScanIdentity>,
    scan_properties_debug: Option<&ScanPropertiesDebug>,
    led_info_query_outcome: LedInfoQueryOutcome,
    led_info_write_modes_attempted: Vec<String>,
    led_info_sync_time_fallback_attempted: bool,
    led_info_last_payload: Option<Vec<u8>>,
) -> ConnectionDiagnostics {
    let sections = ModelResolutionSections {
        scan_identity: ScanIdentitySection::from_scan_identity(scan_identity),
        advertisement_data: AdvertisementDataSection::from_scan_properties(scan_properties_debug),
        led_info_probe: LedInfoProbeSection::from_led_info_probe(
            led_info_query_outcome,
            led_info_write_modes_attempted,
            led_info_sync_time_fallback_attempted,
            led_info_last_payload,
        ),
    };

    let mut diagnostics = ConnectionDiagnosticsBuilder::new();
    diagnostics.extend(&sections);
    diagnostics.finish()
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::*;

    fn section_rows(
        diagnostics: &ConnectionDiagnostics,
        section_id: &str,
    ) -> Vec<(String, String)> {
        let section = diagnostics
            .sections()
            .iter()
            .find(|section| section.id() == section_id)
            .expect("expected diagnostics section to exist");
        section
            .rows()
            .iter()
            .map(|row| (row.label().to_string(), row.value().to_string()))
            .collect()
    }

    #[rstest]
    #[case(
        None,
        vec![
            ("Identity present".to_string(), "no".to_string()),
            ("Shape".to_string(), "<none>".to_string()),
            ("CID".to_string(), "<none>".to_string()),
            ("PID".to_string(), "<none>".to_string()),
        ],
    )]
    #[case(
        Some(ScanIdentity {
            cid: 8,
            pid: 1,
            shape: 4,
            reverse: false,
            group_id: 5,
            device_id: 3,
            lamp_count: 0,
            lamp_num: 0,
        }),
        vec![
            ("Identity present".to_string(), "yes".to_string()),
            ("Shape".to_string(), "4".to_string()),
            ("CID".to_string(), "8".to_string()),
            ("PID".to_string(), "1".to_string()),
        ],
    )]
    fn scan_identity_section_formats_expected_rows(
        #[case] scan_identity: Option<ScanIdentity>,
        #[case] expected_rows: Vec<(String, String)>,
    ) {
        let diagnostics = model_resolution_diagnostics(
            scan_identity,
            None,
            LedInfoQueryOutcome::NoResponse,
            Vec::new(),
            false,
            None,
        );
        let rows = section_rows(&diagnostics, "scan_identity");
        assert_eq!(expected_rows, rows);
    }

    #[test]
    fn advertisement_and_led_info_sections_render_expected_values() {
        let scan_properties_debug = ScanPropertiesDebug::new(
            vec![ManufacturerDataRecord::new(
                0x5254,
                vec![0x00, 0x70, 0x04, 0x05, 0x03, 0x00, 0x08, 0x01],
            )],
            vec![ServiceDataRecord::new(
                "0000fa03-0000-1000-8000-00805f9b34fb".to_string(),
                vec![0x01, 0x80, 0x04],
            )],
            vec!["000000fa-0000-1000-8000-00805f9b34fb".to_string()],
        );
        let diagnostics = model_resolution_diagnostics(
            None,
            Some(&scan_properties_debug),
            LedInfoQueryOutcome::ParsedNotifyAfterSyncTime,
            vec![
                "without_response:get_led_type".to_string(),
                "with_response:get_led_type".to_string(),
            ],
            true,
            Some(vec![0x09, 0x00, 0x01, 0x80, 0x05, 0x03, 0x01, 0x04, 0x00]),
        );

        assert_eq!(
            vec![
                (
                    "Manufacturer data".to_string(),
                    "0x5254=00 70 04 05 03 00 08 01".to_string(),
                ),
                (
                    "Service data".to_string(),
                    "0000fa03-0000-1000-8000-00805f9b34fb=01 80 04".to_string(),
                ),
                (
                    "Services".to_string(),
                    "000000fa-0000-1000-8000-00805f9b34fb".to_string(),
                ),
            ],
            section_rows(&diagnostics, "advertisement_data")
        );
        assert_eq!(
            vec![
                (
                    "Query outcome".to_string(),
                    "parsed_notify_after_sync_time".to_string(),
                ),
                (
                    "Write modes attempted".to_string(),
                    "without_response:get_led_type,with_response:get_led_type".to_string(),
                ),
                (
                    "Sync-time fallback attempted".to_string(),
                    "yes".to_string()
                ),
                (
                    "Last payload".to_string(),
                    "09 00 01 80 05 03 01 04 00".to_string(),
                ),
            ],
            section_rows(&diagnostics, "led_info_probe")
        );
    }
}
