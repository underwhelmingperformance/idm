use time::OffsetDateTime;
use tracing::instrument;

use crate::error::ProtocolError;
use crate::hw::{DeviceSession, WriteMode};
use crate::protocol::EndpointId;

use super::{FrameCodec, FrameCodecError};

/// Handler for device time synchronisation.
pub struct TimeSyncHandler;

impl TimeSyncHandler {
    fn payload_for(timestamp: OffsetDateTime) -> [u8; 7] {
        let year = u8::try_from(timestamp.year().rem_euclid(100))
            .expect("year modulo 100 should always fit in u8");
        let month = timestamp.month() as u8;
        let day = timestamp.day();
        let weekday = timestamp.weekday().number_from_monday();
        let hour = timestamp.hour();
        let minute = timestamp.minute();
        let second = timestamp.second();

        [year, month, day, weekday, hour, minute, second]
    }

    fn frame_for(timestamp: OffsetDateTime) -> Result<Vec<u8>, FrameCodecError> {
        FrameCodec::encode_short(0x01, 0x80, &Self::payload_for(timestamp))
    }

    /// Sends a time synchronisation frame.
    ///
    /// ```
    /// # async fn demo(session: idm::DeviceSession) -> Result<(), idm::ProtocolError> {
    /// use idm::TimeSyncHandler;
    /// use time::OffsetDateTime;
    ///
    /// TimeSyncHandler::sync_time(&session, OffsetDateTime::now_utc()).await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error when frame encoding fails or the BLE write fails.
    #[instrument(
        skip(session),
        level = "debug",
        fields(unix_timestamp = timestamp.unix_timestamp())
    )]
    pub async fn sync_time(
        session: &DeviceSession,
        timestamp: OffsetDateTime,
    ) -> Result<(), ProtocolError> {
        let frame = Self::frame_for(timestamp)?;
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
    use time::{Date, Month, PrimitiveDateTime, Time, UtcOffset};

    use super::*;

    fn timestamp_utc(
        year: i32,
        month: Month,
        day: u8,
        hour: u8,
        minute: u8,
        second: u8,
    ) -> OffsetDateTime {
        let date = Date::from_calendar_date(year, month, day)
            .expect("calendar date used in tests should be valid");
        let time =
            Time::from_hms(hour, minute, second).expect("time used in tests should be valid");
        PrimitiveDateTime::new(date, time).assume_offset(UtcOffset::UTC)
    }

    #[test]
    fn payload_for_maps_timestamp_fields() {
        let timestamp = timestamp_utc(2026, Month::February, 15, 21, 4, 5);
        let payload = TimeSyncHandler::payload_for(timestamp);
        assert_eq!([26, 2, 15, 7, 21, 4, 5], payload);
    }

    #[test]
    fn frame_for_matches_protocol_example_shape() {
        let timestamp = timestamp_utc(2026, Month::February, 16, 9, 30, 45);
        let frame =
            TimeSyncHandler::frame_for(timestamp).expect("time sync frame should encode cleanly");
        assert_eq!(
            vec![
                0x0B, 0x00, 0x01, 0x80, 0x1A, 0x02, 0x10, 0x01, 0x09, 0x1E, 0x2D
            ],
            frame
        );
    }
}
