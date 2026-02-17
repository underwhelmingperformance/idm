use derive_more::Display;
use thiserror::Error;
use tracing::instrument;

use crate::hw::LedInfoResponse;

/// Transfer families used by notification flow-control responses.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Display)]
pub enum TransferFamily {
    /// Text upload transfer family.
    #[display("text")]
    Text,
    /// GIF upload transfer family.
    #[display("gif")]
    Gif,
    /// Image upload transfer family.
    #[display("image")]
    Image,
    /// DIY upload transfer family.
    #[display("diy")]
    Diy,
    /// Timer transfer family.
    #[display("timer")]
    Timer,
    /// OTA transfer family.
    #[display("ota")]
    Ota,
}

/// Typed notification events emitted by iDotMatrix devices.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum NotifyEvent {
    /// Per-chunk flow-control acknowledgement.
    NextPackage(TransferFamily),
    /// Transfer completion acknowledgement.
    Finished(TransferFamily),
    /// Family-specific error status.
    Error(TransferFamily, u8),
    /// Schedule setup response byte.
    ScheduleSetup(u8),
    /// Schedule master-switch response byte.
    ScheduleMasterSwitch(u8),
    /// Parsed `Get LED type` response.
    LedInfo(LedInfoResponse),
    /// Parsed screen-light readback timeout value.
    ScreenLightTimeout(u8),
    /// Unrecognised notification payload preserved as raw bytes.
    Unknown(Vec<u8>),
}

/// Errors returned while decoding notification payloads.
#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum NotificationDecodeError {
    #[error("notification payload was empty")]
    EmptyPayload,
}

/// Decodes raw `fa03` notification payloads into typed events.
pub struct NotificationHandler;

impl NotificationHandler {
    /// Decodes one notification payload.
    ///
    /// # Errors
    ///
    /// Returns an error when the payload is empty.
    #[instrument(skip(payload), level = "trace", fields(payload_len = payload.len()))]
    pub fn decode(payload: &[u8]) -> Result<NotifyEvent, NotificationDecodeError> {
        if payload.is_empty() {
            return Err(NotificationDecodeError::EmptyPayload);
        }

        if let Some(led_info) = LedInfoResponse::parse(payload) {
            return Ok(NotifyEvent::LedInfo(led_info));
        }

        if payload.len() >= 5 && payload[0] == 0x05 && payload[1] == 0x00 {
            if payload[2] == 0x0F && payload[3] == 0x80 {
                return Ok(NotifyEvent::ScreenLightTimeout(payload[4]));
            }
            if payload[2] == 0x05 && payload[3] == 0x80 {
                return Ok(NotifyEvent::ScheduleSetup(payload[4]));
            }
            if payload[2] == 0x07 && payload[3] == 0x80 {
                return Ok(NotifyEvent::ScheduleMasterSwitch(payload[4]));
            }
        }

        if payload.len() >= 5 {
            let status = payload[4];
            let maybe_family = match (payload[1], payload[2], payload[3]) {
                (0x00, 0x03, 0x00) => Some(TransferFamily::Text),
                (0x00, 0x01, 0x00) => Some(TransferFamily::Gif),
                (0x00, 0x02, 0x00) => Some(TransferFamily::Image),
                (0x00, 0x00, 0x00) => Some(TransferFamily::Diy),
                (0x00, 0x00, 0x80) => Some(TransferFamily::Timer),
                (0x00, 0x01, 0xC0) => Some(TransferFamily::Ota),
                _ => None,
            };

            if let Some(family) = maybe_family {
                return Ok(decode_transfer_status(family, status));
            }
        }

        Ok(NotifyEvent::Unknown(payload.to_vec()))
    }
}

