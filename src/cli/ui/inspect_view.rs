use std::fmt::{self, Display, Formatter};

use crate::hw::InspectReport;
use crate::protocol;
use crate::utils::format_hex;

use super::device_view::DeviceView;
use super::painter::Painter;
use super::table::Table;

/// Renders a full inspect report with device, endpoint, and service tables.
pub(crate) struct InspectReportView<'a> {
    report: &'a InspectReport,
    painter: &'a Painter,
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
        let metadata = self.report.session_metadata();
        let profile = metadata.device_profile();
        let routing_profile = metadata.device_routing_profile();
        let gatt_profile = metadata.gatt_profile().map_or_else(
            || self.painter.warning("<unknown>"),
            |value| self.painter.value(value.to_string()),
        );
        let write_limit = match metadata.write_without_response_limit() {
            Some(limit) => self.painter.value(format!("{limit} bytes")),
            None => self.painter.warning("<unknown>"),
        };
        let service_count = self.report.services().len();
        let characteristic_count: usize = self
            .report
            .services()
            .iter()
            .map(|service| service.characteristics().len())
            .sum();

        let write_props = self.endpoint_properties(protocol::EndpointId::WriteCharacteristic);
        let read_notify_props =
            self.endpoint_properties(protocol::EndpointId::ReadNotifyCharacteristic);

        Table::key_value(
            self.painter,
            vec![
                (
                    "required_endpoints_verified",
                    if metadata.required_endpoints_verified() {
                        self.painter.success("yes")
                    } else {
                        self.painter.warning("no")
                    },
                ),
                ("gatt_profile", gatt_profile),
                ("write_without_response_limit", write_limit),
                (
                    "discovered_services",
                    self.painter.value(service_count.to_string()),
                ),
                (
                    "discovered_characteristics",
                    self.painter.value(characteristic_count.to_string()),
                ),
                (
                    "write_characteristic_properties",
                    write_props.map_or_else(
                        || self.painter.warning("<missing>"),
                        |value| self.painter.value(value),
                    ),
                ),
                (
                    "read_notify_characteristic_properties",
                    read_notify_props.map_or_else(
                        || self.painter.warning("<missing>"),
                        |value| self.painter.value(value),
                    ),
                ),
                (
                    "resolved_write_characteristic_uuid",
                    metadata
                        .resolved_endpoint_uuid(protocol::EndpointId::WriteCharacteristic)
                        .map_or_else(
                            || self.painter.warning("<unknown>"),
                            |value| self.painter.value(value),
                        ),
                ),
                (
                    "resolved_read_notify_uuid",
                    metadata
                        .resolved_endpoint_uuid(protocol::EndpointId::ReadNotifyCharacteristic)
                        .map_or_else(
                            || self.painter.warning("<unknown>"),
                            |value| self.painter.value(value),
                        ),
                ),
                (
                    "profile_panel_size",
                    self.painter.value(profile.panel_size().to_string()),
                ),
                (
                    "profile_led_type",
                    routing_profile
                        .and_then(|value| value.led_type)
                        .map_or_else(
                            || self.painter.warning("<unknown>"),
                            |value| self.painter.value(value.to_string()),
                        ),
                ),
                (
                    "profile_text_path",
                    routing_profile
                        .and_then(|value| value.text_path)
                        .map_or_else(
                            || self.painter.warning("<unknown>"),
                            |value| self.painter.value(value.to_string()),
                        ),
                ),
                (
                    "profile_joint_mode",
                    routing_profile
                        .and_then(|value| value.joint_mode)
                        .map_or_else(
                            || self.painter.warning("<none>"),
                            |value| self.painter.value(value.to_string()),
                        ),
                ),
                (
                    "profile_image_upload_mode",
                    self.painter.value(profile.image_upload_mode().to_string()),
                ),
                (
                    "profile_gif_header",
                    self.painter.value(profile.gif_header_profile().to_string()),
                ),
                (
                    "profile_write_chunk_fallback",
                    self.painter.value(format!(
                        "{} bytes",
                        profile.write_without_response_fallback()
                    )),
                ),
            ],
        )
    }

    fn endpoint_properties(&self, endpoint: protocol::EndpointId) -> Option<String> {
        let expected_uuid = self
            .report
            .session_metadata()
            .resolved_endpoint_uuid(endpoint)
            .unwrap_or_else(|| protocol::endpoint_metadata(endpoint).uuid());
        self.report
            .services()
            .iter()
            .flat_map(|service| service.characteristics().iter())
            .find(|characteristic| characteristic.uuid().eq_ignore_ascii_case(expected_uuid))
            .map(|characteristic| characteristic.properties().join(","))
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

    fn model_resolution_debug_table(&self) -> Option<Table> {
        let debug = self.report.session_metadata().model_resolution_debug()?;
        let scan_identity = debug.scan_identity();
        let write_modes = debug.led_info_write_modes_attempted();
        let scan_properties_debug = debug.scan_properties_debug();

        Some(Table::key_value(
            self.painter,
            vec![
                (
                    "Scan identity present",
                    if scan_identity.is_some() {
                        self.painter.success("yes")
                    } else {
                        self.painter.warning("no")
                    },
                ),
                (
                    "Scan identity shape",
                    scan_identity.map_or_else(
                        || self.painter.warning("<none>"),
                        |identity| self.painter.value(identity.shape.to_string()),
                    ),
                ),
                (
                    "Scan identity CID",
                    scan_identity.map_or_else(
                        || self.painter.warning("<none>"),
                        |identity| self.painter.value(identity.cid.to_string()),
                    ),
                ),
                (
                    "Scan identity PID",
                    scan_identity.map_or_else(
                        || self.painter.warning("<none>"),
                        |identity| self.painter.value(identity.pid.to_string()),
                    ),
                ),
                (
                    "CoreBluetooth manufacturer data",
                    scan_properties_debug.map_or_else(
                        || self.painter.warning("<none>"),
                        |scan_debug| {
                            if scan_debug.manufacturer_data().is_empty() {
                                return self.painter.warning("<none>");
                            }

                            let rendered = scan_debug
                                .manufacturer_data()
                                .iter()
                                .map(|record| {
                                    format!(
                                        "0x{:04X}={}",
                                        record.company_id(),
                                        format_hex(record.payload())
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join(";");
                            self.painter.value(rendered)
                        },
                    ),
                ),
                (
                    "CoreBluetooth service data",
                    scan_properties_debug.map_or_else(
                        || self.painter.warning("<none>"),
                        |scan_debug| {
                            if scan_debug.service_data().is_empty() {
                                return self.painter.warning("<none>");
                            }

                            let rendered = scan_debug
                                .service_data()
                                .iter()
                                .map(|record| {
                                    format!("{}={}", record.uuid(), format_hex(record.payload()))
                                })
                                .collect::<Vec<_>>()
                                .join(";");
                            self.painter.value(rendered)
                        },
                    ),
                ),
                (
                    "CoreBluetooth services",
                    scan_properties_debug.map_or_else(
                        || self.painter.warning("<none>"),
                        |scan_debug| {
                            if scan_debug.service_uuids().is_empty() {
                                return self.painter.warning("<none>");
                            }
                            self.painter.value(scan_debug.service_uuids().join(","))
                        },
                    ),
                ),
                (
                    "LED-info query outcome",
                    self.painter
                        .value(debug.led_info_query_outcome().to_string()),
                ),
                (
                    "LED-info write modes attempted",
                    if write_modes.is_empty() {
                        self.painter.warning("<none>")
                    } else {
                        self.painter.value(write_modes.join(","))
                    },
                ),
                (
                    "LED-info sync-time fallback attempted",
                    if debug.led_info_sync_time_fallback_attempted() {
                        self.painter.success("yes")
                    } else {
                        self.painter.warning("no")
                    },
                ),
                (
                    "LED-info last payload",
                    debug.led_info_last_payload().map_or_else(
                        || self.painter.warning("<none>"),
                        |payload| self.painter.value(format_hex(payload)),
                    ),
                ),
            ],
        ))
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
        if let Some(table) = self.model_resolution_debug_table() {
            writeln!(f)?;
            write!(f, "\n{}", self.painter.heading("Model resolution debug:"))?;
            write!(f, "\n{table}")?;
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
        ImageUploadMode, PanelSize, ServiceInfo, SessionMetadata,
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
                DeviceProfile::new(
                    PanelSize::Unknown,
                    GifHeaderProfile::Timed,
                    ImageUploadMode::PngFile,
                    512,
                ),
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
                DeviceProfile::new(
                    PanelSize::Unknown,
                    GifHeaderProfile::Timed,
                    ImageUploadMode::PngFile,
                    512,
                ),
            ),
        );
        let painter = Painter::new(false);
        assert_snapshot!(
            "inspect_service_no_characteristics",
            InspectReportView::new(&report, &painter).to_string()
        );
    }
}
