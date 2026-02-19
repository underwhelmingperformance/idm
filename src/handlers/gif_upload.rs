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
    FrameCodec, GifAnimation, GifChunkFlag, GifHeaderFields, MediaHeaderTail, TransferFamily,
};

const LOGICAL_CHUNK_SIZE: usize = 4096;
const DEFAULT_PER_FRAGMENT_DELAY: Duration = Duration::from_millis(20);
const DEFAULT_NOTIFY_ACK_TIMEOUT: Duration = Duration::from_secs(5);
const POST_FINISH_SETTLE_DELAY: Duration = Duration::from_millis(500);
const GIF_HEADER_LEN: usize = 16;

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
    #[error(
        "device reported transfer completion too early at chunk {chunk_index} of {total_chunks}"
    )]
    PrematureFinish {
        chunk_index: usize,
        total_chunks: usize,
    },
}

/// GIF upload request parameters.
#[derive(Debug, Clone, Eq, PartialEq, Builder)]
pub struct GifUploadRequest {
    gif: GifAnimation,
    #[builder(default = DEFAULT_PER_FRAGMENT_DELAY)]
    per_fragment_delay: Duration,
    #[builder(default = DEFAULT_NOTIFY_ACK_TIMEOUT)]
    ack_timeout: Duration,
    #[builder(default = MediaHeaderTail::default())]
    media_header_tail: MediaHeaderTail,
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
            media_header_tail: MediaHeaderTail::default(),
        }
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

    /// Returns the media-header tail policy used for bytes `13..15`.
    ///
    /// ```
    /// use idm::{GifAnimation, GifUploadRequest, MediaHeaderTail};
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
    /// assert_eq!(MediaHeaderTail::default(), request.media_header_tail());
    /// ```
    #[must_use]
    pub fn media_header_tail(&self) -> MediaHeaderTail {
        self.media_header_tail
    }

    /// Returns a request with an explicit media-header tail policy.
    ///
    /// ```
    /// use idm::{GifAnimation, GifUploadRequest, MaterialSlot, MaterialTimeSign, MediaHeaderTail};
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
    /// let tail = MediaHeaderTail::NoTimeSignature;
    /// let request = GifUploadRequest::new(gif).with_media_header_tail(tail);
    /// assert_eq!(tail, request.media_header_tail());
    /// ```
    #[must_use]
    pub fn with_media_header_tail(mut self, media_header_tail: MediaHeaderTail) -> Self {
        self.media_header_tail = media_header_tail;
        self
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
    #[progress(
        message = "Uploading GIF payload",
        finished = match result {
            Ok(receipt) if receipt.cached() => format!(
                "✓ Uploaded GIF payload: {} bytes in {} chunk(s); device cache hit",
                receipt.bytes_written(),
                receipt.chunks_written(),
            ),
            Ok(receipt) => format!(
                "✓ Uploaded GIF payload: {} bytes in {} chunk(s) across {} logical chunk(s)",
                receipt.bytes_written(),
                receipt.chunks_written(),
                receipt.logical_chunks_sent(),
            ),
            Err(_error) => "✗ GIF upload failed".to_string(),
        },
        skip_all,
        level = "info"
    )]
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
        let baseline_chunk_size = write_chunk_size(session)?;
        let mut chunk_sizer = AdaptiveChunkSizer::from_baseline(baseline_chunk_size);
        let logical_chunk_sizes: Vec<usize> = payload
            .chunks(LOGICAL_CHUNK_SIZE)
            .map(|logical_chunk| GIF_HEADER_LEN + logical_chunk.len())
            .collect();
        let logical_chunks_total = logical_chunk_sizes.len();
        let crc32 = hash(payload);
        let payload_len_u32 =
            u32::try_from(payload.len()).map_err(|_overflow| GifUploadError::PayloadTooLarge {
                payload_len: payload.len(),
                max_payload_len: u32::MAX as usize,
            })?;
        let endpoint = EndpointId::ReadNotifyCharacteristic;

        let mut stream = session
            .notification_stream(endpoint, None, CancellationToken::new())
            .await?;

        drain_stale_notifications(&mut stream, "gif").await?;

        let mut bytes_written = 0usize;
        let mut chunks_written = 0usize;
        let mut logical_chunks_sent = 0usize;
        let mut cached = false;
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
                GifUploadError::ChunkPayloadTooLarge {
                    chunk_payload_len: logical_chunk.len(),
                    max_payload_len: u16::MAX as usize,
                }
            })?;
            let fields =
                GifHeaderFields::new(chunk_payload_len, chunk_flag, payload_len_u32, crc32)?;
            let mut header = FrameCodec::encode_gif_header(fields);
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
                            "write failed during GIF upload; reducing chunk size and retrying"
                        );
                    }
                }
            }

            let ack_outcome =
                wait_for_transfer_ack(&mut stream, request.ack_timeout, TransferFamily::Gif)
                    .await?;
            if matches!(ack_outcome, UploadAckOutcome::Finished) {
                let chunk_number = index + 1;
                if chunk_number < logical_chunks_total {
                    if chunk_number == 1 {
                        cached = true;
                        progress_set_length!(chunks_written);
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

        // Keep the link up briefly so the panel can apply the newly selected
        // material before callers close the session.
        apply_fragment_delay(POST_FINISH_SETTLE_DELAY).await;
        drop(stream);
        Ok(GifUploadReceipt::new(
            bytes_written,
            chunks_written,
            logical_chunks_sent,
            cached,
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
        "resolved gif upload chunk sizing"
    );
    if chunk_sizing.baseline_chunk_size() == 0 {
        return Err(GifUploadError::InvalidChunkSize.into());
    }
    Ok(chunk_sizing.baseline_chunk_size())
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
        assert_eq!(MediaHeaderTail::default(), request.media_header_tail());
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
        MediaHeaderTail::default(),
        [0x05, 0x00, 0x0D]
    )]
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
        let fields = GifHeaderFields::new(0x1000, GifChunkFlag::First, 0x2000, 0x1122_3344)
            .expect("valid gif header fields should construct");
        let mut header = FrameCodec::encode_gif_header(fields);

        tail.apply_to_header(&mut header);

        assert_eq!(expected_tail, [header[13], header[14], header[15]]);
    }
}
