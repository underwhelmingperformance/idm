use std::time::Duration;

use crc32fast::hash;
use thiserror::Error;
use tokio::time::{sleep, timeout};
use tracing::instrument;

use crate::error::ProtocolError;
use crate::hw::{DeviceSession, GifHeaderProfile, PanelDimensions, WriteMode};
use crate::protocol::EndpointId;
use crate::{
    FrameCodec, GifAnimation, GifChunkFlag, GifHeaderFields, NotificationDecodeError,
    NotificationHandler, NotifyEvent, TransferFamily,
};

const LOGICAL_CHUNK_SIZE: usize = 4096;
const DEFAULT_PER_FRAGMENT_DELAY: Duration = Duration::from_millis(20);
const DEFAULT_NOTIFY_ACK_TIMEOUT: Duration = Duration::from_secs(5);
const DRAIN_NOTIFICATION_TIMEOUT: Duration = Duration::from_millis(25);
const MAX_STALE_NOTIFICATION_DRAIN: usize = 8;
const UNUSABLE_WRITE_WITHOUT_RESPONSE_LIMIT: usize = 20;

/// Errors returned by GIF upload operations.
#[derive(Debug, Error)]
pub enum GifUploadError {
    #[error("gif upload payload is too large: {payload_len} bytes exceeds max {max_payload_len}")]
    PayloadTooLarge {
        payload_len: usize,
        max_payload_len: usize,
    },
    #[error(
        "gif upload dimensions {gif_dimensions} do not match device panel dimensions {device_dimensions}"
    )]
    PanelDimensionsMismatch {
        gif_dimensions: PanelDimensions,
        device_dimensions: PanelDimensions,
    },
    #[error(
        "gif logical chunk payload is too large: {chunk_payload_len} bytes exceeds max {max_payload_len}"
    )]
    ChunkPayloadTooLarge {
        chunk_payload_len: usize,
        max_payload_len: usize,
    },
    #[error("gif upload chunk size cannot be zero")]
    InvalidChunkSize,
    #[error("notification acknowledgement timed out after {timeout_ms}ms")]
    NotifyAckTimeout { timeout_ms: u64 },
    #[error("notification stream ended before a GIF acknowledgement was received")]
    MissingNotifyAck,
    #[error("received unexpected notification while waiting for a GIF acknowledgement")]
    UnexpectedNotifyEvent,
    #[error("gif transfer was rejected by device status 0x{status:02X}")]
    TransferRejected { status: u8 },
    #[error(
        "device reported transfer completion too early at chunk {chunk_index} of {total_chunks}"
    )]
    PrematureFinish {
        chunk_index: usize,
        total_chunks: usize,
    },
    #[error(transparent)]
    NotifyDecode(#[from] NotificationDecodeError),
}

/// GIF upload request parameters.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct GifUploadRequest {
    gif: GifAnimation,
    per_fragment_delay: Duration,
    ack_timeout: Duration,
}

impl GifUploadRequest {
    /// Creates a GIF upload request using default pacing.
    ///
    /// ```
    /// use idm::{GifAnimation, GifUploadRequest};
    ///
    /// # fn tiny_gif() -> Vec<u8> {
    /// #     vec![
    /// #         0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00,
    /// #         0x00, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0x21, 0xF9, 0x04, 0x01, 0x00,
    /// #         0x00, 0x00, 0x00, 0x2C, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00,
    /// #         0x00, 0x02, 0x02, 0x44, 0x01, 0x00, 0x3B,
    /// #     ]
    /// # }
    /// let gif = GifAnimation::try_from(tiny_gif()).expect("test gif should decode");
    /// let request = GifUploadRequest::new(gif);
    /// assert_eq!(43, request.payload().len());
    /// ```
    #[must_use]
    pub fn new(gif: GifAnimation) -> Self {
        Self {
            gif,
            per_fragment_delay: DEFAULT_PER_FRAGMENT_DELAY,
            ack_timeout: DEFAULT_NOTIFY_ACK_TIMEOUT,
        }
    }

