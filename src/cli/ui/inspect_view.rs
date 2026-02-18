use std::fmt::{self, Display, Formatter};

use idm_macros::DiagnosticsSection;

use crate::hw::diagnostic_value::{Bytes, MissingOr, NoneOr, UnknownOr, YesNo};
use crate::hw::diagnostics::DiagnosticRow;
use crate::hw::{GattProfile, InspectReport, ServiceInfo, TextPath};
use crate::protocol;

use super::device_view::DeviceView;
use super::painter::Painter;
use super::table::Table;

/// Renders a full inspect report with device, endpoint, and service tables.
pub(crate) struct InspectReportView<'a> {
    report: &'a InspectReport,
    painter: &'a Painter,
}

#[derive(Debug, DiagnosticsSection)]
#[diagnostics(id = "session_metadata", section = "Session metadata")]
struct SessionMetadataSection {
    #[diagnostic(name = "Required endpoints verified")]
    required_endpoints_verified: YesNo,
    #[diagnostic(name = "GATT profile")]
    gatt_profile: UnknownOr<GattProfile>,
    #[diagnostic(name = "Write-without-response limit")]
    write_without_response_limit: UnknownOr<Bytes>,
    #[diagnostic(name = "Discovered services")]
    discovered_services: ServiceCount,
    #[diagnostic(name = "Discovered characteristics")]
    discovered_characteristics: CharacteristicCount,
    #[diagnostic(name = "Write characteristic properties")]
    write_characteristic_properties: MissingOr<String>,
    #[diagnostic(name = "Read/notify characteristic properties")]
    read_notify_characteristic_properties: MissingOr<String>,
    #[diagnostic(name = "Resolved write characteristic UUID")]
    resolved_write_characteristic_uuid: UnknownOr<String>,
    #[diagnostic(name = "Resolved read/notify UUID")]
    resolved_read_notify_uuid: UnknownOr<String>,
    #[diagnostic(name = "Profile panel dimensions")]
    profile_panel_dimensions: UnknownOr<crate::hw::PanelDimensions>,
    #[diagnostic(name = "Profile LED type")]
    profile_led_type: UnknownOr<u8>,
    #[diagnostic(name = "Profile text path")]
    profile_text_path: UnknownOr<TextPath>,
    #[diagnostic(name = "Profile joint mode")]
    profile_joint_mode: NoneOr<u8>,
    #[diagnostic(name = "Profile image upload mode")]
    profile_image_upload_mode: crate::hw::ImageUploadMode,
    #[diagnostic(name = "Profile GIF header")]
    profile_gif_header: crate::hw::GifHeaderProfile,
    #[diagnostic(name = "Profile write chunk fallback")]
    profile_write_chunk_fallback: Bytes,
}

