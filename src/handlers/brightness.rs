use thiserror::Error;

use crate::error::ProtocolError;
use crate::hw::{DeviceSession, WriteMode};
use crate::protocol::EndpointId;

use super::{FrameCodec, FrameCodecError};

const MIN_BRIGHTNESS: u8 = 0;
const MAX_BRIGHTNESS: u8 = 100;

/// Errors returned by brightness validation.
#[derive(Debug, Error, Clone, Copy, Eq, PartialEq)]
pub enum BrightnessError {
    /// The brightness byte was outside the accepted range.
    #[error("brightness {value} is out of range ({min}..={max})")]
    OutOfRange { value: u8, min: u8, max: u8 },
}

/// Validated brightness value in the inclusive range `0..=100`.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct Brightness(u8);

impl Brightness {
    /// Creates a validated brightness value.
    ///
    /// # Errors
    ///
    /// Returns an error when `value` is outside `0..=100`.
    ///
    /// ```
    /// use idm::Brightness;
    ///
    /// let value = Brightness::new(42)?;
    /// assert_eq!(42, value.value());
    /// # Ok::<(), idm::BrightnessError>(())
    /// ```
    pub fn new(value: u8) -> Result<Self, BrightnessError> {
        if !(MIN_BRIGHTNESS..=MAX_BRIGHTNESS).contains(&value) {
            return Err(BrightnessError::OutOfRange {
                value,
                min: MIN_BRIGHTNESS,
                max: MAX_BRIGHTNESS,
            });
        }

        Ok(Self(value))
    }

    /// Returns the underlying brightness byte.
    ///
    /// ```
    /// use idm::Brightness;
    ///
    /// let value = Brightness::new(12)?;
    /// assert_eq!(12, value.value());
    /// # Ok::<(), idm::BrightnessError>(())
    /// ```
    #[must_use]
    pub fn value(self) -> u8 {
        self.0
    }
}

/// Handler for brightness commands.
pub struct BrightnessHandler;

impl BrightnessHandler {
    fn frame_for(brightness: Brightness) -> Result<Vec<u8>, FrameCodecError> {
        FrameCodec::encode_short(0x04, 0x80, &[brightness.value()])
    }

    /// Sends a brightness command.
    ///
    /// ```
    /// # async fn demo(session: idm::DeviceSession) -> Result<(), idm::ProtocolError> {
    /// use idm::{Brightness, BrightnessHandler};
    ///
    /// let brightness = Brightness::new(60)?;
    /// BrightnessHandler::set_brightness(&session, brightness).await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error when frame encoding fails or the BLE write fails.
    pub async fn set_brightness(
        session: &DeviceSession,
        brightness: Brightness,
    ) -> Result<(), ProtocolError> {
        let frame = Self::frame_for(brightness)?;
        session
            .write_endpoint(
                EndpointId::WriteCharacteristic,
                &frame,
                WriteMode::WithoutResponse,
            )
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case(0)]
    #[case(50)]
    #[case(100)]
    fn brightness_accepts_range(#[case] value: u8) {
        let brightness = Brightness::new(value).expect("valid brightness should construct");
        assert_eq!(value, brightness.value());
    }

    #[rstest]
    #[case(101)]
    #[case(255)]
    fn brightness_rejects_out_of_range(#[case] value: u8) {
        let result = Brightness::new(value);
        assert_matches!(
            result,
            Err(BrightnessError::OutOfRange {
                value: rejected,
                min: MIN_BRIGHTNESS,
                max: MAX_BRIGHTNESS,
            }) if rejected == value
        );
    }

    #[test]
    fn frame_for_brightness_matches_protocol() {
        let brightness = Brightness::new(80).expect("test brightness should be valid");
        let frame = BrightnessHandler::frame_for(brightness)
            .expect("brightness command frame should encode cleanly");
        assert_eq!(vec![0x05, 0x00, 0x04, 0x80, 0x50], frame);
    }
}
