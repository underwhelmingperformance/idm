use std::time::Duration;

use crc32fast::hash;
use thiserror::Error;
use tokio::time::{sleep, timeout};
use tracing::instrument;

use crate::error::ProtocolError;
use crate::hw::{DeviceSession, GifHeaderProfile, PanelDimensions, WriteMode};
use crate::protocol::EndpointId;
use crate::{
    FrameCodec, GifChunkFlag, ImageHeaderFields, NotificationDecodeError, NotificationHandler,
    NotifyEvent, Rgb888Frame, TransferFamily,
};

const LOGICAL_CHUNK_SIZE: usize = 4096;
const DEFAULT_PER_FRAGMENT_DELAY: Duration = Duration::from_millis(20);
const DEFAULT_NOTIFY_ACK_TIMEOUT: Duration = Duration::from_secs(5);
const DRAIN_NOTIFICATION_TIMEOUT: Duration = Duration::from_millis(25);
const MAX_STALE_NOTIFICATION_DRAIN: usize = 8;
const UNUSABLE_WRITE_WITHOUT_RESPONSE_LIMIT: usize = 20;

/// Errors returned by image upload operations.
#[derive(Debug, Error)]
pub enum ImageUploadError {
    #[error("image upload payload is too large: {payload_len} bytes exceeds max {max_payload_len}")]
    PayloadTooLarge {
        payload_len: usize,
        max_payload_len: usize,
    },
    #[error("image upload requires known panel dimensions from the active device profile")]
    MissingPanelDimensions,
    #[error(
        "image upload frame dimensions {frame_dimensions} do not match device panel dimensions {device_dimensions}"
    )]
    PanelDimensionsMismatch {
        frame_dimensions: PanelDimensions,
        device_dimensions: PanelDimensions,
    },
    #[error(
        "image logical chunk payload is too large: {chunk_payload_len} bytes exceeds max {max_payload_len}"
    )]
    ChunkPayloadTooLarge {
        chunk_payload_len: usize,
        max_payload_len: usize,
    },
    #[error("image upload chunk size cannot be zero")]
    InvalidChunkSize,
    #[error("notification acknowledgement timed out after {timeout_ms}ms")]
    NotifyAckTimeout { timeout_ms: u64 },
    #[error("notification stream ended before an image acknowledgement was received")]
    MissingNotifyAck,
    #[error("received unexpected notification while waiting for an image acknowledgement")]
    UnexpectedNotifyEvent,
    #[error("image transfer was rejected by device status 0x{status:02X}")]
    TransferRejected { status: u8 },
    #[error(
        "device reported image transfer completion too early at chunk {chunk_index} of {total_chunks}"
    )]
    PrematureFinish {
        chunk_index: usize,
        total_chunks: usize,
    },
    #[error(transparent)]
    NotifyDecode(#[from] NotificationDecodeError),
}

/// Image upload request parameters.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ImageUploadRequest {
    frame: Rgb888Frame,
    per_fragment_delay: Duration,
    ack_timeout: Duration,
}

impl ImageUploadRequest {
    /// Creates an image upload request using default pacing.
    ///
    /// ```
    /// use idm::{ImageUploadRequest, PanelDimensions, Rgb888Frame};
    ///
    /// let dimensions = PanelDimensions::new(1, 1).expect("1x1 should be valid");
    /// let frame = Rgb888Frame::try_from((dimensions, vec![0x89, 0x50, 0x4E]))
    ///     .expect("1x1 frame should require 3 bytes");
    /// let request = ImageUploadRequest::new(frame);
    /// assert_eq!(&[0x89, 0x50, 0x4E], request.payload());
    /// ```
    #[must_use]
    pub fn new(frame: Rgb888Frame) -> Self {
        Self {
            frame,
            per_fragment_delay: DEFAULT_PER_FRAGMENT_DELAY,
            ack_timeout: DEFAULT_NOTIFY_ACK_TIMEOUT,
        }
    }

