use bon::Builder;
use idm_macros::progress;
use thiserror::Error;

use crate::error::ProtocolError;
use crate::hw::{Ack, DeviceSession, PanelDimensions, SessionWriter};
use crate::{
    FrameCodec, GifChunkFlag, ImageHeaderFields, MediaHeaderTail, Rgb888Frame, TransferFamily,
};

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
}

/// Image upload request parameters.
#[derive(Debug, Clone, Eq, PartialEq, Builder)]
pub struct ImageUploadRequest {
    frame: Rgb888Frame,
    #[builder(default = MediaHeaderTail::default())]
    media_header_tail: MediaHeaderTail,
}

impl ImageUploadRequest {
    /// Creates an image upload request.
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
        let media_header_tail = request.media_header_tail();

        let encoder = move |chunk: &[u8], index: usize, total_len: u32, crc: u32| {
            let flag = if index == 0 {
                GifChunkFlag::First
            } else {
                GifChunkFlag::Continuation
            };
            let chunk_len = u16::try_from(chunk.len()).map_err(|_overflow| {
                ImageUploadError::ChunkPayloadTooLarge {
                    chunk_payload_len: chunk.len(),
                    max_payload_len: u16::MAX as usize,
                }
            })?;
            let fields = ImageHeaderFields::new(chunk_len, flag, total_len, crc)?;
            let mut header = FrameCodec::encode_image_header(fields);
            media_header_tail.apply_to_header(&mut header);
            Ok(header.to_vec())
        };

        let stats = SessionWriter::builder()
            .session(session)
            .payload(payload)
            .ack(Ack::Transfer(TransferFamily::Image))
            .header(&encoder)
            .build()
            .send()
            .await?;

        Ok(ImageUploadReceipt::new(
            stats.bytes_written,
            stats.chunks_written,
            stats.logical_chunks_sent,
        ))
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::*;

    #[test]
    fn image_upload_request_defaults_match_expected_fields() -> Result<(), crate::Rgb888FrameError>
    {
        let dimensions = PanelDimensions::new(1, 1).expect("1x1 should be valid");
        let frame = Rgb888Frame::try_from((dimensions, vec![0x89, 0x50, 0x4E]))?;
        let request = ImageUploadRequest::new(frame);

        assert_eq!(&[0x89, 0x50, 0x4E], request.payload());
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
    #[case(MediaHeaderTail::default(), [0x00, 0x00, 0x0C])]
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
