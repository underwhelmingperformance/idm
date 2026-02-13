use std::fmt::{self, Display, Formatter};

use crate::hw::InspectReport;
use crate::protocol;

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
}

impl Display for InspectReportView<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let device = DeviceView::new(self.report.device(), self.painter);

        write!(f, "{}", self.painter.heading("Connected device:"))?;
        write!(f, "\n{device}")?;
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

    use crate::hw::{CharacteristicInfo, EndpointPresence, FoundDevice, ServiceInfo};
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
        InspectReport::new(device, services, EndpointPresence::new(presence))
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
        let report = InspectReport::new(device, services, EndpointPresence::new(presence));
        let painter = Painter::new(false);
        assert_snapshot!(
            "inspect_service_no_characteristics",
            InspectReportView::new(&report, &painter).to_string()
        );
    }
}