    /// Overrides delay between transport fragments.
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use idm::{ImageUploadRequest, PanelDimensions, Rgb888Frame};
    ///
    /// let dimensions = PanelDimensions::new(1, 1).expect("1x1 should be valid");
    /// let frame = Rgb888Frame::try_from((dimensions, vec![0x01, 0x02, 0x03]))
    ///     .expect("1x1 frame should require 3 bytes");
    /// let request = ImageUploadRequest::new(frame).with_per_fragment_delay(Duration::ZERO);
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
    /// use idm::{ImageUploadRequest, PanelDimensions, Rgb888Frame};
    ///
    /// let dimensions = PanelDimensions::new(1, 1).expect("1x1 should be valid");
    /// let frame = Rgb888Frame::try_from((dimensions, vec![0x01, 0x02, 0x03]))
    ///     .expect("1x1 frame should require 3 bytes");
    /// let request = ImageUploadRequest::new(frame).with_ack_timeout(Duration::from_millis(250));
    /// assert_eq!(Duration::from_millis(250), request.ack_timeout());
    /// ```
    #[must_use]
    pub fn with_ack_timeout(mut self, ack_timeout: Duration) -> Self {
        self.ack_timeout = ack_timeout;
        self
    }

    /// Returns the raw image payload bytes.
    ///
    /// ```
    /// use idm::{ImageUploadRequest, PanelDimensions, Rgb888Frame};
    ///
    /// let dimensions = PanelDimensions::new(1, 1).expect("1x1 should be valid");
    /// let frame = Rgb888Frame::try_from((dimensions, vec![0x01, 0x02, 0x03]))
    ///     .expect("1x1 frame should require 3 bytes");
    /// let request = ImageUploadRequest::new(frame);
    /// assert_eq!(&[0x01, 0x02, 0x03], request.payload());
    /// ```
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        self.frame.payload()
    }

    /// Returns the validated RGB888 frame.
    ///
    /// ```
    /// use idm::{ImageUploadRequest, PanelDimensions, Rgb888Frame};
    ///
    /// let dimensions = PanelDimensions::new(1, 1).expect("1x1 should be valid");
    /// let frame = Rgb888Frame::try_from((dimensions, vec![0xAA, 0xBB, 0xCC]))
    ///     .expect("1x1 frame should require 3 bytes");
    /// let request = ImageUploadRequest::new(frame.clone());
    /// assert_eq!(frame, request.frame().clone());
    /// ```
    #[must_use]
    pub fn frame(&self) -> &Rgb888Frame {
        &self.frame
    }

    /// Returns the configured transport-fragment delay.
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use idm::{ImageUploadRequest, PanelDimensions, Rgb888Frame};
    ///
    /// let dimensions = PanelDimensions::new(1, 1).expect("1x1 should be valid");
    /// let frame = Rgb888Frame::try_from((dimensions, vec![0x01, 0x02, 0x03]))
    ///     .expect("1x1 frame should require 3 bytes");
    /// let request = ImageUploadRequest::new(frame);
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
    /// use idm::{ImageUploadRequest, PanelDimensions, Rgb888Frame};
    ///
    /// let dimensions = PanelDimensions::new(1, 1).expect("1x1 should be valid");
    /// let frame = Rgb888Frame::try_from((dimensions, vec![0x01, 0x02, 0x03]))
    ///     .expect("1x1 frame should require 3 bytes");
    /// let request = ImageUploadRequest::new(frame);
    /// assert_eq!(Duration::from_secs(5), request.ack_timeout());
    /// ```
    #[must_use]
    pub fn ack_timeout(&self) -> Duration {
        self.ack_timeout
    }
}

/// Image upload metadata returned on success.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ImageUploadReceipt {
    bytes_written: usize,
    chunks_written: usize,
    logical_chunks_sent: usize,
}

impl ImageUploadReceipt {
    /// Creates an image upload receipt.
    ///
    /// ```
    /// use idm::ImageUploadReceipt;
    ///
    /// let receipt = ImageUploadReceipt::new(5032, 11, 2);
    /// assert_eq!(5032, receipt.bytes_written());
    /// ```
    #[must_use]
    pub fn new(bytes_written: usize, chunks_written: usize, logical_chunks_sent: usize) -> Self {
        Self {
            bytes_written,
            chunks_written,
            logical_chunks_sent,
        }
    }

