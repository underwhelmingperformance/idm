use std::fmt::{self, Display, Formatter};

use crate::utils::{format_hex, format_rssi};

/// Formats a boolean as `yes` / `no`.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct YesNo(pub(crate) bool);

impl Display for YesNo {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if self.0 {
            f.write_str("yes")
        } else {
            f.write_str("no")
        }
    }
}

/// Formats a byte count as `<n> bytes`.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct Bytes(pub(crate) usize);

impl Display for Bytes {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{} bytes", self.0)
    }
}

/// Formats optional values as `<unknown>` when absent.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct UnknownOr<T>(pub(crate) Option<T>);

impl<T: Display> Display for UnknownOr<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Some(value) => write!(f, "{value}"),
            None => f.write_str("<unknown>"),
        }
    }
}

/// Formats optional values as `<none>` when absent.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct NoneOr<T>(pub(crate) Option<T>);

impl<T: Display> Display for NoneOr<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Some(value) => write!(f, "{value}"),
            None => f.write_str("<none>"),
        }
    }
}

/// Formats optional values as `<missing>` when absent.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct MissingOr<T>(pub(crate) Option<T>);

impl<T: Display> Display for MissingOr<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Some(value) => write!(f, "{value}"),
            None => f.write_str("<missing>"),
        }
    }
}

/// Formats an RSSI reading using the CLI's canonical formatter.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct Rssi(pub(crate) Option<i16>);

impl Display for Rssi {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", format_rssi(self.0))
    }
}

/// Formats bytes as uppercase hexadecimal pairs.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct HexBytes(pub(crate) Vec<u8>);

impl Display for HexBytes {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", format_hex(&self.0))
    }
}

/// Joins string parts with a separator.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct JoinedStrings {
    parts: Vec<String>,
    separator: &'static str,
}

impl JoinedStrings {
    pub(crate) fn comma(parts: Vec<String>) -> Self {
        Self {
            parts,
            separator: ",",
        }
    }

    pub(crate) fn semicolon(parts: Vec<String>) -> Self {
        Self {
            parts,
            separator: ";",
        }
    }

    pub(crate) fn into_option(self) -> Option<Self> {
        if self.parts.is_empty() {
            None
        } else {
            Some(self)
        }
    }
}

impl Display for JoinedStrings {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.parts.join(self.separator))
    }
}
