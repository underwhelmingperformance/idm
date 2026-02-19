use std::time::Duration;

use bon::Builder;
use crc32fast::hash;
use idm_macros::progress;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use super::transport_chunk_sizer::AdaptiveChunkSizer;
use super::upload_common::{
    UploadAckOutcome, apply_fragment_delay, drain_stale_notifications, remaining_transport_chunks,
    resolve_upload_chunk_sizing, wait_for_transfer_ack,
};
use crate::error::ProtocolError;
use crate::hw::{DeviceSession, PanelDimensions, WriteMode};
use crate::protocol::EndpointId;
use crate::{
    FrameCodec, GifChunkFlag, ImageHeaderFields, MediaHeaderTail, Rgb888Frame, TransferFamily,
};

const LOGICAL_CHUNK_SIZE: usize = 4096;
const DEFAULT_PER_FRAGMENT_DELAY: Duration = Duration::from_millis(20);
const DEFAULT_NOTIFY_ACK_TIMEOUT: Duration = Duration::from_secs(5);
const MEDIA_HEADER_LEN: usize = 16;

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
    #[error(
        "device reported image transfer completion too early at chunk {chunk_index} of {total_chunks}"
    )]
    PrematureFinish {
        chunk_index: usize,
        total_chunks: usize,
    },
}

/// Image upload request parameters.
#[derive(Debug, Clone, Eq, PartialEq, Builder)]
pub struct ImageUploadRequest {
    frame: Rgb888Frame,
    #[builder(default = DEFAULT_PER_FRAGMENT_DELAY)]
    per_fragment_delay: Duration,
    #[builder(default = DEFAULT_NOTIFY_ACK_TIMEOUT)]
    ack_timeout: Duration,
    #[builder(default = MediaHeaderTail::default())]
    media_header_tail: MediaHeaderTail,
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
            media_header_tail: MediaHeaderTail::default(),
        }
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

    /// Returns the media-header tail policy used for bytes `13..15`.
    ///
    /// ```
    /// use idm::{ImageUploadRequest, MediaHeaderTail, PanelDimensions, Rgb888Frame};
    ///
    /// let dimensions = PanelDimensions::new(1, 1).expect("1x1 should be valid");
    /// let frame = Rgb888Frame::try_from((dimensions, vec![0x01, 0x02, 0x03]))
    ///     .expect("1x1 frame should require 3 bytes");
    /// let request = ImageUploadRequest::new(frame);
    /// assert_eq!(MediaHeaderTail::default(), request.media_header_tail());
    /// ```
    #[must_use]
    pub fn media_header_tail(&self) -> MediaHeaderTail {
        self.media_header_tail
    }

    /// Returns a request with an explicit media-header tail policy.
    ///
    /// ```
    /// use idm::{
    ///     ImageUploadRequest, MaterialSlot, MaterialTimeSign, MediaHeaderTail, PanelDimensions,
    ///     Rgb888Frame,
    /// };
    ///
    /// let dimensions = PanelDimensions::new(1, 1).expect("1x1 should be valid");
    /// let frame = Rgb888Frame::try_from((dimensions, vec![0x01, 0x02, 0x03]))
    ///     .expect("1x1 frame should require 3 bytes");
    /// let tail = MediaHeaderTail::NoTimeSignature;
    /// let request = ImageUploadRequest::new(frame).with_media_header_tail(tail);
    /// assert_eq!(tail, request.media_header_tail());
    /// ```
    #[must_use]
    pub fn with_media_header_tail(mut self, media_header_tail: MediaHeaderTail) -> Self {
        self.media_header_tail = media_header_tail;
        self
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
    #[progress(
        message = "Uploading image payload",
        finished = match result {
            Ok(receipt) => format!(
                "✓ Uploaded image payload: {} bytes in {} chunk(s) across {} logical chunk(s)",
                receipt.bytes_written(),
                receipt.chunks_written(),
                receipt.logical_chunks_sent(),
            ),
            Err(_error) => "✗ Image upload failed".to_string(),
        },
        skip_all,
        level = "info"
    )]
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
        let baseline_chunk_size = write_chunk_size(session)?;
        let mut chunk_sizer = AdaptiveChunkSizer::from_baseline(baseline_chunk_size);
        let logical_chunk_sizes: Vec<usize> = payload
            .chunks(LOGICAL_CHUNK_SIZE)
            .map(|logical_chunk| MEDIA_HEADER_LEN + logical_chunk.len())
            .collect();
        let logical_chunks_total = logical_chunk_sizes.len();
        let crc32 = hash(payload);
        let payload_len_u32 = u32::try_from(payload.len()).map_err(|_overflow| {
            ImageUploadError::PayloadTooLarge {
                payload_len: payload.len(),
                max_payload_len: u32::MAX as usize,
            }
        })?;
        let endpoint = EndpointId::ReadNotifyCharacteristic;

        let mut stream = session
            .notification_stream(endpoint, None, CancellationToken::new())
            .await?;

        drain_stale_notifications(&mut stream, "image").await?;

        let mut bytes_written = 0usize;
        let mut chunks_written = 0usize;
        let mut logical_chunks_sent = 0usize;
        let mut transport_chunks_total: usize = logical_chunk_sizes
            .iter()
            .map(|block_len| block_len.div_ceil(chunk_sizer.current()))
            .sum();
        progress_set_length!(transport_chunks_total);

        for (index, logical_chunk) in payload.chunks(LOGICAL_CHUNK_SIZE).enumerate() {
            let chunk_flag = if index == 0 {
                GifChunkFlag::First
            } else {
                GifChunkFlag::Continuation
            };
            let chunk_payload_len = u16::try_from(logical_chunk.len()).map_err(|_overflow| {
                ImageUploadError::ChunkPayloadTooLarge {
                    chunk_payload_len: logical_chunk.len(),
                    max_payload_len: u16::MAX as usize,
                }
            })?;
            let fields =
                ImageHeaderFields::new(chunk_payload_len, chunk_flag, payload_len_u32, crc32)?;
            let mut header = FrameCodec::encode_image_header(fields);
            request.media_header_tail().apply_to_header(&mut header);

            let mut frame_block = Vec::with_capacity(header.len() + logical_chunk.len());
            frame_block.extend_from_slice(&header);
            frame_block.extend_from_slice(logical_chunk);
            logical_chunks_sent += 1;

            let mut block_offset = 0usize;
            while block_offset < frame_block.len() {
                let chunk_size = chunk_sizer.current();
                let block_end = usize::min(block_offset + chunk_size, frame_block.len());
                let transport_chunk = &frame_block[block_offset..block_end];
                match session
                    .write_endpoint(
                        EndpointId::WriteCharacteristic,
                        transport_chunk,
                        WriteMode::WithoutResponse,
                    )
                    .await
                {
                    Ok(()) => {
                        bytes_written += transport_chunk.len();
                        chunks_written += 1;
                        block_offset = block_end;
                        progress_inc!();
                        progress_trace!(chunks_written, transport_chunks_total);
                        apply_fragment_delay(request.per_fragment_delay).await;
                    }
                    Err(error) => {
                        let previous_chunk_size = chunk_sizer.current();
                        if !chunk_sizer.reduce_on_failure() {
                            return Err(error.into());
                        }
                        let next_chunk_size = chunk_sizer.current();
                        let remaining_chunks = remaining_transport_chunks(
                            &logical_chunk_sizes,
                            index,
                            block_offset,
                            next_chunk_size,
                        );
                        transport_chunks_total = chunks_written + remaining_chunks;
                        progress_set_length!(transport_chunks_total);
                        tracing::debug!(
                            ?error,
                            previous_chunk_size,
                            next_chunk_size,
                            logical_chunk_index = index + 1,
                            logical_chunks_total,
                            "write failed during image upload; reducing chunk size and retrying"
                        );
                    }
                }
            }

            let ack_outcome =
                wait_for_transfer_ack(&mut stream, request.ack_timeout, TransferFamily::Image)
                    .await?;
            if matches!(ack_outcome, UploadAckOutcome::Finished) {
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

        drop(stream);
        Ok(ImageUploadReceipt::new(
            bytes_written,
            chunks_written,
            logical_chunks_sent,
        ))
    }
}

