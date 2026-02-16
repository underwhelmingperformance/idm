use std::fmt::{self, Display, Formatter};

use idm_macros::DiagnosticsSection;

use crate::hw::FoundDevice;
use crate::hw::diagnostic_value::{Rssi, UnknownOr};
use crate::hw::diagnostics::DiagnosticsSection as _;

use super::painter::Painter;
use super::table::Table;

/// Renders a `FoundDevice` as a key-value table.
pub(crate) struct DeviceView<'a> {
    device: &'a FoundDevice,
    painter: &'a Painter,
}

impl<'a> DeviceView<'a> {
    pub(crate) fn new(device: &'a FoundDevice, painter: &'a Painter) -> Self {
        Self { device, painter }
    }
}

#[derive(Debug, DiagnosticsSection)]
#[diagnostics(id = "connected_device", section = "Connected device")]
struct ConnectedDeviceSection {
    #[diagnostic(name = "Adapter")]
    adapter: AdapterName,
    #[diagnostic(name = "Device ID")]
    device_id: DeviceIdentifier,
    #[diagnostic(name = "Name")]
    name: UnknownOr<String>,
    #[diagnostic(name = "RSSI")]
    rssi: Rssi,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct AdapterName(String);

impl Display for AdapterName {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct DeviceIdentifier(String);

impl Display for DeviceIdentifier {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&FoundDevice> for ConnectedDeviceSection {
    fn from(device: &FoundDevice) -> Self {
        Self {
            adapter: AdapterName(device.adapter_name().to_string()),
            device_id: DeviceIdentifier(device.device_id_display().to_string()),
            name: UnknownOr(device.local_name().map(str::to_owned)),
            rssi: Rssi(device.rssi()),
        }
    }
}

impl Display for DeviceView<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let section = ConnectedDeviceSection::from(self.device);
        let section_rows = section.rows();
        let rows = section_rows
            .iter()
            .map(|row| (row.label(), self.painter.value(row.value())))
            .collect();
        let table = Table::key_value(self.painter, rows);
        write!(f, "{table}")
    }
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;
    use rstest::rstest;

    use super::*;

    fn device(name: Option<&str>, rssi: Option<i16>) -> FoundDevice {
        FoundDevice::new(
            "hci0".into(),
            "AA:BB:CC".into(),
            name.map(String::from),
            rssi,
        )
    }

    #[rstest]
    #[case::all_fields(Some("IDM-Clock"), Some(-43), "device_all_fields")]
    #[case::missing_name(None, Some(-43), "device_missing_name")]
    #[case::missing_rssi(Some("IDM-Clock"), None, "device_missing_rssi")]
    fn device_view_renders(
        #[case] name: Option<&str>,
        #[case] rssi: Option<i16>,
        #[case] snapshot_name: &str,
    ) {
        let dev = device(name, rssi);
        let painter = Painter::new(false);
        assert_snapshot!(snapshot_name, DeviceView::new(&dev, &painter).to_string());
    }
}
