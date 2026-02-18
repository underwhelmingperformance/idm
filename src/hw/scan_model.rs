use serde::Serialize;

/// Parsed device identity from manufacturer advertisement data.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
pub struct ScanIdentity {
    /// Vendor identifier byte.
    pub cid: u8,
    /// Product identifier byte.
    pub pid: u8,
    /// Shape/device-type byte interpreted as signed.
    pub shape: i8,
    /// Whether the panel orientation is reversed.
    pub reverse: bool,
    /// Group identifier byte.
    pub group_id: u8,
    /// Device identifier byte.
    pub device_id: u8,
    /// Reported lamp count.
    pub lamp_count: u16,
    /// Reported lamp number.
    pub lamp_num: u16,
}

/// Ambiguous model families that require a user-selected LED type.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AmbiguousShape {
    /// Shape byte `0x81`.
    Shape81,
    /// Shape byte `0x82`.
    Shape82,
    /// Shape byte `0x83`.
    Shape83,
}

/// Capability hints inferred from scan identity fields.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
pub struct ModelProfile {
    /// Provisional LED type, when the shape maps directly.
    pub led_type: Option<u8>,
    /// Provisional panel size, when known.
    pub panel_size: Option<(u16, u16)>,
    /// Ambiguous shape marker requiring explicit selection.
    pub ambiguous_shape: Option<AmbiguousShape>,
}

/// Parses scan payload identity fields and resolves provisional model hints.
pub struct ScanModelHandler;

impl ScanModelHandler {
    /// Parses identity fields from a raw BLE advertisement byte stream.
    ///
    /// The input may be a full AD-TLV stream or a direct manufacturer payload.
    ///
    /// ```
    /// let scan_data = [
    ///     0x0F, 0xFF, 0x54, 0x52, 0x00, 0x70, 0x04, 0x01, 0x02, 0x00, 0x01, 0x05, 0x20, 0x00,
    ///     0x20, 0x00,
    /// ];
    /// let identity = idm::ScanModelHandler::parse_identity(&scan_data);
    /// assert_eq!(Some(4), identity.map(|value| value.shape));
    /// ```
    #[must_use]
    pub fn parse_identity(scan_data: &[u8]) -> Option<ScanIdentity> {
        if let Some(identity) = Self::parse_identity_from_manufacturer_payload(scan_data) {
            return Some(identity);
        }

        let mut index = 0usize;
        while index < scan_data.len() {
            let record_len = usize::from(scan_data[index]);
            if record_len == 0 {
                index += 1;
                continue;
            }
            if record_len > 31 {
                return None;
            }

            let payload_start = index + 1;
            let payload_end = payload_start + record_len;
            if payload_end > scan_data.len() {
                return None;
            }

            let record_payload = &scan_data[payload_start..payload_end];
            if let Some(identity) = Self::parse_identity_from_manufacturer_payload(record_payload) {
                return Some(identity);
            }

            index = payload_end;
        }

        None
    }

    /// Parses identity from one manufacturer payload (`0xFF` AD record body).
    #[must_use]
    pub(crate) fn parse_identity_from_manufacturer_payload(payload: &[u8]) -> Option<ScanIdentity> {
        parse_with_ad_type(payload).or_else(|| parse_without_ad_type(payload))
    }

    /// Resolves a provisional model profile from parsed identity fields.
    ///
    /// ```
    /// let identity = idm::ScanIdentity {
    ///     cid: 1,
    ///     pid: 5,
    ///     shape: 4,
    ///     reverse: false,
    ///     group_id: 1,
    ///     device_id: 2,
    ///     lamp_count: 32,
    ///     lamp_num: 32,
    /// };
    /// let profile = idm::ScanModelHandler::resolve_model(&identity);
    /// assert_eq!(Some(4), profile.led_type);
    /// assert_eq!(Some((64, 64)), profile.panel_size);
    /// ```
    #[must_use]
    pub fn resolve_model(identity: &ScanIdentity) -> ModelProfile {
        match identity.shape {
            1 => model_profile(Some(1), Some((16, 16)), None),
            2 => model_profile(Some(2), Some((8, 32)), None),
            3 => model_profile(Some(3), Some((32, 32)), None),
            4 => model_profile(Some(4), Some((64, 64)), None),
            6 => model_profile(Some(6), Some((24, 48)), None),
            7 => model_profile(Some(7), Some((16, 32)), None),
            11 => model_profile(Some(11), Some((16, 64)), None),
            -127 => model_profile(None, None, Some(AmbiguousShape::Shape81)),
            -126 => model_profile(None, None, Some(AmbiguousShape::Shape82)),
            -125 => model_profile(None, None, Some(AmbiguousShape::Shape83)),
            _ => model_profile(None, None, None),
        }
    }
}

const AD_TYPE_MANUFACTURER_SPECIFIC: u8 = 0xFF;
const TR_SIGNATURE_P: [u8; 4] = [0x54, 0x52, 0x00, 0x70];
const TR_SIGNATURE_Q: [u8; 4] = [0x54, 0x52, 0x00, 0x71];