fn write_chunk_size(session: &DeviceSession) -> Result<usize, ProtocolError> {
    let chunk_sizing = resolve_upload_chunk_sizing(session);
    tracing::trace!(
        write_without_response_limit = chunk_sizing.reported_limit(),
        fallback_chunk = chunk_sizing.fallback_chunk(),
        baseline_chunk_size = chunk_sizing.baseline_chunk_size(),
        initial_probe_chunk_size = chunk_sizing.initial_probe_chunk_size(),
        probing_enabled = chunk_sizing.probing_enabled(),
        using_fallback_baseline = chunk_sizing.using_fallback_baseline(),
        "resolved image upload chunk sizing"
    );
    if chunk_sizing.baseline_chunk_size() == 0 {
        return Err(ImageUploadError::InvalidChunkSize.into());
    }
    Ok(chunk_sizing.baseline_chunk_size())
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
        assert_eq!(MediaHeaderTail::default(), request.media_header_tail());
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
    #[case(MediaHeaderTail::default(), [0x05, 0x00, 0x0D])]
    #[case(
        MediaHeaderTail::NoTimeSignature,
        [0x00, 0x00, 0x0C]
    )]
    #[case(
        MediaHeaderTail::timed(
            crate::TimedMaterialSlot::new(0x2A).expect("0x2A should be valid timed slot"),
            crate::MaterialTimeSign::TenSeconds
        ),
        [0x0A, 0x00, 0x2A]
    )]
    fn media_header_tail_sets_expected_tail_bytes(
        #[case] tail: MediaHeaderTail,
        #[case] expected_tail: [u8; 3],
    ) {
        let fields = ImageHeaderFields::new(0x1000, GifChunkFlag::First, 0x2000, 0x1122_3344)
            .expect("valid image header fields should construct");
        let mut header = FrameCodec::encode_image_header(fields);

        tail.apply_to_header(&mut header);

        assert_eq!(expected_tail, [header[13], header[14], header[15]]);
    }
}
