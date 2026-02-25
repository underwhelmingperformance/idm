use std::time::Duration;

use bon::Builder;
use idm_macros::progress;
use thiserror::Error;

use super::upload_common::{UploadAckError, apply_fragment_delay};
use crate::error::ProtocolError;
use crate::hw::{Ack, DeviceSession, PanelDimensions, SessionWriter};
use crate::{
    FrameCodec, GifAnimation, GifChunkFlag, GifHeaderFields, MediaHeaderTail, TransferFamily,
};
const POST_FINISH_SETTLE_DELAY: Duration = Duration::from_millis(500);

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
}

/// GIF upload request parameters.
#[derive(Debug, Clone, Eq, PartialEq, Builder)]
pub struct GifUploadRequest {
    gif: GifAnimation,
    #[builder(default = MediaHeaderTail::default())]
    media_header_tail: MediaHeaderTail,
}

impl GifUploadRequest {
    /// Creates a GIF upload request.
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
        let media_header_tail = request.media_header_tail();

        let encoder = move |chunk: &[u8], index: usize, total_len: u32, crc: u32| {
            let flag = if index == 0 {
                GifChunkFlag::First
            } else {
                GifChunkFlag::Continuation
            };
            let chunk_len = u16::try_from(chunk.len()).map_err(|_overflow| {
                GifUploadError::ChunkPayloadTooLarge {
                    chunk_payload_len: chunk.len(),
                    max_payload_len: u16::MAX as usize,
                }
            })?;
            let fields = GifHeaderFields::new(chunk_len, flag, total_len, crc)?;
            let mut header = FrameCodec::encode_gif_header(fields);
            media_header_tail.apply_to_header(&mut header);
            Ok(header.to_vec())
        };

        let stats = SessionWriter::builder()
            .session(session)
            .payload(payload)
            .ack(Ack::Transfer(TransferFamily::Gif))
            .header(&encoder)
            .allow_early_finish(true)
            .build()
            .send()
            .await?;

        let cached = match (stats.logical_chunks_sent, stats.total_logical_chunks) {
            (1, total) if total > 1 => true,
            (sent, total) if sent < total => {
                return Err(UploadAckError::PrematureFinish {
                    chunk_index: sent,
                    total_chunks: total,
                }
                .into());
            }
            _ => false,
        };

        // Keep the link up briefly so the panel can apply the newly selected
        // material before callers close the session.
        apply_fragment_delay(POST_FINISH_SETTLE_DELAY).await;
        Ok(GifUploadReceipt::new(
            stats.bytes_written,
            stats.chunks_written,
            stats.logical_chunks_sent,
            cached,
        ))
    }
}

#[cfg(test)]
mod tests {
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
    fn gif_upload_request_defaults_match_expected_fields() {
        let request = GifUploadRequest::new(tiny_gif());

        assert_eq!(43, request.payload().len());
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
        [0x00, 0x00, 0x0C]
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
