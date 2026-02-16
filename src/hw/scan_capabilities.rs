use super::scan_model::ScanIdentity;

/// CID/PID lookup key for scan-derived device capabilities.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub(crate) struct CapabilityKey {
    cid: u8,
    pid: u8,
}

impl CapabilityKey {
    #[must_use]
    pub(crate) const fn new(cid: u8, pid: u8) -> Self {
        Self { cid, pid }
    }
}

/// Capability family derived from CID/PID model groups.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum CapabilityFamily {
    Fixed16x16,
    Fixed8x32,
    Fixed16x32,
    Fixed24x48,
    Fixed32x32,
    Fixed64x64,
    Ambiguous1Plus3,
    Ambiguous1Plus15,
}

impl CapabilityFamily {
    #[must_use]
    pub(crate) fn led_type(self) -> Option<u8> {
        match self {
            Self::Fixed16x16 => Some(1),
            Self::Fixed8x32 => Some(2),
            Self::Fixed16x32 => Some(7),
            Self::Fixed24x48 => Some(6),
            Self::Fixed32x32 => Some(3),
            Self::Fixed64x64 => Some(4),
            Self::Ambiguous1Plus3 | Self::Ambiguous1Plus15 => None,
        }
    }

    #[must_use]
    pub(crate) fn panel_size(self) -> Option<(u16, u16)> {
        match self {
            Self::Fixed16x16 => Some((16, 16)),
            Self::Fixed8x32 => Some((8, 32)),
            Self::Fixed16x32 => Some((16, 32)),
            Self::Fixed24x48 => Some((24, 48)),
            Self::Fixed32x32 => Some((32, 32)),
            Self::Fixed64x64 => Some((64, 64)),
            Self::Ambiguous1Plus3 | Self::Ambiguous1Plus15 => None,
        }
    }

    #[must_use]
    pub(crate) fn requires_led_selection(self) -> bool {
        matches!(self, Self::Ambiguous1Plus3 | Self::Ambiguous1Plus15)
    }
}

/// Lookup result for one known CID/PID model.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct ScanCapabilityProfile {
    key: CapabilityKey,
    family: CapabilityFamily,
}

impl ScanCapabilityProfile {
    #[must_use]
    pub(crate) const fn new(key: CapabilityKey, family: CapabilityFamily) -> Self {
        Self { key, family }
    }

    #[must_use]
    pub(crate) fn led_type(self) -> Option<u8> {
        self.family.led_type()
    }

    #[must_use]
    pub(crate) fn panel_size(self) -> Option<(u16, u16)> {
        self.family.panel_size()
    }

    #[must_use]
    pub(crate) fn requires_led_selection(self) -> bool {
        self.family.requires_led_selection()
    }
}

/// Typed CID/PID model capability lookup.
pub(crate) struct ScanCapabilityTable;

impl ScanCapabilityTable {
    #[must_use]
    pub(crate) fn lookup(identity: &ScanIdentity) -> Option<ScanCapabilityProfile> {
        let key = CapabilityKey::new(identity.cid, identity.pid);
        CAPABILITY_TABLE
            .iter()
            .copied()
            .find(|entry| entry.key == key)
    }
}

