use derive_more::Display;
use serde::Serialize;
use serde_with::SerializeDisplay;

use super::scan_capabilities::ScanCapabilityTable;
use super::scan_model::{ScanIdentity, ScanModelHandler};

/// Typed text encoder path selected for a resolved LED type.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Display, SerializeDisplay)]
pub enum TextPath {
    /// `sendTextTo832`.
    #[display("path_8x32")]
    Path832,
    /// `sendTextTo1616` class (`16x16`, `24x48`, `16x32`).
    #[display("path_16x16")]
    Path1616,
    /// `sendTextTo3232`.
    #[display("path_32x32")]
    Path3232,
    /// `sendTextTo6464`.
    #[display("path_64x64")]
    Path6464,
    /// `sendTextTo1664`.
    #[display("path_16x64")]
    Path1664,
}

/// Device-routing decisions derived from scan identity and optional LED-info query data.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct DeviceRoutingProfile {
    /// Resolved LED type (screen type), when known.
    pub led_type: Option<u8>,
    /// Resolved panel size in pixels (`width`, `height`), when known.
    pub panel_size: Option<(u16, u16)>,
    /// Resolved text upload path, when known.
    pub text_path: Option<TextPath>,
    /// Canonical joint mode for ambiguous shapes, when required.
    pub joint_mode: Option<u8>,
}

/// Parsed `Get LED type` (`04 00 01 80`) response fields.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
pub struct LedInfoResponse {
    /// MCU major version byte.
    pub mcu_major_version: u8,
    /// MCU minor version byte.
    pub mcu_minor_version: u8,
    /// Device status byte.
    pub status: u8,
    /// Screen type / LED type byte.
    pub screen_type: u8,
    /// Password-enabled flag.
    pub password_enabled: bool,
}

impl LedInfoResponse {
    /// Parses one `Get LED type` response payload.
    ///
    /// ```
    /// let response = idm::LedInfoResponse::parse(&[0x09, 0x00, 0x01, 0x80, 0x02, 0x0A, 0x01, 0x04, 0x00]);
    /// assert_eq!(Some(4), response.map(|value| value.screen_type));
    /// ```
    #[must_use]
    pub fn parse(payload: &[u8]) -> Option<Self> {
        if payload.len() < 9 {
            return None;
        }
        if payload[2] != 0x01 || payload[3] != 0x80 {
            return None;
        }

        Some(Self {
            mcu_major_version: payload[4],
            mcu_minor_version: payload[5],
            status: payload[6],
            screen_type: payload[7],
            password_enabled: payload[8] != 0,
        })
    }
}

/// Resolves routing decisions from discovery identity and LED-info response data.
pub(crate) struct DeviceProfileResolver;

impl DeviceProfileResolver {
    /// Resolves the routing profile for one device.
    ///
    /// ```ignore
    /// let identity = idm::ScanIdentity {
    ///     cid: 1,
    ///     pid: 5,
    ///     shape: 4,
    ///     reverse: false,
    ///     group_id: 1,
    ///     device_id: 2,
    ///     lamp_count: 64,
    ///     lamp_num: 64,
    /// };
    /// let resolved = idm::DeviceProfileResolver::resolve(&identity, None);
    /// assert_eq!(Some(idm::TextPath::Path6464), resolved.text_path);
    /// ```
    #[must_use]
    pub(crate) fn resolve(
        identity: &ScanIdentity,
        led_info: Option<LedInfoResponse>,
    ) -> DeviceRoutingProfile {
        Self::resolve_with_selected_led_type(identity, led_info, None)
    }

    /// Resolves the routing profile with an explicit selected LED type hint.
    #[must_use]
    pub(crate) fn resolve_with_selected_led_type(
        identity: &ScanIdentity,
        led_info: Option<LedInfoResponse>,
        selected_led_type: Option<u8>,
    ) -> DeviceRoutingProfile {
        let provisional = ScanModelHandler::resolve_model(identity);
        let capability = ScanCapabilityTable::lookup(identity);
        let requires_led_selection = provisional.ambiguous_shape.is_some()
            || (provisional.led_type.is_none()
                && capability.is_some_and(|entry| entry.requires_led_selection()));
        let resolved_led_type = led_info
            .map(|response| response.screen_type)
            .filter(|value| is_known_led_type(*value))
            .or(selected_led_type.filter(|value| is_known_led_type(*value)))
            .or(provisional.led_type)
            .or_else(|| capability.and_then(|entry| entry.led_type()));

        let panel_size = resolved_led_type
            .and_then(panel_size_for_led_type)
            .or_else(|| capability.and_then(|entry| entry.panel_size()));
        let text_path = resolved_led_type.and_then(text_path_for_led_type);
        let joint_mode = if requires_led_selection {
            resolved_led_type.and_then(joint_mode_for_led_type)
        } else {
            None
        };

        DeviceRoutingProfile {
            led_type: resolved_led_type,
            panel_size,
            text_path,
            joint_mode,
        }
    }

