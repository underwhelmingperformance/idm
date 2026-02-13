/// Formats bytes as uppercase hexadecimal pairs separated by spaces.
pub(crate) fn format_hex(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "<empty>".to_string();
    }

    let mut rendered = String::with_capacity(bytes.len().saturating_mul(3));
    for (index, value) in bytes.iter().enumerate() {
        if index > 0 {
            rendered.push(' ');
        }
        let high = value >> 4;
        let low = value & 0x0F;
        rendered.push(nibble_to_hex(high));
        rendered.push(nibble_to_hex(low));
    }
    rendered
}

/// Formats an optional RSSI for terminal output.
pub(crate) fn format_rssi(rssi: Option<i16>) -> String {
    match rssi {
        Some(value) => value.to_string(),
        None => "-".to_string(),
    }
}

fn nibble_to_hex(value: u8) -> char {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    HEX[value as usize] as char
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn format_hex_handles_empty_payload() {
        assert_eq!("<empty>", format_hex(&[]));
    }

    #[test]
    fn format_hex_formats_uppercase_pairs() {
        assert_eq!("05 00 A1 FF", format_hex(&[0x05, 0x00, 0xA1, 0xFF]));
    }

    #[test]
    fn format_rssi_handles_unknown() {
        assert_eq!("-", format_rssi(None));
    }
}
