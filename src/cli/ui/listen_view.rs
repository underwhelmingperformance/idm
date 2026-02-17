use std::fmt::{self, Display, Formatter};

use crate::hw::{FoundDevice, ListenStopReason, ListenSummary};
use crate::protocol::{self, EndpointId};
use crate::utils::format_hex;

use super::device_view::DeviceView;
use super::painter::Painter;
use super::table::Table;

/// Renders the listen-session readiness output.
pub(crate) struct ListenReadyView<'a> {
    device: &'a FoundDevice,
    initial_read: Option<&'a [u8]>,
    painter: &'a Painter,
}

impl<'a> ListenReadyView<'a> {
    pub(crate) fn new(
        device: &'a FoundDevice,
        initial_read: Option<&'a [u8]>,
        painter: &'a Painter,
    ) -> Self {
        Self {
            device,
            initial_read,
            painter,
        }
    }
}

impl Display for ListenReadyView<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let endpoint = protocol::endpoint_metadata(EndpointId::ReadNotifyCharacteristic);
        let initial_read_value = match self.initial_read {
            Some(payload) => format_hex(payload),
            None => "<none>".to_string(),
        };

        let session_table = Table::key_value(
            self.painter,
            vec![
                (
                    "initial_read",
                    if self.initial_read.is_some() {
                        self.painter.value(&initial_read_value)
                    } else {
                        self.painter.warning(&initial_read_value)
                    },
                ),
                (
                    "listening_on",
                    format!(
                        "{} {}",
                        self.painter.value(endpoint.uuid()),
                        self.painter.muted(format!("({})", endpoint.name()))
                    ),
                ),
            ],
        );

        let device = DeviceView::new(self.device, self.painter);

        write!(f, "{}", self.painter.heading("Connected device:"))?;
        write!(f, "\n{device}")?;
        writeln!(f)?;
        write!(f, "\n{}", self.painter.heading("Listen session:"))?;
        write!(f, "\n{session_table}")
    }
}

/// Renders a single notification line.
pub(crate) struct ListenNotificationView<'a> {
    index: usize,
    payload: &'a [u8],
    event_label: Option<String>,
    painter: &'a Painter,
}

impl<'a> ListenNotificationView<'a> {
    pub(crate) fn new(
        index: usize,
        payload: &'a [u8],
        event_label: Option<String>,
        painter: &'a Painter,
    ) -> Self {
        Self {
            index,
            payload,
            event_label,
            painter,
        }
    }
}

impl Display for ListenNotificationView<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let index_label = self.painter.muted(format!("[{:04}]", self.index));
        let event_label = self.event_label.as_deref().unwrap_or("Notification");
        let raw_payload = self
            .painter
            .muted(format!("raw={}", format_hex(self.payload)));
        write!(
            f,
            "{index_label} {} {}",
            self.painter.value(event_label),
            raw_payload
        )
    }
}

/// Renders the listen session summary.
pub(crate) struct ListenSummaryView<'a> {
    summary: &'a ListenSummary,
    painter: &'a Painter,
}

impl<'a> ListenSummaryView<'a> {
    pub(crate) fn new(summary: &'a ListenSummary, painter: &'a Painter) -> Self {
        Self { summary, painter }
    }
}

impl Display for ListenSummaryView<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let stop_reason = match self.summary.stop_reason() {
            ListenStopReason::ReachedLimit(_) => {
                self.painter.success(self.summary.stop_reason().to_string())
            }
            ListenStopReason::Interrupted | ListenStopReason::NotificationStreamClosed => {
                self.painter.warning(self.summary.stop_reason().to_string())
            }
        };
        write!(
            f,
            "{} {} {}",
            self.painter.heading("Stopped:"),
            stop_reason,
            self.painter.value(format!(
                "- received {} notification(s)",
                self.summary.received_notifications()
            ))
        )
    }
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;
    use rstest::rstest;

    use crate::hw::FoundDevice;

    use super::*;

    fn device() -> FoundDevice {
        FoundDevice::new(
            "hci0".into(),
            "AA:BB:CC".into(),
            Some("IDM-Clock".into()),
            Some(-43),
        )
    }

    #[rstest]
    #[case::with_read(Some(vec![0xDE, 0xAD, 0xBE, 0xEF]), "listen_ready_with_read")]
    #[case::no_read(None, "listen_ready_no_read")]
    fn listen_ready_renders(#[case] initial_read: Option<Vec<u8>>, #[case] snapshot_name: &str) {
        let dev = device();
        let painter = Painter::new(false);
        let view = ListenReadyView::new(&dev, initial_read.as_deref(), &painter);
        assert_snapshot!(snapshot_name, view.to_string());
    }

    #[test]
    fn notification_formats_index_and_hex() {
        let painter = Painter::new(false);
        let payload = [0x05, 0x00, 0x01];
        let view = ListenNotificationView::new(42, &payload, None, &painter);
        assert_snapshot!("notification_line", view.to_string());
    }

    #[test]
    fn notification_formats_with_event_label() {
        let painter = Painter::new(false);
        let payload = [0x05, 0x00, 0x01];
        let view = ListenNotificationView::new(
            42,
            &payload,
            Some("Text next package".to_string()),
            &painter,
        );
        assert_snapshot!("notification_line_with_event", view.to_string());
    }

    #[rstest]
    #[case::reached_limit(ListenStopReason::ReachedLimit(10), "summary_reached_limit")]
    #[case::interrupted(ListenStopReason::Interrupted, "summary_interrupted")]
    fn summary_renders_stop_reason(
        #[case] stop_reason: ListenStopReason,
        #[case] snapshot_name: &str,
    ) {
        let dev = device();
        let summary = ListenSummary::new(dev, None, 5, stop_reason);
        let painter = Painter::new(false);
        assert_snapshot!(
            snapshot_name,
            ListenSummaryView::new(&summary, &painter).to_string()
        );
    }
}