fn parse_with_ad_type(payload: &[u8]) -> Option<ScanIdentity> {
    if payload.len() < 11 {
        return None;
    }
    if payload[0] != AD_TYPE_MANUFACTURER_SPECIFIC {
        return None;
    }
    if !is_signature(&payload[1..5]) {
        return None;
    }

    Some(ScanIdentity {
        shape: payload[5] as i8,
        group_id: payload[6],
        device_id: payload[7],
        reverse: payload[8] != 0,
        cid: payload[9],
        pid: payload[10],
        lamp_count: payload
            .get(11..13)
            .map_or(0, |bytes| u16::from_le_bytes([bytes[0], bytes[1]])),
        lamp_num: payload
            .get(13..15)
            .map_or(0, |bytes| u16::from_le_bytes([bytes[0], bytes[1]])),
    })
}

fn parse_without_ad_type(payload: &[u8]) -> Option<ScanIdentity> {
    if payload.len() < 10 {
        return None;
    }
    if !is_signature(&payload[0..4]) {
        return None;
    }

    Some(ScanIdentity {
        shape: payload[4] as i8,
        group_id: payload[5],
        device_id: payload[6],
        reverse: payload[7] != 0,
        cid: payload[8],
        pid: payload[9],
        lamp_count: payload
            .get(10..12)
            .map_or(0, |bytes| u16::from_le_bytes([bytes[0], bytes[1]])),
        lamp_num: payload
            .get(12..14)
            .map_or(0, |bytes| u16::from_le_bytes([bytes[0], bytes[1]])),
    })
}

fn is_signature(signature: &[u8]) -> bool {
    signature == TR_SIGNATURE_P || signature == TR_SIGNATURE_Q
}

fn model_profile(
    led_type: Option<u8>,
    panel_size: Option<(u16, u16)>,
    ambiguous_shape: Option<AmbiguousShape>,
) -> ModelProfile {
    ModelProfile {
        led_type,
        panel_size,
        ambiguous_shape,
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case(
        &[
            0x0F, 0xFF, 0x54, 0x52, 0x00, 0x70, 0x04, 0x10, 0x11, 0x01, 0x02, 0x20, 0x00, 0x21,
            0x00, 0x22,
        ],
        Some(ScanIdentity {
            cid: 2,
            pid: 0x20,
            shape: 4,
            reverse: true,
            group_id: 0x10,
            device_id: 0x11,
            lamp_count: 0x2100,
            lamp_num: 0x2200,
        })
    )]
    #[case(
        &[
            0x54, 0x52, 0x00, 0x71, 0x03, 0x01, 0x02, 0x00, 0x01, 0x04, 0x20, 0x00, 0x30, 0x00,
        ],
        Some(ScanIdentity {
            cid: 1,
            pid: 4,
            shape: 3,
            reverse: false,
            group_id: 1,
            device_id: 2,
            lamp_count: 32,
            lamp_num: 48,
        })
    )]
    #[case(
        &[0x54, 0x52, 0x00, 0x70, 0x04, 0x05, 0x03, 0x00, 0x08, 0x01],
        Some(ScanIdentity {
            cid: 8,
            pid: 1,
            shape: 4,
            reverse: false,
            group_id: 5,
            device_id: 3,
            lamp_count: 0,
            lamp_num: 0,
        })
    )]
    #[case(
        &[0x0B, 0xFF, 0x54, 0x52, 0x00, 0x70, 0x04, 0x05, 0x03, 0x00, 0x08, 0x01],
        Some(ScanIdentity {
            cid: 8,
            pid: 1,
            shape: 4,
            reverse: false,
            group_id: 5,
            device_id: 3,
            lamp_count: 0,
            lamp_num: 0,
        })
    )]
    #[case(&[0x02, 0x01, 0x06], None)]
    fn parse_identity_returns_expected_values(
        #[case] scan_data: &[u8],
        #[case] expected: Option<ScanIdentity>,
    ) {
        let parsed = ScanModelHandler::parse_identity(scan_data);
        assert_eq!(expected, parsed);
    }

    #[rstest]
    #[case(1, Some(1), Some((16, 16)), None)]
    #[case(2, Some(2), Some((8, 32)), None)]
    #[case(3, Some(3), Some((32, 32)), None)]
    #[case(4, Some(4), Some((64, 64)), None)]
    #[case(6, Some(6), Some((24, 48)), None)]
    #[case(7, Some(7), Some((16, 32)), None)]
    #[case(11, Some(11), Some((16, 64)), None)]
    #[case(-127, None, None, Some(AmbiguousShape::Shape81))]
    #[case(-126, None, None, Some(AmbiguousShape::Shape82))]
    #[case(-125, None, None, Some(AmbiguousShape::Shape83))]
    #[case(42, None, None, None)]
    fn resolve_model_maps_shape_as_expected(
        #[case] shape: i8,
        #[case] expected_led_type: Option<u8>,
        #[case] expected_panel_size: Option<(u16, u16)>,
        #[case] expected_ambiguous: Option<AmbiguousShape>,
    ) {
        let identity = ScanIdentity {
            cid: 1,
            pid: 1,
            shape,
            reverse: false,
            group_id: 1,
            device_id: 2,
            lamp_count: 32,
            lamp_num: 32,
        };
        let profile = ScanModelHandler::resolve_model(&identity);

        assert_eq!(
            ModelProfile {
                led_type: expected_led_type,
                panel_size: expected_panel_size,
                ambiguous_shape: expected_ambiguous,
            },
            profile
        );
    }
}
