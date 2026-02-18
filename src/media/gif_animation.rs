use std::io::Cursor;

use thiserror::Error;

use crate::hw::PanelDimensions;

/// Errors returned when validating GIF upload payloads.
#[derive(Debug, Error)]
pub enum GifAnimationError {
    /// The payload is empty.
    #[error("gif payload cannot be empty")]
    EmptyPayload,
    /// The payload cannot be decoded as a GIF stream.
    #[error("invalid gif payload")]
    InvalidGif { source: gif::DecodingError },
    /// GIF dimensions are invalid for panel representation.
    #[error("gif payload has invalid logical dimensions: {width}x{height}")]
    InvalidDimensions { width: u16, height: u16 },
}

/// Validated GIF payload with parsed logical dimensions.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct GifAnimation {
    dimensions: PanelDimensions,
    payload: Vec<u8>,
}

impl GifAnimation {
    /// Returns the logical GIF dimensions parsed from the payload.
    ///
    /// ```
    /// use idm::GifAnimation;
    ///
    /// let bytes = vec![
    ///     0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00,
    ///     0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0x21, 0xF9, 0x04, 0x01, 0x00, 0x00, 0x00,
    ///     0x00, 0x2C, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02,
    ///     0x44, 0x01, 0x00, 0x3B,
    /// ];
    /// let gif = GifAnimation::try_from(bytes)?;
    /// assert_eq!("1x1", gif.dimensions().to_string());
    /// # Ok::<(), idm::GifAnimationError>(())
    /// ```
    #[must_use]
    pub fn dimensions(&self) -> PanelDimensions {
        self.dimensions
    }

    /// Returns the validated GIF bytes.
    ///
    /// ```
    /// use idm::GifAnimation;
    ///
    /// let bytes = vec![
    ///     0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00,
    ///     0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0x21, 0xF9, 0x04, 0x01, 0x00, 0x00, 0x00,
    ///     0x00, 0x2C, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02,
    ///     0x44, 0x01, 0x00, 0x3B,
    /// ];
    /// let gif = GifAnimation::try_from(bytes.clone())?;
    /// assert_eq!(bytes.as_slice(), gif.payload());
    /// # Ok::<(), idm::GifAnimationError>(())
    /// ```
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Consumes the payload and returns GIF bytes.
    ///
    /// ```
    /// use idm::GifAnimation;
    ///
    /// let bytes = vec![
    ///     0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00,
    ///     0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0x21, 0xF9, 0x04, 0x01, 0x00, 0x00, 0x00,
    ///     0x00, 0x2C, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02,
    ///     0x44, 0x01, 0x00, 0x3B,
    /// ];
    /// let gif = GifAnimation::try_from(bytes.clone())?;
    /// assert_eq!(bytes, gif.into_payload());
    /// # Ok::<(), idm::GifAnimationError>(())
    /// ```
    #[must_use]
    pub fn into_payload(self) -> Vec<u8> {
        self.payload
    }

    fn parse_dimensions(payload: &[u8]) -> Result<PanelDimensions, GifAnimationError> {
        let mut options = gif::DecodeOptions::new();
        options.check_frame_consistency(true);
        let reader = options
            .read_info(Cursor::new(payload))
            .map_err(|source| GifAnimationError::InvalidGif { source })?;
        let width = reader.width();
        let height = reader.height();
        PanelDimensions::new(width, height)
            .ok_or(GifAnimationError::InvalidDimensions { width, height })
    }
}

impl TryFrom<Vec<u8>> for GifAnimation {
    type Error = GifAnimationError;

    fn try_from(payload: Vec<u8>) -> Result<Self, Self::Error> {
        if payload.is_empty() {
            return Err(GifAnimationError::EmptyPayload);
        }
        let dimensions = Self::parse_dimensions(&payload)?;
        Ok(Self {
            dimensions,
            payload,
        })
    }
}

impl TryFrom<&[u8]> for GifAnimation {
    type Error = GifAnimationError;

    fn try_from(payload: &[u8]) -> Result<Self, Self::Error> {
        Self::try_from(payload.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use pretty_assertions::assert_eq;

    use super::*;

    const MINIMAL_GIF_1X1: [u8; 43] = [
        0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00,
        0x00, 0xFF, 0xFF, 0xFF, 0x21, 0xF9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, 0x2C, 0x00, 0x00,
        0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44, 0x01, 0x00, 0x3B,
    ];

    #[test]
    fn try_from_parses_gif_dimensions() -> Result<(), GifAnimationError> {
        let gif = GifAnimation::try_from(MINIMAL_GIF_1X1.to_vec())?;
        assert_eq!(
            PanelDimensions::new(1, 1).expect("1x1 should be valid"),
            gif.dimensions()
        );
        Ok(())
    }

    #[test]
    fn try_from_rejects_empty_payload() {
        let result = GifAnimation::try_from(Vec::new());
        assert_matches!(result, Err(GifAnimationError::EmptyPayload));
    }

    #[test]
    fn try_from_rejects_invalid_bytes() {
        let result = GifAnimation::try_from(vec![0x47, 0x49, 0x46]);
        assert_matches!(result, Err(GifAnimationError::InvalidGif { .. }));
    }
}
