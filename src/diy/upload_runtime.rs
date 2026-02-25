use std::time::Duration;

use idm_macros::progress;
use strum_macros::Display;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use super::session::{UploadRequest, UploadStats};
use crate::error::ProtocolError;
use crate::handlers::upload_common::drain_stale_notifications;
use crate::hw::{
    Ack, DeviceSession, NotificationSubscription, PanelDimensions, SessionWriter, WriteMode,
};
use crate::protocol::EndpointId;
use crate::{DiyPrefixFields, FrameCodec, GifChunkFlag, TransferFamily};
const DIY_GUARD_DROP_EXIT_TIMEOUT: Duration = Duration::from_millis(250);
const DIY_GUARD_DROP_WAIT_TIMEOUT: Duration = Duration::from_millis(300);

/// Errors returned by DIY upload operations.
#[derive(Debug, Error)]
pub enum DiyError {
    #[error("diy upload requires known panel dimensions from the active device profile")]
    MissingPanelDimensions,
    #[error(
        "diy upload frame dimensions {frame_dimensions} do not match device panel dimensions {device_dimensions}"
    )]
    PanelDimensionsMismatch {
        frame_dimensions: PanelDimensions,
        device_dimensions: PanelDimensions,
    },
    #[error("diy upload payload is too large: {payload_len} bytes exceeds max {max_payload_len}")]
    PayloadTooLarge {
        payload_len: usize,
        max_payload_len: usize,
    },
    #[error("diy upload chunk size cannot be zero")]
    InvalidChunkSize,
    #[error("diy point list cannot be empty")]
    EmptyPointList,
    #[error("diy point ({x}, {y}) is outside device panel dimensions {panel_dimensions}")]
    PointOutOfBounds {
        x: u8,
        y: u8,
        panel_dimensions: PanelDimensions,
    },
    #[error("diy movement command requires at least one direction")]
    EmptyMovementDirection,
}

#[derive(Debug, Display, Clone, Copy, Eq, PartialEq)]
enum DiyMode {
    Enter,
    Exit,
}

#[derive(Debug, Display, Clone, Copy, Eq, PartialEq)]
pub(crate) enum RuntimeMode {
    NoEffect,
    HorizontalMirror,
    VerticalMirror,
    OverallMovement,
    Erase,
}

/// Guard for one active DIY mode scope.
///
/// Enter mode with [`DiyModeGuard::enter`], then let this guard drop to
/// trigger best-effort exit. For deterministic shutdown, call
/// [`DiyModeGuard::exit`].
pub(crate) struct DiyModeGuard {
    session: DeviceSession,
    active: bool,
}

/// Reusable active-DIY uploader state for multi-frame animations.
struct DiyActiveUploader {
    session: DeviceSession,
    stream: NotificationSubscription,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum GuardDropExitResult {
    Exited,
    TimedOut,
    ThreadWaitTimedOut,
    SpawnFailed,
    RuntimeBuildFailed,
    ExitFailed,
}

impl DiyModeGuard {
    /// Enters DIY mode and returns a guard that owns the mode lease.
    ///
    /// # Errors
    ///
    /// Returns an error when the mode-enter command write fails.
    pub async fn enter(session: &DeviceSession) -> Result<Self, ProtocolError> {
        DiyHandler::set_diy_mode(session, DiyMode::Enter).await?;
        Ok(Self {
            session: session.clone(),
            active: true,
        })
    }

    /// Exits DIY mode immediately and consumes this guard.
    ///
    /// # Errors
    ///
    /// Returns an error when the mode-exit command write fails.
    pub async fn exit(mut self) -> Result<(), ProtocolError> {
        if self.active {
            DiyHandler::set_diy_mode(&self.session, DiyMode::Exit).await?;
            self.active = false;
        }
        Ok(())
    }

    fn exit_blocking_with_timeout(
        session: DeviceSession,
        timeout: Duration,
    ) -> GuardDropExitResult {
        let (result_tx, result_rx) = std::sync::mpsc::channel::<GuardDropExitResult>();
        let spawn_result = std::thread::Builder::new()
            .name("idm-diy-mode-exit".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_time()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(_error) => {
                        let _ = result_tx.send(GuardDropExitResult::RuntimeBuildFailed);
                        return;
                    }
                };

                let exit_result = runtime.block_on(async move {
                    match tokio::time::timeout(
                        timeout,
                        DiyHandler::set_diy_mode(&session, DiyMode::Exit),
                    )
                    .await
                    {
                        Ok(Ok(())) => GuardDropExitResult::Exited,
                        Ok(Err(_error)) => GuardDropExitResult::ExitFailed,
                        Err(_elapsed) => GuardDropExitResult::TimedOut,
                    }
                });
                let _ = result_tx.send(exit_result);
            });