    /// Overrides delay between transport fragments.
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use idm::{GifAnimation, GifUploadRequest};
    ///
    /// # fn tiny_gif() -> Vec<u8> {
    /// #     vec![
    /// #         0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00,
    /// #         0x00, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0x21, 0xF9, 0x04, 0x01, 0x00,
    /// #         0x00, 0x00, 0x00, 0x2C, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00,
    /// #         0x00, 0x02, 0x02, 0x44, 0x01, 0x00, 0x3B,
    /// #     ]
    /// # }
    /// let gif = GifAnimation::try_from(tiny_gif()).expect("test gif should decode");
    /// let request = GifUploadRequest::new(gif).with_per_fragment_delay(Duration::ZERO);
    /// assert_eq!(Duration::ZERO, request.per_fragment_delay());
    /// ```
    #[must_use]
    pub fn with_per_fragment_delay(mut self, per_fragment_delay: Duration) -> Self {
        self.per_fragment_delay = per_fragment_delay;
        self
    }

    /// Overrides acknowledgement timeout per logical chunk.
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use idm::{GifAnimation, GifUploadRequest};
    ///
    /// # fn tiny_gif() -> Vec<u8> {
    /// #     vec![
    /// #         0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00,
    /// #         0x00, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0x21, 0xF9, 0x04, 0x01, 0x00,
    /// #         0x00, 0x00, 0x00, 0x2C, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00,
    /// #         0x00, 0x02, 0x02, 0x44, 0x01, 0x00, 0x3B,
    /// #     ]
    /// # }
    /// let gif = GifAnimation::try_from(tiny_gif()).expect("test gif should decode");
    /// let request = GifUploadRequest::new(gif).with_ack_timeout(Duration::from_millis(250));
    /// assert_eq!(Duration::from_millis(250), request.ack_timeout());
    /// ```
    #[must_use]
    pub fn with_ack_timeout(mut self, ack_timeout: Duration) -> Self {
        self.ack_timeout = ack_timeout;
        self
    }

    /// Returns the raw GIF payload bytes.
    ///
    /// ```
    /// use idm::{GifAnimation, GifUploadRequest};
    ///
    /// # fn tiny_gif() -> Vec<u8> {
    /// #     vec![
    /// #         0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00,
    /// #         0x00, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0x21, 0xF9, 0x04, 0x01, 0x00,
    /// #         0x00, 0x00, 0x00, 0x2C, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00,
    /// #         0x00, 0x02, 0x02, 0x44, 0x01, 0x00, 0x3B,
    /// #     ]
    /// # }
    /// let gif = GifAnimation::try_from(tiny_gif()).expect("test gif should decode");
    /// let request = GifUploadRequest::new(gif);
    /// assert_eq!(43, request.payload().len());
    /// ```
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        self.gif.payload()
    }

    /// Returns the validated GIF payload and metadata.
    ///
    /// ```
    /// use idm::{GifAnimation, GifUploadRequest};
    ///
    /// # fn tiny_gif() -> Vec<u8> {
    /// #     vec![
    /// #         0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00,
    /// #         0x00, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0x21, 0xF9, 0x04, 0x01, 0x00,
    /// #         0x00, 0x00, 0x00, 0x2C, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00,
    /// #         0x00, 0x02, 0x02, 0x44, 0x01, 0x00, 0x3B,
    /// #     ]
    /// # }
    /// let gif = GifAnimation::try_from(tiny_gif()).expect("test gif should decode");
    /// let request = GifUploadRequest::new(gif.clone());
    /// assert_eq!(gif, request.gif().clone());
    /// ```
    #[must_use]
    pub fn gif(&self) -> &GifAnimation {
        &self.gif
    }

    /// Returns the configured transport-fragment delay.
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use idm::{GifAnimation, GifUploadRequest};
    ///
    /// # fn tiny_gif() -> Vec<u8> {
    /// #     vec![
    /// #         0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00,
    /// #         0x00, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0x21, 0xF9, 0x04, 0x01, 0x00,
    /// #         0x00, 0x00, 0x00, 0x2C, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00,
    /// #         0x00, 0x02, 0x02, 0x44, 0x01, 0x00, 0x3B,
    /// #     ]
    /// # }
    /// let gif = GifAnimation::try_from(tiny_gif()).expect("test gif should decode");
    /// let request = GifUploadRequest::new(gif);
    /// assert_eq!(Duration::from_millis(20), request.per_fragment_delay());
    /// ```
    #[must_use]
    pub fn per_fragment_delay(&self) -> Duration {
        self.per_fragment_delay
    }

    /// Returns the configured chunk-acknowledgement timeout.
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use idm::{GifAnimation, GifUploadRequest};
    ///
    /// # fn tiny_gif() -> Vec<u8> {
    /// #     vec![
    /// #         0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00,
    /// #         0x00, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0x21, 0xF9, 0x04, 0x01, 0x00,
    /// #         0x00, 0x00, 0x00, 0x2C, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00,
    /// #         0x00, 0x02, 0x02, 0x44, 0x01, 0x00, 0x3B,
    /// #     ]
    /// # }
    /// let gif = GifAnimation::try_from(tiny_gif()).expect("test gif should decode");
    /// let request = GifUploadRequest::new(gif);
    /// assert_eq!(Duration::from_secs(5), request.ack_timeout());
    /// ```
    #[must_use]
    pub fn ack_timeout(&self) -> Duration {
        self.ack_timeout
    }
}