    #[must_use]
    pub(crate) fn resolve_without_scan_identity(
        led_info: Option<LedInfoResponse>,
        selected_led_type: Option<u8>,
    ) -> Option<DeviceRoutingProfile> {
        let resolved_led_type = led_info
            .map(|response| response.screen_type)
            .filter(|value| is_known_led_type(*value))
            .or(selected_led_type.filter(|value| is_known_led_type(*value)))?;

        Some(DeviceRoutingProfile {
            led_type: Some(resolved_led_type),
            panel_size: panel_size_for_led_type(resolved_led_type),
            text_path: text_path_for_led_type(resolved_led_type),
            joint_mode: None,
        })
    }

    #[must_use]
    pub(crate) fn requires_led_type_selection(identity: &ScanIdentity) -> bool {
        let provisional = ScanModelHandler::resolve_model(identity);
        if provisional.ambiguous_shape.is_some() {
            return true;
        }

        if provisional.led_type.is_some() {
            return false;
        }

        ScanCapabilityTable::lookup(identity).is_some_and(|entry| entry.requires_led_selection())
    }
}

fn is_known_led_type(led_type: u8) -> bool {
    matches!(led_type, 1 | 2 | 3 | 4 | 6 | 7 | 11)
}

fn panel_size_for_led_type(led_type: u8) -> Option<(u16, u16)> {
    match led_type {
        1 => Some((16, 16)),
        2 => Some((8, 32)),
        3 => Some((32, 32)),
        4 => Some((64, 64)),
        6 => Some((24, 48)),
        7 => Some((16, 32)),
        11 => Some((16, 64)),
        _ => None,
    }
}

fn text_path_for_led_type(led_type: u8) -> Option<TextPath> {
    match led_type {
        2 => Some(TextPath::Path832),
        1 | 6 | 7 => Some(TextPath::Path1616),
        3 => Some(TextPath::Path3232),
        4 => Some(TextPath::Path6464),
        11 => Some(TextPath::Path1664),
        _ => None,
    }
}