        if spawn_result.is_err() {
            return GuardDropExitResult::SpawnFailed;
        }

        match result_rx.recv_timeout(DIY_GUARD_DROP_WAIT_TIMEOUT) {
            Ok(result) => result,
            Err(_timeout_or_disconnected) => GuardDropExitResult::ThreadWaitTimedOut,
        }
    }
}

impl Drop for DiyModeGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }

        let exit_result =
            Self::exit_blocking_with_timeout(self.session.clone(), DIY_GUARD_DROP_EXIT_TIMEOUT);
        if exit_result != GuardDropExitResult::Exited {
            tracing::trace!(?exit_result, "failed to exit DIY mode from drop guard");
        }
    }
}

impl DiyMode {
    fn as_payload_byte(self) -> u8 {
        match self {
            Self::Enter => 0x01,
            Self::Exit => 0x02,
        }
    }
}

impl RuntimeMode {
    fn as_payload_byte(self) -> u8 {
        match self {
            Self::NoEffect => 0x00,
            Self::HorizontalMirror => 0x01,
            Self::VerticalMirror => 0x02,
            Self::OverallMovement => 0x03,
            Self::Erase => 0x04,
        }
    }
}

/// Uploads raw RGB888 framebuffer payloads via the DIY transfer family.
pub(crate) struct DiyHandler;

impl DiyHandler {
    fn frame_for_mode(mode: DiyMode) -> Result<Vec<u8>, ProtocolError> {
        Ok(FrameCodec::encode_short(
            0x04,
            0x01,
            &[mode.as_payload_byte()],
        )?)
    }

    #[tracing::instrument(
        skip(session),
        level = "trace",
        fields(
            %mode,
            command_id = 0x04_u8,
            namespace = 0x01_u8,
            frame_len = tracing::field::Empty
        )
    )]
    async fn set_diy_mode(session: &DeviceSession, mode: DiyMode) -> Result<(), ProtocolError> {
        let span = tracing::Span::current();
        let frame = Self::frame_for_mode(mode)?;
        span.record("frame_len", frame.len());
        SessionWriter::builder()
            .session(session)
            .payload(&frame)
            .ack(Ack::Transport)
            .build()
            .send()
            .await?;
        Ok(())
    }

    fn frame_for_runtime(mode: RuntimeMode, payload: &[u8]) -> Result<Vec<u8>, ProtocolError> {
        let mut command_payload = Vec::with_capacity(payload.len() + 1);
        command_payload.push(mode.as_payload_byte());
        command_payload.extend_from_slice(payload);
        Ok(FrameCodec::encode_short(0x05, 0x01, &command_payload)?)
    }

    /// Sends a runtime command, fragmenting into transport-sized chunks
    /// with inter-fragment pacing.
    #[tracing::instrument(
        skip(session),
        level = "trace",
        fields(
            %mode,
            command_id = 0x05_u8,
            namespace = 0x01_u8,
            payload_len = payload.len(),
            frame_len = tracing::field::Empty
        )
    )]
    pub(crate) async fn send_runtime_command(
        session: &DeviceSession,
        mode: RuntimeMode,
        payload: &[u8],
    ) -> Result<usize, ProtocolError> {
        let span = tracing::Span::current();
        let frame = Self::frame_for_runtime(mode, payload)?;
        span.record("frame_len", frame.len());
        let stats = SessionWriter::builder()
            .session(session)
            .payload(&frame)
            .ack(Ack::Transport)
            .build()
            .send()
            .await?;
        Ok(stats.bytes_written)
    }

    async fn begin_active_uploader(
        session: &DeviceSession,
    ) -> Result<DiyActiveUploader, ProtocolError> {
        let endpoint = EndpointId::ReadNotifyCharacteristic;
        let mut stream = session
            .notification_stream(endpoint, None, CancellationToken::new())
            .await?;
        drain_stale_notifications(&mut stream, "diy").await?;
        Ok(DiyActiveUploader {
            session: session.clone(),
            stream,
        })
    }

    /// Uploads one DIY RGB framebuffer, entering and exiting DIY mode around
    /// the transfer.
    ///
    /// # Errors
    ///
    /// Returns an error when payload validation, frame encoding, BLE writes, or
    /// acknowledgement handling fails.
    #[progress(
        message = "Uploading DIY payload",
        finished = match result {
            Ok(stats) => format!(
                "✓ Uploaded DIY payload: {} bytes in {} chunk(s) across {} logical chunk(s)",
                stats.bytes_written(),
                stats.chunks_written(),
                stats.logical_chunks_sent(),
            ),
            Err(_error) => "✗ DIY upload failed".to_string(),
        },
        skip_all,
        level = "info"
    )]
    pub async fn upload(
        session: &DeviceSession,
        request: UploadRequest,
    ) -> Result<UploadStats, ProtocolError> {
        let mode_guard = DiyModeGuard::enter(session).await?;
        let mut uploader = Self::begin_active_uploader(session).await?;
        let stats = uploader.upload_request(&request).await?;
        mode_guard.exit().await?;
        Ok(stats)
    }
}

