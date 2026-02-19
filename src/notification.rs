use std::fmt::{self, Display, Formatter};

use strum_macros::Display as StrumDisplay;
use thiserror::Error;
use tracing::instrument;

use crate::hw::LedInfoResponse;

/// Transfer families used by notification flow-control responses.
#[derive(Debug, Clone, Copy, Eq, PartialEq, StrumDisplay)]
#[strum(serialize_all = "title_case")]
pub enum TransferFamily {
    /// Text upload transfer family.
    Text,
    /// GIF upload transfer family.
    #[strum(to_string = "GIF")]
    Gif,
    /// Image upload transfer family.
    Image,
    /// DIY upload transfer family.
    #[strum(to_string = "DIY")]
    Diy,
    /// Timer transfer family.
    Timer,
    /// OTA transfer family.
    #[strum(to_string = "OTA")]
    Ota,
}

/// Decoded status for the schedule setup response.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ScheduleSetupStatus {
    /// The device accepted setup and is ready for the next phase.
    Success,
    /// The device requested the next queued schedule resource.
    Continue,
    /// The device rejected setup with a status byte.
    Failed(u8),
}

impl From<u8> for ScheduleSetupStatus {
    fn from(value: u8) -> Self {
        match value {
            0x01 => Self::Success,
            0x03 => Self::Continue,
            other => Self::Failed(other),
        }
    }
}

impl Display for ScheduleSetupStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Success => f.write_str("success"),
            Self::Continue => f.write_str("continue"),
            Self::Failed(status) => write!(f, "failed ({status:#04X})"),
        }
    }
}

/// Decoded status for the schedule master-switch response.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ScheduleMasterSwitchStatus {
    /// The device accepted the master-switch command.
    Success,
    /// The device rejected the master-switch command with a status byte.
    Failed(u8),
}

impl From<u8> for ScheduleMasterSwitchStatus {
    fn from(value: u8) -> Self {
        match value {
            0x01 => Self::Success,
            other => Self::Failed(other),
        }
    }
}

impl Display for ScheduleMasterSwitchStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Success => f.write_str("success"),
            Self::Failed(status) => write!(f, "failed ({status:#04X})"),
        }
    }
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
    /// Schedule setup response status.
    ScheduleSetup(ScheduleSetupStatus),
    /// Schedule master-switch response status.
    ScheduleMasterSwitch(ScheduleMasterSwitchStatus),
    /// Parsed `Get LED type` response.
    LedInfo(LedInfoResponse),
    /// Parsed screen-light readback timeout value.
    ScreenLightTimeout(u8),
    /// Unrecognised notification payload preserved as raw bytes.
    Unknown(Vec<u8>),
}

impl Display for NotifyEvent {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::NextPackage(family) => write!(f, "{family} next package"),
            Self::Finished(family) => write!(f, "{family} finished"),
            Self::Error(family, status) => write!(f, "{family} error ({status:#04X})"),
            Self::ScheduleSetup(status) => write!(f, "Schedule setup: {status}"),
            Self::ScheduleMasterSwitch(status) => {
                write!(f, "Schedule master switch: {status}")
            }
            Self::LedInfo(response) => write!(
                f,
                "LED info: screen_type={} mcu={}.{} status={:#04X} password={}",
                response.screen_type,
                response.mcu_major_version,
                response.mcu_minor_version,
                response.status,
                if response.password_enabled {
                    "on"
                } else {
                    "off"
                }
            ),
            Self::ScreenLightTimeout(value) => write!(f, "Screen-light timeout: {value}"),
            Self::Unknown(_unknown_payload) => f.write_str("Unknown event"),
        }
    }
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
                return Ok(NotifyEvent::ScheduleSetup(payload[4].into()));
            }
            if payload[2] == 0x07 && payload[3] == 0x80 {
                return Ok(NotifyEvent::ScheduleMasterSwitch(payload[4].into()));
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
    #[case(
        [0x05, 0x00, 0x05, 0x80, 0x01],
        NotifyEvent::ScheduleSetup(ScheduleSetupStatus::Success)
    )]
    #[case(
        [0x05, 0x00, 0x07, 0x80, 0x03],
        NotifyEvent::ScheduleMasterSwitch(ScheduleMasterSwitchStatus::Failed(0x03))
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