/// GIF upload metadata returned on success.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct GifUploadReceipt {
    bytes_written: usize,
    chunks_written: usize,
    logical_chunks_sent: usize,
    cached: bool,
}

impl GifUploadReceipt {
    /// Creates a GIF upload receipt.
    ///
    /// ```
    /// use idm::GifUploadReceipt;
    ///
    /// let receipt = GifUploadReceipt::new(4112, 9, 1, true);
    /// assert_eq!(4112, receipt.bytes_written());
    /// assert!(receipt.cached());
    /// ```
    #[must_use]
    pub fn new(
        bytes_written: usize,
        chunks_written: usize,
        logical_chunks_sent: usize,
        cached: bool,
    ) -> Self {
        Self {
            bytes_written,
            chunks_written,
            logical_chunks_sent,
            cached,
        }
    }

    /// Returns total bytes written to `fa02`.
    ///
    /// ```
    /// use idm::GifUploadReceipt;
    ///
    /// let receipt = GifUploadReceipt::new(100, 1, 1, false);
    /// assert_eq!(100, receipt.bytes_written());
    /// ```
    #[must_use]
    pub fn bytes_written(&self) -> usize {
        self.bytes_written
    }

    /// Returns number of transport chunks written.
    ///
    /// ```
    /// use idm::GifUploadReceipt;
    ///
    /// let receipt = GifUploadReceipt::new(100, 2, 1, false);
    /// assert_eq!(2, receipt.chunks_written());
    /// ```
    #[must_use]
    pub fn chunks_written(&self) -> usize {
        self.chunks_written
    }

    /// Returns number of logical 4K chunks attempted.
    ///
    /// ```
    /// use idm::GifUploadReceipt;
    ///
    /// let receipt = GifUploadReceipt::new(100, 2, 1, false);
    /// assert_eq!(1, receipt.logical_chunks_sent());
    /// ```
    #[must_use]
    pub fn logical_chunks_sent(&self) -> usize {
        self.logical_chunks_sent
    }