fn endpoint_properties(
    report: &InspectReport,
    endpoint: protocol::EndpointId,
) -> MissingOr<String> {
    let expected_uuid = report
        .session_metadata()
        .resolved_endpoint_uuid(endpoint)
        .unwrap_or_else(|| protocol::endpoint_metadata(endpoint).uuid());

    MissingOr(
        report
            .services()
            .iter()
            .flat_map(|service| service.characteristics().iter())
            .find(|characteristic| characteristic.uuid().eq_ignore_ascii_case(expected_uuid))
            .map(|characteristic| characteristic.properties().join(",")),
    )
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct ServiceCount(usize);

impl ServiceCount {
    fn from_services(services: &[ServiceInfo]) -> Self {
        Self(services.len())
    }
}

impl Display for ServiceCount {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct CharacteristicCount(usize);

impl CharacteristicCount {
    fn from_services(services: &[ServiceInfo]) -> Self {
        Self(
            services
                .iter()
                .map(|service| service.characteristics().len())
                .sum(),
        )
    }
}

impl Display for CharacteristicCount {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&InspectReport> for SessionMetadataSection {
    fn from(report: &InspectReport) -> Self {
        let metadata = report.session_metadata();
        let profile = metadata.device_profile();

        Self {
            required_endpoints_verified: YesNo(metadata.required_endpoints_verified()),
            gatt_profile: UnknownOr(metadata.gatt_profile()),
            write_without_response_limit: UnknownOr(
                metadata.write_without_response_limit().map(Bytes),
            ),
            discovered_services: ServiceCount::from_services(report.services()),
            discovered_characteristics: CharacteristicCount::from_services(report.services()),
            write_characteristic_properties: endpoint_properties(
                report,
                protocol::EndpointId::WriteCharacteristic,
            ),
            read_notify_characteristic_properties: endpoint_properties(
                report,
                protocol::EndpointId::ReadNotifyCharacteristic,
            ),
            resolved_write_characteristic_uuid: UnknownOr(
                metadata
                    .resolved_endpoint_uuid(protocol::EndpointId::WriteCharacteristic)
                    .map(str::to_owned),
            ),
            resolved_read_notify_uuid: UnknownOr(
                metadata
                    .resolved_endpoint_uuid(protocol::EndpointId::ReadNotifyCharacteristic)
                    .map(str::to_owned),
            ),
            profile_panel_dimensions: UnknownOr(profile.panel_dimensions()),
            profile_led_type: UnknownOr(profile.led_type()),
            profile_text_path: UnknownOr(profile.text_path()),
            profile_joint_mode: NoneOr(profile.joint_mode()),
            profile_image_upload_mode: profile.image_upload_mode(),
            profile_gif_header: profile.gif_header_profile(),
            profile_write_chunk_fallback: Bytes(profile.write_without_response_fallback()),
        }
    }
}

impl<'a> InspectReportView<'a> {
    pub(crate) fn new(report: &'a InspectReport, painter: &'a Painter) -> Self {
        Self { report, painter }
    }

    fn endpoints_table(&self) -> Table {
        let endpoints = self.report.endpoint_presence();
        let rows = protocol::known_endpoints()
            .map(|endpoint| {
                let metadata = protocol::endpoint_metadata(endpoint);
                vec![
                    self.painter.value(metadata.uuid()),
                    self.painter.muted(metadata.kind().to_string()),
                    self.painter.value(metadata.name()),
                    if endpoints.is_present(endpoint) {
                        self.painter.success("present")
                    } else {
                        self.painter.warning("missing")
                    },
                ]
            })
            .collect();
        Table::grid(["uuid", "kind", "name", "status"], rows)
    }

    fn session_table(&self) -> Table {
        let section = SessionMetadataSection::from(self.report);

        self.section_table(&section)
    }

    fn services_table(&self) -> Table {
        let mut rows = Vec::new();
        for service in self.report.services() {
            if service.characteristics().is_empty() {
                rows.push(vec![
                    self.painter.value(service.uuid()),
                    if service.is_primary() {
                        self.painter.success("yes")
                    } else {
                        self.painter.muted("no")
                    },
                    self.painter.warning("<none>"),
                    self.painter.warning("<none>"),
                ]);
                continue;
            }

            for characteristic in service.characteristics() {
                rows.push(vec![
                    self.painter.value(service.uuid()),
                    if service.is_primary() {
                        self.painter.success("yes")
                    } else {
                        self.painter.muted("no")
                    },
                    self.painter.value(characteristic.uuid()),
                    self.painter.value(characteristic.properties().join(",")),
                ]);
            }
        }
        Table::grid(
            [
                "service_uuid",
                "primary",
                "characteristic_uuid",
                "properties",
            ],
            rows,
        )
    }

    fn diagnostic_value(&self, value: &str) -> String {
        if value == "yes" {
            return self.painter.success(value);
        }
        if value == "no" {
            return self.painter.warning(value);
        }
        if value.starts_with('<') && value.ends_with('>') {
            return self.painter.warning(value);
        }

        self.painter.value(value)
    }

    fn rows_table(&self, rows: &[DiagnosticRow]) -> Table {
        let rows = rows
            .iter()
            .map(|row| (row.label(), self.diagnostic_value(row.value())))
            .collect();
        Table::key_value(self.painter, rows)
    }

    fn section_table(&self, section: &dyn crate::hw::diagnostics::DiagnosticsSection) -> Table {
        let section_rows = section.rows();
        self.rows_table(&section_rows)
    }

    fn connection_diagnostics_tables(&self) -> Vec<(String, Table)> {
        self.report
            .session_metadata()
            .connection_diagnostics()
            .sections()
            .iter()
            .map(|section| (section.name().to_string(), self.rows_table(section.rows())))
            .collect()
    }
}

impl Display for InspectReportView<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let device = DeviceView::new(self.report.device(), self.painter);

        write!(f, "{}", self.painter.heading("Connected device:"))?;
        write!(f, "\n{device}")?;
        writeln!(f)?;
        write!(f, "\n{}", self.painter.heading("Session metadata:"))?;
        write!(f, "\n{}", self.session_table())?;
        if !self
            .report
            .session_metadata()
            .connection_diagnostics()
            .is_empty()
        {
            let diagnostics_tables = self.connection_diagnostics_tables();
            writeln!(f)?;
            write!(f, "\n{}", self.painter.heading("Connection diagnostics:"))?;
            for (section_name, table) in diagnostics_tables {
                writeln!(f)?;
                write!(f, "\n{}", self.painter.value(format!("{section_name}:")))?;
                write!(f, "\n{table}")?;
            }
        }
        writeln!(f)?;
        write!(
            f,
            "\n{}",
            self.painter.heading("Expected iDotMatrix endpoints:")
        )?;
        write!(f, "\n{}", self.endpoints_table())?;
        writeln!(f)?;
        write!(f, "\n{}", self.painter.heading("Discovered GATT services:"))?;
        write!(f, "\n{}", self.services_table())
    }
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;

    use crate::hw::{
        CharacteristicInfo, DeviceProfile, EndpointPresence, FoundDevice, GifHeaderProfile,
        ImageUploadMode, ServiceInfo, SessionMetadata,
    };
    use crate::protocol;

    use super::*;

    fn full_report() -> InspectReport {
        let device = FoundDevice::new(
            "hci0".into(),
            "AA:BB:CC".into(),
            Some("IDM-Clock".into()),
            Some(-43),
        );
        let services = vec![ServiceInfo::new(
            "000000fa-0000-1000-8000-00805f9b34fb".into(),
            true,
            vec![
                CharacteristicInfo::new(
                    "0000fa02-0000-1000-8000-00805f9b34fb".into(),
                    vec!["write".into()],
                ),
                CharacteristicInfo::new(
                    "0000fa03-0000-1000-8000-00805f9b34fb".into(),
                    vec!["read".into(), "notify".into()],
                ),
            ],
        )];
        let mut presence = protocol::empty_presence_map();
        for endpoint in protocol::known_endpoints() {
            presence.insert(endpoint, true);
        }
        InspectReport::new(
            device,
            services,
            EndpointPresence::new(presence),
            SessionMetadata::new(
                true,
                Some(514),
                DeviceProfile::new(None, GifHeaderProfile::Timed, ImageUploadMode::PngFile, 512),
            ),
        )
    }

    #[test]
    fn inspect_report_renders_all_sections() {
        let report = full_report();
        let painter = Painter::new(false);
        assert_snapshot!(
            "inspect_report_all_sections",
            InspectReportView::new(&report, &painter).to_string()
        );
    }

    #[test]
    fn service_without_characteristics() {
        let device = FoundDevice::new(
            "hci0".into(),
            "AA:BB:CC".into(),
            Some("IDM-Clock".into()),
            Some(-43),
        );
        let services = vec![ServiceInfo::new(
            "00001800-0000-1000-8000-00805f9b34fb".into(),
            false,
            vec![],
        )];
        let presence = protocol::empty_presence_map();
        let report = InspectReport::new(
            device,
            services,
            EndpointPresence::new(presence),
            SessionMetadata::new(
                false,
                None,
                DeviceProfile::new(None, GifHeaderProfile::Timed, ImageUploadMode::PngFile, 512),
            ),
        );
        let painter = Painter::new(false);
        assert_snapshot!(
            "inspect_service_no_characteristics",
            InspectReportView::new(&report, &painter).to_string()
        );
    }
}
