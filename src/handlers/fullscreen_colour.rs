use crate::error::ProtocolError;
use crate::hw::{DeviceSession, WriteMode};
use crate::protocol::EndpointId;

use super::{FrameCodec, FrameCodecError};

/// RGB colour value.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct Rgb {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
}

impl Rgb {
    /// Creates an RGB colour.
    ///
    /// ```
    /// use idm::Rgb;
    ///
    /// let colour = Rgb::new(255, 127, 0);
    /// assert_eq!(255, colour.r);
    /// ```
    #[must_use]
    pub fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// Handler for full-screen colour fill commands.
pub struct FullscreenColourHandler;

impl FullscreenColourHandler {
    fn frame_for(colour: Rgb) -> Result<Vec<u8>, FrameCodecError> {
        FrameCodec::encode_short(0x02, 0x02, &[colour.r, colour.g, colour.b])
    }

    /// Fills the panel with a single colour.
    ///
    /// ```
    /// # async fn demo(session: idm::DeviceSession) -> Result<(), idm::ProtocolError> {
    /// use idm::{FullscreenColourHandler, Rgb};
    ///
    /// FullscreenColourHandler::set_colour(&session, Rgb::new(255, 0, 0)).await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error when frame encoding fails or the BLE write fails.
    pub async fn set_colour(session: &DeviceSession, colour: Rgb) -> Result<(), ProtocolError> {
        let frame = Self::frame_for(colour)?;
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
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn frame_for_colour_matches_protocol() {
        let frame = FullscreenColourHandler::frame_for(Rgb::new(0x11, 0x22, 0x33))
            .expect("colour command frame should encode cleanly");
        assert_eq!(vec![0x07, 0x00, 0x02, 0x02, 0x11, 0x22, 0x33], frame);
    }
}