    /// Returns total bytes written to `fa02`.
    ///
    /// ```
    /// use idm::ImageUploadReceipt;
    ///
    /// let receipt = ImageUploadReceipt::new(123, 2, 1);
    /// assert_eq!(123, receipt.bytes_written());
    /// ```
    #[must_use]
    pub fn bytes_written(&self) -> usize {
        self.bytes_written
    }

    /// Returns number of transport chunks written.
    ///
    /// ```
    /// use idm::ImageUploadReceipt;
    ///
    /// let receipt = ImageUploadReceipt::new(123, 2, 1);
    /// assert_eq!(2, receipt.chunks_written());
    /// ```
    #[must_use]
    pub fn chunks_written(&self) -> usize {
        self.chunks_written
    }

    /// Returns number of logical 4K chunks attempted.
    ///
    /// ```
    /// use idm::ImageUploadReceipt;
    ///
    /// let receipt = ImageUploadReceipt::new(123, 2, 1);
    /// assert_eq!(1, receipt.logical_chunks_sent());
    /// ```
    #[must_use]
    pub fn logical_chunks_sent(&self) -> usize {
        self.logical_chunks_sent
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum ImageAckOutcome {
    Continue,
    Finished,
}

/// Uploads non-DIY image payloads to iDotMatrix devices.
pub struct ImageUploadHandler;

impl ImageUploadHandler {
    /// Uploads one image payload to the active session.
    ///
    /// ```
    /// # async fn demo(session: idm::DeviceSession) -> Result<(), idm::ProtocolError> {
    /// use idm::{ImageUploadHandler, ImageUploadRequest, PanelDimensions, Rgb888Frame};
    ///
    /// let dimensions = PanelDimensions::new(1, 1).expect("1x1 should be valid");
    /// let frame = Rgb888Frame::try_from((dimensions, vec![0x89, 0x50, 0x4E]))
    ///     .expect("1x1 frame should require 3 bytes");
    /// let request = ImageUploadRequest::new(frame);
    /// let _receipt = ImageUploadHandler::upload(&session, request).await?;
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
        request: ImageUploadRequest,
    ) -> Result<ImageUploadReceipt, ProtocolError> {
        let device_dimensions = session
            .device_profile()
            .panel_dimensions()
            .ok_or(ImageUploadError::MissingPanelDimensions)?;
        let frame_dimensions = request.frame().dimensions();
        if frame_dimensions != device_dimensions {
            return Err(ImageUploadError::PanelDimensionsMismatch {
                frame_dimensions,
                device_dimensions,
            }
            .into());
        }

        let payload = request.payload();
        let chunk_size = write_chunk_size(session)?;
        let logical_chunks_total = payload.chunks(LOGICAL_CHUNK_SIZE).len();
        let crc32 = hash(payload);
        let payload_len_u32 = u32::try_from(payload.len()).map_err(|_overflow| {
            ImageUploadError::PayloadTooLarge {
                payload_len: payload.len(),
                max_payload_len: u32::MAX as usize,
            }
        })?;
        let endpoint = EndpointId::ReadNotifyCharacteristic;

        session.subscribe_endpoint(endpoint).await?;

        let upload_result = async {
            drain_stale_notifications(session, endpoint).await?;

            let mut bytes_written = 0usize;
            let mut chunks_written = 0usize;
            let mut logical_chunks_sent = 0usize;

            for (index, logical_chunk) in payload.chunks(LOGICAL_CHUNK_SIZE).enumerate() {
                let chunk_flag = if index == 0 {
                    GifChunkFlag::First
                } else {
                    GifChunkFlag::Continuation
                };
                let chunk_payload_len =
                    u16::try_from(logical_chunk.len()).map_err(|_overflow| {
                        ImageUploadError::ChunkPayloadTooLarge {
                            chunk_payload_len: logical_chunk.len(),
                            max_payload_len: u16::MAX as usize,
                        }
                    })?;
                let fields =
                    ImageHeaderFields::new(chunk_payload_len, chunk_flag, payload_len_u32, crc32)?;
                let mut header = FrameCodec::encode_image_header(fields);
                apply_media_header_profile(
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

                let ack_outcome = wait_for_image_ack(session, request.ack_timeout).await?;
                if matches!(ack_outcome, ImageAckOutcome::Finished) {
                    let chunk_number = index + 1;
                    if chunk_number < logical_chunks_total {
                        return Err(ImageUploadError::PrematureFinish {
                            chunk_index: chunk_number,
                            total_chunks: logical_chunks_total,
                        }
                        .into());
                    }
                    break;
                }
            }

            Ok(ImageUploadReceipt::new(
                bytes_written,
                chunks_written,
                logical_chunks_sent,
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
                    "failed to unsubscribe image-upload notifications cleanly"
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
        return Err(ImageUploadError::InvalidChunkSize.into());
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
            }
        }
    }

    Ok(())
}

#[instrument(skip(session), level = "trace", fields(timeout_ms = timeout_duration.as_millis()))]
async fn wait_for_image_ack(
    session: &DeviceSession,
    timeout_duration: Duration,
) -> Result<ImageAckOutcome, ProtocolError> {
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
            Err(ImageUploadError::NotifyAckTimeout { timeout_ms }.into())
        }
        Ok(Err(error)) => Err(error.into()),
        Ok(Ok(_summary)) => {
            let Some(event_result) = decoded_event else {
                return Err(ImageUploadError::MissingNotifyAck.into());
            };
            let event = event_result?;
            match event {
                NotifyEvent::NextPackage(TransferFamily::Image) => Ok(ImageAckOutcome::Continue),
                NotifyEvent::Finished(TransferFamily::Image) => Ok(ImageAckOutcome::Finished),
                NotifyEvent::Error(TransferFamily::Image, status) => {
                    Err(ImageUploadError::TransferRejected { status }.into())
                }
                _other => Err(ImageUploadError::UnexpectedNotifyEvent.into()),
            }
        }
    }
}

fn apply_media_header_profile(header: &mut [u8; 16], profile: GifHeaderProfile) {
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

    #[test]
    fn image_upload_request_defaults_match_protocol_pacing() -> Result<(), crate::Rgb888FrameError>
    {
        let dimensions = PanelDimensions::new(1, 1).expect("1x1 should be valid");
        let frame = Rgb888Frame::try_from((dimensions, vec![0x89, 0x50, 0x4E]))?;
        let request = ImageUploadRequest::new(frame);

        assert_eq!(&[0x89, 0x50, 0x4E], request.payload());
        assert_eq!(Duration::from_millis(20), request.per_fragment_delay());
        assert_eq!(Duration::from_secs(5), request.ack_timeout());
        Ok(())
    }

    #[test]
    fn image_upload_receipt_accessors_return_constructor_values() {
        let receipt = ImageUploadReceipt::new(5032, 11, 2);

        assert_eq!(5032, receipt.bytes_written());
        assert_eq!(11, receipt.chunks_written());
        assert_eq!(2, receipt.logical_chunks_sent());
    }

    #[rstest]
    #[case(GifHeaderProfile::Timed, [0x05, 0x00, 0x0D])]
    #[case(GifHeaderProfile::NoTimeSignature, [0x00, 0x00, 0x0C])]
    fn media_header_profile_sets_expected_tail_bytes(
        #[case] profile: GifHeaderProfile,
        #[case] expected_tail: [u8; 3],
    ) {
        let fields = ImageHeaderFields::new(0x1000, GifChunkFlag::First, 0x2000, 0x1122_3344)
            .expect("valid image header fields should construct");
        let mut header = FrameCodec::encode_image_header(fields);

        apply_media_header_profile(&mut header, profile);

        assert_eq!(expected_tail, [header[13], header[14], header[15]]);
    }
}