fn decode_transfer_status(family: TransferFamily, status: u8) -> NotifyEvent {
    match family {
        TransferFamily::Text => match status {
            0x01 => NotifyEvent::NextPackage(TransferFamily::Text),
            0x03 => NotifyEvent::Finished(TransferFamily::Text),
            other => NotifyEvent::Error(TransferFamily::Text, other),
        },
        TransferFamily::Gif => match status {
            0x01 => NotifyEvent::NextPackage(TransferFamily::Gif),
            0x03 => NotifyEvent::Finished(TransferFamily::Gif),
            other => NotifyEvent::Error(TransferFamily::Gif, other),
        },
        TransferFamily::Image => match status {
            0x01 => NotifyEvent::NextPackage(TransferFamily::Image),
            0x03 => NotifyEvent::Finished(TransferFamily::Image),
            other => NotifyEvent::Error(TransferFamily::Image, other),
        },
        TransferFamily::Diy => match status {
            0x02 => NotifyEvent::NextPackage(TransferFamily::Diy),
            0x00 | 0x01 => NotifyEvent::Finished(TransferFamily::Diy),
            other => NotifyEvent::Error(TransferFamily::Diy, other),
        },
        TransferFamily::Timer => match status {
            0x01 => NotifyEvent::NextPackage(TransferFamily::Timer),
            0x03 => NotifyEvent::Finished(TransferFamily::Timer),
            other => NotifyEvent::Error(TransferFamily::Timer, other),
        },
        TransferFamily::Ota => match status {
            0x01 => NotifyEvent::NextPackage(TransferFamily::Ota),
            0x03 => NotifyEvent::Finished(TransferFamily::Ota),
            other => NotifyEvent::Error(TransferFamily::Ota, other),
        },
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case([0x05, 0x00, 0x03, 0x00, 0x01], NotifyEvent::NextPackage(TransferFamily::Text))]
    #[case([0x05, 0x00, 0x03, 0x00, 0x03], NotifyEvent::Finished(TransferFamily::Text))]
    #[case([0x05, 0x00, 0x03, 0x00, 0x02], NotifyEvent::Error(TransferFamily::Text, 0x02))]
    #[case([0x05, 0x00, 0x01, 0x00, 0x00], NotifyEvent::Error(TransferFamily::Gif, 0x00))]
    #[case([0x05, 0x00, 0x01, 0x00, 0x01], NotifyEvent::NextPackage(TransferFamily::Gif))]
    #[case([0x05, 0x00, 0x02, 0x00, 0x03], NotifyEvent::Finished(TransferFamily::Image))]
    #[case([0x05, 0x00, 0x00, 0x00, 0x02], NotifyEvent::NextPackage(TransferFamily::Diy))]
    #[case([0x05, 0x00, 0x00, 0x00, 0x00], NotifyEvent::Finished(TransferFamily::Diy))]
    #[case([0x05, 0x00, 0x00, 0x80, 0x01], NotifyEvent::NextPackage(TransferFamily::Timer))]
    #[case([0x05, 0x00, 0x01, 0xC0, 0x03], NotifyEvent::Finished(TransferFamily::Ota))]
    fn decode_maps_transfer_family_packets(
        #[case] payload: [u8; 5],
        #[case] expected: NotifyEvent,
    ) {
        let decoded =
            NotificationHandler::decode(&payload).expect("known packet should decode cleanly");
        assert_eq!(expected, decoded);
    }

    #[rstest]
    #[case([0x05, 0x00, 0x05, 0x80, 0x01], NotifyEvent::ScheduleSetup(0x01))]
    #[case(
        [0x05, 0x00, 0x07, 0x80, 0x03],
        NotifyEvent::ScheduleMasterSwitch(0x03)
    )]
    #[case(
        [0x05, 0x00, 0x0F, 0x80, 0x1E],
        NotifyEvent::ScreenLightTimeout(0x1E)
    )]
    fn decode_maps_schedule_and_state_packets(
        #[case] payload: [u8; 5],
        #[case] expected: NotifyEvent,
    ) {
        let decoded =
            NotificationHandler::decode(&payload).expect("known packet should decode cleanly");
        assert_eq!(expected, decoded);
    }

    #[test]
    fn decode_maps_led_info_response() {
        let payload = [0x09, 0x00, 0x01, 0x80, 0x02, 0x0A, 0x01, 0x04, 0x00];
        let decoded =
            NotificationHandler::decode(&payload).expect("LED info payload should decode cleanly");

        assert_eq!(
            NotifyEvent::LedInfo(LedInfoResponse {
                mcu_major_version: 0x02,
                mcu_minor_version: 0x0A,
                status: 0x01,
                screen_type: 0x04,
                password_enabled: false,
            }),
            decoded
        );
    }

    #[test]
    fn decode_preserves_unknown_payload() {
        let payload = [0xAA, 0x55, 0x01];
        let decoded = NotificationHandler::decode(&payload)
            .expect("unknown non-empty payload should decode as Unknown");
        assert_eq!(NotifyEvent::Unknown(payload.to_vec()), decoded);
    }

    #[test]
    fn decode_rejects_empty_payload() {
        let decoded = NotificationHandler::decode(&[]);
        assert_matches!(decoded, Err(NotificationDecodeError::EmptyPayload));
    }
}
