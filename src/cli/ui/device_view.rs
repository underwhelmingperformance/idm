use std::fmt::{self, Display, Formatter};

use crate::hw::FoundDevice;
use crate::utils::format_rssi;

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

impl Display for DeviceView<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let name = self.device.local_name().unwrap_or("<unknown>");
        let table = Table::key_value(
            self.painter,
            vec![
                ("adapter", self.painter.value(self.device.adapter_name())),
                ("device_id", self.painter.value(self.device.device_id())),
                ("name", self.painter.value(name)),
                ("rssi", self.painter.value(format_rssi(self.device.rssi()))),
            ],
        );
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
