use crate::error::ProtocolError;
use crate::hw::{DeviceSession, WriteMode};
use crate::protocol::EndpointId;

use super::{FrameCodec, FrameCodecError};

/// Screen power state.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ScreenPower {
    /// Turn the panel off.
    Off,
    /// Turn the panel on.
    On,
}

impl ScreenPower {
    fn as_payload_byte(self) -> u8 {
        match self {
            Self::Off => 0x00,
            Self::On => 0x01,
        }
    }
}

/// Handler for screen power commands.
pub struct PowerHandler;

impl PowerHandler {
    fn frame_for(power: ScreenPower) -> Result<Vec<u8>, FrameCodecError> {
        FrameCodec::encode_short(0x07, 0x01, &[power.as_payload_byte()])
    }

    /// Sends a screen power command.
    ///
    /// ```
    /// # async fn demo(session: idm::DeviceSession) -> Result<(), idm::ProtocolError> {
    /// use idm::{PowerHandler, ScreenPower};
    ///
    /// PowerHandler::set_power(&session, ScreenPower::On).await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error when frame encoding fails or the BLE write fails.
    pub async fn set_power(
        session: &DeviceSession,
        power: ScreenPower,
    ) -> Result<(), ProtocolError> {
        let frame = Self::frame_for(power)?;
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
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case(ScreenPower::Off, vec![0x05, 0x00, 0x07, 0x01, 0x00])]
    #[case(ScreenPower::On, vec![0x05, 0x00, 0x07, 0x01, 0x01])]
    fn frame_for_power_matches_protocol(#[case] power: ScreenPower, #[case] expected: Vec<u8>) {
        let frame =
            PowerHandler::frame_for(power).expect("power command frame should encode cleanly");
        assert_eq!(expected, frame);
    }
}
