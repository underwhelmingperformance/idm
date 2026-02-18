use thiserror::Error;

use crate::hw::PanelDimensions;

/// Errors returned when validating an RGB888 framebuffer payload.
#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum Rgb888FrameError {
    /// The payload length does not match `width * height * 3`.
    #[error(
        "rgb888 payload length mismatch for panel {dimensions}: expected {expected_len} bytes, got {actual_len}"
    )]
    LengthMismatch {
        dimensions: PanelDimensions,
        expected_len: usize,
        actual_len: usize,
    },
    /// Panel dimensions cannot be represented as an in-memory RGB888 length.
    #[error("rgb888 payload length overflows platform usize for panel {dimensions}")]
    PayloadLengthOverflow { dimensions: PanelDimensions },
}

/// Validated RGB888 framebuffer payload for one panel-sized frame.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Rgb888Frame {
    dimensions: PanelDimensions,
    payload: Vec<u8>,
}

impl Rgb888Frame {
    /// Returns panel dimensions this frame was validated against.
    ///
    /// ```
    /// use idm::{PanelDimensions, Rgb888Frame};
    ///
    /// let dimensions = PanelDimensions::new(2, 1).expect("2x1 should be valid");
    /// let frame = Rgb888Frame::try_from((dimensions, vec![0x10, 0x20, 0x30, 0x40, 0x50, 0x60]))?;
    /// assert_eq!(dimensions, frame.dimensions());
    /// # Ok::<(), idm::Rgb888FrameError>(())
    /// ```
    #[must_use]
    pub fn dimensions(&self) -> PanelDimensions {
        self.dimensions
    }

    /// Returns the validated RGB888 payload bytes.
    ///
    /// ```
    /// use idm::{PanelDimensions, Rgb888Frame};
    ///
    /// let dimensions = PanelDimensions::new(1, 1).expect("1x1 should be valid");
    /// let frame = Rgb888Frame::try_from((dimensions, vec![0xAA, 0xBB, 0xCC]))?;
    /// assert_eq!(&[0xAA, 0xBB, 0xCC], frame.payload());
    /// # Ok::<(), idm::Rgb888FrameError>(())
    /// ```
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Consumes this frame and returns the payload bytes.
    ///
    /// ```
    /// use idm::{PanelDimensions, Rgb888Frame};
    ///
    /// let dimensions = PanelDimensions::new(1, 1).expect("1x1 should be valid");
    /// let frame = Rgb888Frame::try_from((dimensions, vec![0x01, 0x02, 0x03]))?;
    /// assert_eq!(vec![0x01, 0x02, 0x03], frame.into_payload());
    /// # Ok::<(), idm::Rgb888FrameError>(())
    /// ```
    #[must_use]
    pub fn into_payload(self) -> Vec<u8> {
        self.payload
    }

    /// Returns expected RGB888 byte length for one frame at the given dimensions.
    ///
    /// # Errors
    ///
    /// Returns an error if `width * height * 3` cannot fit in `usize` on this
    /// platform.
    ///
    /// ```
    /// use idm::{PanelDimensions, Rgb888Frame};
    ///
    /// let dimensions = PanelDimensions::new(8, 8).expect("8x8 should be valid");
    /// assert_eq!(192, Rgb888Frame::expected_payload_len(dimensions)?);
    /// # Ok::<(), idm::Rgb888FrameError>(())
    /// ```
    pub fn expected_payload_len(dimensions: PanelDimensions) -> Result<usize, Rgb888FrameError> {
        let pixels = usize::from(dimensions.width())
            .checked_mul(usize::from(dimensions.height()))
            .ok_or(Rgb888FrameError::PayloadLengthOverflow { dimensions })?;
        pixels
            .checked_mul(3)
            .ok_or(Rgb888FrameError::PayloadLengthOverflow { dimensions })
    }
}

impl TryFrom<(PanelDimensions, Vec<u8>)> for Rgb888Frame {
    type Error = Rgb888FrameError;

    fn try_from(value: (PanelDimensions, Vec<u8>)) -> Result<Self, Self::Error> {
        let (dimensions, payload) = value;
        let expected_len = Self::expected_payload_len(dimensions)?;
        let actual_len = payload.len();

        if actual_len != expected_len {
            return Err(Rgb888FrameError::LengthMismatch {
                dimensions,
                expected_len,
                actual_len,
            });
        }

        Ok(Self {
            dimensions,
            payload,
        })
    }
}

impl TryFrom<(PanelDimensions, &[u8])> for Rgb888Frame {
    type Error = Rgb888FrameError;

    fn try_from(value: (PanelDimensions, &[u8])) -> Result<Self, Self::Error> {
        let (dimensions, payload) = value;
        Self::try_from((dimensions, payload.to_vec()))
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::*;

    #[test]
    fn try_from_accepts_exact_panel_payload_len() {
        let dimensions = PanelDimensions::new(2, 2).expect("2x2 should be valid");
        let payload = vec![0x7F; 12];

        let frame = Rgb888Frame::try_from((dimensions, payload.clone()))
            .expect("exact payload length should construct");

        assert_eq!(dimensions, frame.dimensions());
        assert_eq!(payload, frame.into_payload());
    }

    #[rstest]
    #[case(0usize)]
    #[case(11usize)]
    #[case(13usize)]
    fn try_from_rejects_non_matching_len(#[case] payload_len: usize) {
        let dimensions = PanelDimensions::new(2, 2).expect("2x2 should be valid");
        let payload = vec![0x00; payload_len];

        let result = Rgb888Frame::try_from((dimensions, payload));

        assert_matches!(
            result,
            Err(Rgb888FrameError::LengthMismatch {
                dimensions: dims,
                expected_len: 12,
                actual_len,
            }) if dims == dimensions && actual_len == payload_len
        );
    }

    #[test]
    fn try_from_slice_validates_and_copies_payload() {
        let dimensions = PanelDimensions::new(1, 2).expect("1x2 should be valid");
        let payload = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06];

        let frame = Rgb888Frame::try_from((dimensions, payload.as_slice()))
            .expect("slice with expected length should construct");

        assert_eq!(payload.as_slice(), frame.payload());
    }
}