const CAPABILITY_TABLE: [ScanCapabilityProfile; 27] = [
    // 16x16
    ScanCapabilityProfile::new(CapabilityKey::new(1, 3), CapabilityFamily::Fixed16x16),
    ScanCapabilityProfile::new(CapabilityKey::new(1, 19), CapabilityFamily::Fixed16x16),
    ScanCapabilityProfile::new(CapabilityKey::new(2, 3), CapabilityFamily::Fixed16x16),
    ScanCapabilityProfile::new(CapabilityKey::new(4, 3), CapabilityFamily::Fixed16x16),
    ScanCapabilityProfile::new(CapabilityKey::new(5, 1), CapabilityFamily::Fixed16x16),
    ScanCapabilityProfile::new(CapabilityKey::new(5, 2), CapabilityFamily::Fixed16x16),
    ScanCapabilityProfile::new(CapabilityKey::new(6, 1), CapabilityFamily::Fixed16x16),
    // 32x32
    ScanCapabilityProfile::new(CapabilityKey::new(1, 4), CapabilityFamily::Fixed32x32),
    ScanCapabilityProfile::new(CapabilityKey::new(1, 20), CapabilityFamily::Fixed32x32),
    ScanCapabilityProfile::new(CapabilityKey::new(2, 4), CapabilityFamily::Fixed32x32),
    ScanCapabilityProfile::new(CapabilityKey::new(3, 2), CapabilityFamily::Fixed32x32),
    ScanCapabilityProfile::new(CapabilityKey::new(4, 4), CapabilityFamily::Fixed32x32),
    // 64x64
    ScanCapabilityProfile::new(CapabilityKey::new(1, 5), CapabilityFamily::Fixed64x64),
    ScanCapabilityProfile::new(CapabilityKey::new(4, 7), CapabilityFamily::Fixed64x64),
    // 8x32
    ScanCapabilityProfile::new(CapabilityKey::new(1, 6), CapabilityFamily::Fixed8x32),
    ScanCapabilityProfile::new(CapabilityKey::new(1, 25), CapabilityFamily::Fixed8x32),
    // 16x32
    ScanCapabilityProfile::new(CapabilityKey::new(1, 21), CapabilityFamily::Fixed16x32),
    // 24x48
    ScanCapabilityProfile::new(CapabilityKey::new(1, 22), CapabilityFamily::Fixed24x48),
    // 1+3 families (ambiguous)
    ScanCapabilityProfile::new(CapabilityKey::new(1, 1), CapabilityFamily::Ambiguous1Plus3),
    ScanCapabilityProfile::new(CapabilityKey::new(3, 1), CapabilityFamily::Ambiguous1Plus3),
    ScanCapabilityProfile::new(CapabilityKey::new(4, 1), CapabilityFamily::Ambiguous1Plus3),
    ScanCapabilityProfile::new(CapabilityKey::new(1, 7), CapabilityFamily::Ambiguous1Plus3),
    ScanCapabilityProfile::new(CapabilityKey::new(4, 5), CapabilityFamily::Ambiguous1Plus3),
    // 1+15 families (ambiguous)
    ScanCapabilityProfile::new(CapabilityKey::new(1, 2), CapabilityFamily::Ambiguous1Plus15),
    ScanCapabilityProfile::new(CapabilityKey::new(4, 2), CapabilityFamily::Ambiguous1Plus15),
    ScanCapabilityProfile::new(CapabilityKey::new(1, 8), CapabilityFamily::Ambiguous1Plus15),
    ScanCapabilityProfile::new(CapabilityKey::new(4, 6), CapabilityFamily::Ambiguous1Plus15),
];

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::*;

    fn identity(cid: u8, pid: u8) -> ScanIdentity {
        ScanIdentity {
            cid,
            pid,
            shape: 0,
            reverse: false,
            group_id: 1,
            device_id: 2,
            lamp_count: 0,
            lamp_num: 0,
        }
    }

    #[rstest]
    #[case(1, 5, Some(4), Some((64, 64)), false)]
    #[case(1, 22, Some(6), Some((24, 48)), false)]
    #[case(1, 1, None, None, true)]
    #[case(4, 6, None, None, true)]
    #[case(9, 9, None, None, false)]
    fn lookup_returns_expected_capability(
        #[case] cid: u8,
        #[case] pid: u8,
        #[case] expected_led_type: Option<u8>,
        #[case] expected_panel_size: Option<(u16, u16)>,
        #[case] expected_requires_selection: bool,
    ) {
        let capability = ScanCapabilityTable::lookup(&identity(cid, pid));

        assert_eq!(
            expected_led_type,
            capability.and_then(|entry| entry.led_type())
        );
        assert_eq!(
            expected_panel_size,
            capability.and_then(|entry| entry.panel_size())
        );
        assert_eq!(
            expected_requires_selection,
            capability.is_some_and(|entry| entry.requires_led_selection())
        );
    }
}