impl DiyActiveUploader {
    async fn upload_request(
        &mut self,
        request: &UploadRequest,
    ) -> Result<UploadStats, ProtocolError> {
        let session = &self.session;
        let device_dimensions = session
            .device_profile()
            .panel_dimensions()
            .ok_or(DiyError::MissingPanelDimensions)?;
        let frame_dimensions = request.frame().dimensions();
        if frame_dimensions != device_dimensions {
            return Err(DiyError::PanelDimensionsMismatch {
                frame_dimensions,
                device_dimensions,
            }
            .into());
        }

        let payload = request.payload();

        let encoder = |chunk: &[u8], index: usize, total_len: u32, _crc: u32| {
            let flag = if index == 0 {
                GifChunkFlag::First
            } else {
                GifChunkFlag::Continuation
            };
            let chunk_len =
                u16::try_from(chunk.len()).map_err(|_overflow| DiyError::PayloadTooLarge {
                    payload_len: chunk.len(),
                    max_payload_len: u16::MAX as usize,
                })?;
            let fields = DiyPrefixFields::new(chunk_len, flag, total_len)?;
            Ok(FrameCodec::encode_diy_prefix(fields).to_vec())
        };

        let stats = SessionWriter::builder()
            .session(session)
            .payload(payload)
            .ack(Ack::Transfer(TransferFamily::Diy))
            .write_mode(WriteMode::WithResponse)
            .header(&encoder)
            .stream(&mut self.stream)
            .build()
            .send()
            .await?;

        Ok(UploadStats {
            bytes_written: stats.bytes_written,
            chunks_written: stats.chunks_written,
            logical_chunks_sent: stats.logical_chunks_sent,
        })
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::*;
    use crate::Rgb888Frame;
    use crate::hw::PanelDimensions;

    #[test]
    fn diy_upload_request_payload_matches_frame() -> Result<(), crate::Rgb888FrameError> {
        let dimensions = PanelDimensions::new(1, 1).expect("1x1 should be valid");
        let frame = Rgb888Frame::try_from((dimensions, vec![0x01, 0x02, 0x03]))?;
        let request = UploadRequest::new(frame);

        assert_eq!(&[0x01, 0x02, 0x03], request.payload());
        Ok(())
    }

    #[test]
    fn diy_upload_stats_accessors_return_field_values() {
        let stats = UploadStats {
            bytes_written: 4105,
            chunks_written: 9,
            logical_chunks_sent: 1,
        };

        assert_eq!(4105, stats.bytes_written());
        assert_eq!(9, stats.chunks_written());
        assert_eq!(1, stats.logical_chunks_sent());
    }

    #[rstest]
    #[case(DiyMode::Enter, vec![0x05, 0x00, 0x04, 0x01, 0x01])]
    #[case(DiyMode::Exit, vec![0x05, 0x00, 0x04, 0x01, 0x02])]
    fn frame_for_mode_matches_protocol(#[case] mode: DiyMode, #[case] expected: Vec<u8>) {
        let frame = DiyHandler::frame_for_mode(mode).expect("diy mode frame should encode cleanly");
        assert_eq!(expected, frame);
    }

    #[rstest]
    #[case(
        RuntimeMode::NoEffect,
        vec![0xAA, 0xBB, 0xCC, 0x01, 0x02],
        vec![0x0A, 0x00, 0x05, 0x01, 0x00, 0xAA, 0xBB, 0xCC, 0x01, 0x02]
    )]
    #[case(
        RuntimeMode::OverallMovement,
        vec![0x01, 0x00, 0x00, 0x00],
        vec![0x09, 0x00, 0x05, 0x01, 0x03, 0x01, 0x00, 0x00, 0x00]
    )]
    fn frame_for_runtime_matches_protocol(
        #[case] mode: RuntimeMode,
        #[case] payload: Vec<u8>,
        #[case] expected: Vec<u8>,
    ) {
        let frame = DiyHandler::frame_for_runtime(mode, &payload)
            .expect("runtime frame should encode cleanly");
        assert_eq!(expected, frame);
    }

    #[test]
    fn diy_exit_mode_preserves_current_display() {
        // From the iDotMatrix APK (BleProtocolN.java), DIY mode exit has
        // multiple modes:
        //   0 = QUIT_NOSAVE_KEEP_PREV  — revert to previous display
        //   2 = QUIT_STILL_CUR_SHOW    — keep current canvas visible
        //
        // Runtime drawing (rainbow, graffiti) paints on a temporary canvas.
        // Exiting with mode 0 discards that canvas. Mode 2 preserves it so
        // the user sees the last painted frame after the command finishes.
        let frame = DiyHandler::frame_for_mode(DiyMode::Exit).expect("exit frame should encode");
        assert_eq!(
            vec![0x05, 0x00, 0x04, 0x01, 0x02],
            frame,
            "DIY exit should use mode 2 (QUIT_STILL_CUR_SHOW) to preserve runtime drawing"
        );
    }

    #[test]
    fn set_pixels_frame_encodes_multi_point_payload() {
        use crate::Rgb;
        use crate::diy::Point;

        let mode = RuntimeMode::NoEffect;
        let colour = Rgb::new(0xFF, 0x00, 0x80);
        let points = [Point::new(0, 0), Point::new(15, 15)];

        let mut input = vec![colour.r, colour.g, colour.b];
        for p in &points {
            input.extend_from_slice(&[p.x(), p.y()]);
        }
        let frame = DiyHandler::frame_for_runtime(mode, &input).expect("frame should encode");

        #[rustfmt::skip]
        let expected = FrameCodec::encode_short(0x05, 0x01, &[
            mode.as_payload_byte(),
            colour.r, colour.g, colour.b,
            points[0].x(), points[0].y(),
            points[1].x(), points[1].y(),
        ]).expect("expected should encode");
        assert_eq!(expected, frame);
    }

    #[test]
    fn shift_left_frame_encodes_correct_direction_flags() {
        use crate::diy::Shift;

        let mode = RuntimeMode::OverallMovement;
        let shift = Shift::left();

        let payload = [
            u8::from(shift.is_up()),
            u8::from(shift.is_down()),
            u8::from(shift.is_left()),
            u8::from(shift.is_right()),
        ];
        let frame = DiyHandler::frame_for_runtime(mode, &payload).expect("frame should encode");

        #[rustfmt::skip]
        let expected = FrameCodec::encode_short(0x05, 0x01, &[
            mode.as_payload_byte(),
            u8::from(shift.is_up()),
            u8::from(shift.is_down()),
            u8::from(shift.is_left()),
            u8::from(shift.is_right()),
        ]).expect("expected should encode");
        assert_eq!(expected, frame);
    }

    #[test]
    fn set_pixels_frame_for_full_column_of_16_pixel_panel() {
        use crate::Rgb;
        use crate::diy::Point;

        let mode = RuntimeMode::NoEffect;
        let colour = Rgb::new(0xFF, 0x00, 0x00);
        let points: [Point; 16] = std::array::from_fn(|y| Point::new(0, y as u8));

        let mut input = vec![colour.r, colour.g, colour.b];
        for p in &points {
            input.extend_from_slice(&[p.x(), p.y()]);
        }
        let frame = DiyHandler::frame_for_runtime(mode, &input).expect("frame should encode");

        #[rustfmt::skip]
        let expected = FrameCodec::encode_short(0x05, 0x01, &[
            mode.as_payload_byte(),
            colour.r, colour.g, colour.b,
            points[ 0].x(), points[ 0].y(),
            points[ 1].x(), points[ 1].y(),
            points[ 2].x(), points[ 2].y(),
            points[ 3].x(), points[ 3].y(),
            points[ 4].x(), points[ 4].y(),
            points[ 5].x(), points[ 5].y(),
            points[ 6].x(), points[ 6].y(),
            points[ 7].x(), points[ 7].y(),
            points[ 8].x(), points[ 8].y(),
            points[ 9].x(), points[ 9].y(),
            points[10].x(), points[10].y(),
            points[11].x(), points[11].y(),
            points[12].x(), points[12].y(),
            points[13].x(), points[13].y(),
            points[14].x(), points[14].y(),
            points[15].x(), points[15].y(),
        ]).expect("expected should encode");
        assert_eq!(expected, frame);
    }
}