    /// Returns whether the upload completed via device cache hit.
    ///
    /// ```
    /// use idm::GifUploadReceipt;
    ///
    /// let receipt = GifUploadReceipt::new(100, 2, 1, true);
    /// assert!(receipt.cached());
    /// ```
    #[must_use]
    pub fn cached(&self) -> bool {
        self.cached
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum GifAckOutcome {
    Continue,
    Finished,
}

/// Uploads animated GIF payloads to iDotMatrix devices.
pub struct GifUploadHandler;

impl GifUploadHandler {
    /// Uploads one GIF payload to the active session.
    ///
    /// ```
    /// # async fn demo(session: idm::DeviceSession) -> Result<(), idm::ProtocolError> {
    /// use idm::{GifAnimation, GifUploadHandler, GifUploadRequest};
    ///
    /// # fn tiny_gif() -> Vec<u8> {
    /// #     vec![
    /// #         0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00,
    /// #         0x00, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0x21, 0xF9, 0x04, 0x01, 0x00,
    /// #         0x00, 0x00, 0x00, 0x2C, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00,
    /// #         0x00, 0x02, 0x02, 0x44, 0x01, 0x00, 0x3B,
    /// #     ]
    /// # }
    /// let gif = GifAnimation::try_from(tiny_gif()).expect("test gif should decode");
    /// let request = GifUploadRequest::new(gif);
    /// let _receipt = GifUploadHandler::upload(&session, request).await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error when payload validation, frame encoding, BLE writes, or
    /// acknowledgement handling fails.
    #[instrument(skip_all, level = "debug")]
    pub async fn upload(
        session: &DeviceSession,
        request: GifUploadRequest,
    ) -> Result<GifUploadReceipt, ProtocolError> {
        if let Some(device_dimensions) = session.device_profile().panel_dimensions() {
            let gif_dimensions = request.gif().dimensions();
            if gif_dimensions != device_dimensions {
                return Err(GifUploadError::PanelDimensionsMismatch {
                    gif_dimensions,
                    device_dimensions,
                }
                .into());
            }
        }

        let payload = request.payload();
        let chunk_size = write_chunk_size(session)?;
        let logical_chunks_total = payload.chunks(LOGICAL_CHUNK_SIZE).len();
        let crc32 = hash(payload);
        let payload_len_u32 =
            u32::try_from(payload.len()).map_err(|_overflow| GifUploadError::PayloadTooLarge {
                payload_len: payload.len(),
                max_payload_len: u32::MAX as usize,
            })?;
        let endpoint = EndpointId::ReadNotifyCharacteristic;

        session.subscribe_endpoint(endpoint).await?;

        let upload_result = async {
            drain_stale_notifications(session, endpoint).await?;

            let mut bytes_written = 0usize;
            let mut chunks_written = 0usize;
            let mut logical_chunks_sent = 0usize;
            let mut cached = false;

            for (index, logical_chunk) in payload.chunks(LOGICAL_CHUNK_SIZE).enumerate() {
                let chunk_flag = if index == 0 {
                    GifChunkFlag::First
                } else {
                    GifChunkFlag::Continuation
                };
                let chunk_payload_len =
                    u16::try_from(logical_chunk.len()).map_err(|_overflow| {
                        GifUploadError::ChunkPayloadTooLarge {
                            chunk_payload_len: logical_chunk.len(),
                            max_payload_len: u16::MAX as usize,
                        }
                    })?;
                let fields =
                    GifHeaderFields::new(chunk_payload_len, chunk_flag, payload_len_u32, crc32)?;
                let mut header = FrameCodec::encode_gif_header(fields);
                apply_gif_header_profile(
                    &mut header,
                    session.device_profile().gif_header_profile(),
                );

                let mut frame_block = Vec::with_capacity(header.len() + logical_chunk.len());
                frame_block.extend_from_slice(&header);
                frame_block.extend_from_slice(logical_chunk);
                logical_chunks_sent += 1;

                for transport_chunk in frame_block.chunks(chunk_size) {
                    session
                        .write_endpoint(
                            EndpointId::WriteCharacteristic,
                            transport_chunk,
                            WriteMode::WithoutResponse,
                        )
                        .await?;
                    bytes_written += transport_chunk.len();
                    chunks_written += 1;
                    apply_fragment_delay(request.per_fragment_delay).await;
                }

                let ack_outcome = wait_for_gif_ack(session, request.ack_timeout).await?;
                if matches!(ack_outcome, GifAckOutcome::Finished) {
                    let chunk_number = index + 1;
                    if chunk_number < logical_chunks_total {
                        if chunk_number == 1 {
                            cached = true;
                            break;
                        }
                        return Err(GifUploadError::PrematureFinish {
                            chunk_index: chunk_number,
                            total_chunks: logical_chunks_total,
                        }
                        .into());
                    }
                    break;
                }
            }

            Ok(GifUploadReceipt::new(
                bytes_written,
                chunks_written,
                logical_chunks_sent,
                cached,
            ))
        }
        .await;

        match session.unsubscribe_endpoint(endpoint).await {
            Ok(()) => {}
            Err(error) => {
                if upload_result.is_ok() {
                    return Err(error.into());
                }
                tracing::trace!(
                    ?error,
                    "failed to unsubscribe gif-upload notifications cleanly"
                );
            }
        }

        upload_result
    }
}

fn write_chunk_size(session: &DeviceSession) -> Result<usize, ProtocolError> {
    let fallback = session.device_profile().write_without_response_fallback();
    let chunk_size = match session.write_without_response_limit() {
        Some(limit) if limit > UNUSABLE_WRITE_WITHOUT_RESPONSE_LIMIT => limit,
        _ => fallback,
    };
    if chunk_size == 0 {
        return Err(GifUploadError::InvalidChunkSize.into());
    }
    Ok(chunk_size)
}

async fn apply_fragment_delay(delay: Duration) {
    if !delay.is_zero() {
        sleep(delay).await;
    }
}

#[instrument(skip(session), level = "trace")]
async fn drain_stale_notifications(
    session: &DeviceSession,
    endpoint: EndpointId,
) -> Result<(), ProtocolError> {
    let mut drained_count = 0usize;
    for _attempt in 0..MAX_STALE_NOTIFICATION_DRAIN {
        let mut observed_notification = false;
        let drain_result = timeout(
            DRAIN_NOTIFICATION_TIMEOUT,
            session.run_notifications(endpoint, Some(1), |_index, _payload| {
                observed_notification = true;
            }),
        )
        .await;

        match drain_result {
            Err(_elapsed) => break,
            Ok(Err(error)) => return Err(error.into()),
            Ok(Ok(_summary)) => {
                if !observed_notification {
                    break;
                }
                drained_count += 1;
            }
        }
    }

    if drained_count > 0 {
        tracing::trace!(
            drained_notifications = drained_count,
            "drained stale notifications before gif upload"
        );
    }

    Ok(())
}

#[instrument(skip(session), level = "trace", fields(timeout_ms = timeout_duration.as_millis()))]
async fn wait_for_gif_ack(
    session: &DeviceSession,
    timeout_duration: Duration,
) -> Result<GifAckOutcome, ProtocolError> {
    let mut decoded_event: Option<Result<NotifyEvent, NotificationDecodeError>> = None;
    let wait_result = timeout(
        timeout_duration,
        session.run_notifications(
            EndpointId::ReadNotifyCharacteristic,
            Some(1),
            |_index, payload| {
                decoded_event = Some(NotificationHandler::decode(payload));
            },
        ),
    )
    .await;

    match wait_result {
        Err(_elapsed) => {
            let timeout_ms = u64::try_from(timeout_duration.as_millis()).unwrap_or(u64::MAX);
            Err(GifUploadError::NotifyAckTimeout { timeout_ms }.into())
        }
        Ok(Err(error)) => Err(error.into()),
        Ok(Ok(_summary)) => {
            let Some(event_result) = decoded_event else {
                return Err(GifUploadError::MissingNotifyAck.into());
            };
            let event = event_result?;
            match event {
                NotifyEvent::NextPackage(TransferFamily::Gif) => Ok(GifAckOutcome::Continue),
                NotifyEvent::Finished(TransferFamily::Gif) => Ok(GifAckOutcome::Finished),
                NotifyEvent::Error(TransferFamily::Gif, status) => {
                    Err(GifUploadError::TransferRejected { status }.into())
                }
                _other => Err(GifUploadError::UnexpectedNotifyEvent.into()),
            }
        }
    }
}

fn apply_gif_header_profile(header: &mut [u8; 16], profile: GifHeaderProfile) {
    match profile {
        GifHeaderProfile::Timed => {
            header[13] = 0x05;
            header[14] = 0x00;
            header[15] = 0x0D;
        }
        GifHeaderProfile::NoTimeSignature => {
            header[13] = 0x00;
            header[14] = 0x00;
            header[15] = 0x0C;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::*;

    fn tiny_gif() -> GifAnimation {
        let payload = vec![
            0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0x00,
            0x00, 0x00, 0xFF, 0xFF, 0xFF, 0x21, 0xF9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, 0x2C,
            0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44, 0x01, 0x00,
            0x3B,
        ];
        GifAnimation::try_from(payload).expect("tiny gif payload should parse")
    }

    #[test]
    fn gif_upload_request_defaults_match_protocol_pacing() {
        let request = GifUploadRequest::new(tiny_gif());

        assert_eq!(43, request.payload().len());
        assert_eq!(Duration::from_millis(20), request.per_fragment_delay());
        assert_eq!(Duration::from_secs(5), request.ack_timeout());
    }

    #[test]
    fn gif_upload_receipt_accessors_return_constructor_values() {
        let receipt = GifUploadReceipt::new(4112, 9, 1, true);

        assert_eq!(4112, receipt.bytes_written());
        assert_eq!(9, receipt.chunks_written());
        assert_eq!(1, receipt.logical_chunks_sent());
        assert_eq!(true, receipt.cached());
    }

    #[rstest]
    #[case(
        GifHeaderProfile::Timed,
        [0x05, 0x00, 0x0D]
    )]
    #[case(
        GifHeaderProfile::NoTimeSignature,
        [0x00, 0x00, 0x0C]
    )]
    fn gif_header_profile_sets_expected_tail_bytes(
        #[case] profile: GifHeaderProfile,
        #[case] expected_tail: [u8; 3],
    ) {
        let fields = GifHeaderFields::new(0x1000, GifChunkFlag::First, 0x2000, 0x1122_3344)
            .expect("valid gif header fields should construct");
        let mut header = FrameCodec::encode_gif_header(fields);

        apply_gif_header_profile(&mut header, profile);

        assert_eq!(expected_tail, [header[13], header[14], header[15]]);
    }
}