fn joint_mode_for_led_type(led_type: u8) -> Option<u8> {
    match led_type {
        1 => Some(1),
        2 => Some(2),
        3 => Some(5),
        11 => Some(6),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::*;

    fn identity(shape: i8) -> ScanIdentity {
        ScanIdentity {
            cid: 1,
            pid: 1,
            shape,
            reverse: false,
            group_id: 1,
            device_id: 2,
            lamp_count: 64,
            lamp_num: 64,
        }
    }

    fn led_info(screen_type: u8) -> LedInfoResponse {
        LedInfoResponse {
            mcu_major_version: 1,
            mcu_minor_version: 0,
            status: 0,
            screen_type,
            password_enabled: false,
        }
    }

    #[rstest]
    #[case(1, None, DeviceRoutingProfile { led_type: Some(1), panel_size: Some((16, 16)), text_path: Some(TextPath::Path1616), joint_mode: None })]
    #[case(2, None, DeviceRoutingProfile { led_type: Some(2), panel_size: Some((8, 32)), text_path: Some(TextPath::Path832), joint_mode: None })]
    #[case(3, None, DeviceRoutingProfile { led_type: Some(3), panel_size: Some((32, 32)), text_path: Some(TextPath::Path3232), joint_mode: None })]
    #[case(4, None, DeviceRoutingProfile { led_type: Some(4), panel_size: Some((64, 64)), text_path: Some(TextPath::Path6464), joint_mode: None })]
    #[case(6, None, DeviceRoutingProfile { led_type: Some(6), panel_size: Some((24, 48)), text_path: Some(TextPath::Path1616), joint_mode: None })]
    #[case(7, None, DeviceRoutingProfile { led_type: Some(7), panel_size: Some((16, 32)), text_path: Some(TextPath::Path1616), joint_mode: None })]
    #[case(11, None, DeviceRoutingProfile { led_type: Some(11), panel_size: Some((16, 64)), text_path: Some(TextPath::Path1664), joint_mode: None })]
    #[case(-127, None, DeviceRoutingProfile { led_type: None, panel_size: None, text_path: None, joint_mode: None })]
    #[case(-126, None, DeviceRoutingProfile { led_type: None, panel_size: None, text_path: None, joint_mode: None })]
    #[case(-125, None, DeviceRoutingProfile { led_type: None, panel_size: None, text_path: None, joint_mode: None })]
    #[case(-127, Some(1), DeviceRoutingProfile { led_type: Some(1), panel_size: Some((16, 16)), text_path: Some(TextPath::Path1616), joint_mode: Some(1) })]
    #[case(-127, Some(2), DeviceRoutingProfile { led_type: Some(2), panel_size: Some((8, 32)), text_path: Some(TextPath::Path832), joint_mode: Some(2) })]
    #[case(-125, Some(3), DeviceRoutingProfile { led_type: Some(3), panel_size: Some((32, 32)), text_path: Some(TextPath::Path3232), joint_mode: Some(5) })]
    #[case(-125, Some(11), DeviceRoutingProfile { led_type: Some(11), panel_size: Some((16, 64)), text_path: Some(TextPath::Path1664), joint_mode: Some(6) })]
    #[case(4, Some(3), DeviceRoutingProfile { led_type: Some(3), panel_size: Some((32, 32)), text_path: Some(TextPath::Path3232), joint_mode: None })]
    #[case(4, Some(99), DeviceRoutingProfile { led_type: Some(4), panel_size: Some((64, 64)), text_path: Some(TextPath::Path6464), joint_mode: None })]
    fn resolve_maps_expected_routing_profile(
        #[case] shape: i8,
        #[case] led_type_override: Option<u8>,
        #[case] expected: DeviceRoutingProfile,
    ) {
        let resolved =
            DeviceProfileResolver::resolve(&identity(shape), led_type_override.map(led_info));

        assert_eq!(expected, resolved);
    }

    #[rstest]
    #[case(
        &[0x09, 0x00, 0x01, 0x80, 0x02, 0x0A, 0x01, 0x04, 0x00],
        Some(LedInfoResponse {
            mcu_major_version: 0x02,
            mcu_minor_version: 0x0A,
            status: 0x01,
            screen_type: 0x04,
            password_enabled: false,
        }),
    )]
    #[case(&[0x08, 0x00, 0x01, 0x80, 0x02, 0x0A, 0x01, 0x04], None)]
    #[case(&[0x09, 0x00, 0xAA, 0xBB, 0x02, 0x0A, 0x01, 0x04, 0x00], None)]
    fn led_info_parse_validates_payload_shape(
        #[case] payload: &[u8],
        #[case] expected: Option<LedInfoResponse>,
    ) {
        let parsed = LedInfoResponse::parse(payload);
        assert_eq!(expected, parsed);
    }

    #[test]
    fn resolve_with_selected_led_type_uses_selected_type_when_led_query_is_missing() {
        let resolved =
            DeviceProfileResolver::resolve_with_selected_led_type(&identity(-127), None, Some(2));

        assert_eq!(
            DeviceRoutingProfile {
                led_type: Some(2),
                panel_size: Some((8, 32)),
                text_path: Some(TextPath::Path832),
                joint_mode: Some(2),
            },
            resolved
        );
    }

    #[rstest]
    #[case(Some(4), None, Some(DeviceRoutingProfile {
        led_type: Some(4),
        panel_size: Some((64, 64)),
        text_path: Some(TextPath::Path6464),
        joint_mode: None,
    }))]
    #[case(None, Some(11), Some(DeviceRoutingProfile {
        led_type: Some(11),
        panel_size: Some((16, 64)),
        text_path: Some(TextPath::Path1664),
        joint_mode: None,
    }))]
    #[case(Some(99), None, None)]
    #[case(None, None, None)]
    fn resolve_without_scan_identity_uses_led_hints_only(
        #[case] led_info_type: Option<u8>,
        #[case] selected_type: Option<u8>,
        #[case] expected: Option<DeviceRoutingProfile>,
    ) {
        let led_info = led_info_type.map(led_info);
        let resolved =
            DeviceProfileResolver::resolve_without_scan_identity(led_info, selected_type);
        assert_eq!(expected, resolved);
    }

    #[test]
    fn resolve_falls_back_to_cid_pid_capability_when_shape_is_unknown() {
        let resolved = DeviceProfileResolver::resolve(
            &ScanIdentity {
                cid: 1,
                pid: 5,
                shape: 42,
                reverse: false,
                group_id: 1,
                device_id: 2,
                lamp_count: 0,
                lamp_num: 0,
            },
            None,
        );

        assert_eq!(
            DeviceRoutingProfile {
                led_type: Some(4),
                panel_size: Some((64, 64)),
                text_path: Some(TextPath::Path6464),
                joint_mode: None,
            },
            resolved
        );
    }

    #[test]
    fn requires_led_type_selection_recognises_ambiguous_cid_pid_families() {
        let identity = ScanIdentity {
            cid: 1,
            pid: 1,
            shape: 42,
            reverse: false,
            group_id: 1,
            device_id: 2,
            lamp_count: 0,
            lamp_num: 0,
        };

        assert_eq!(
            true,
            DeviceProfileResolver::requires_led_type_selection(&identity)
        );
    }
}
