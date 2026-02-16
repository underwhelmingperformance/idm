use std::time::Duration;

use crc32fast::hash;
use font8x8::UnicodeFonts;
use thiserror::Error;
use tokio::time::timeout;

use crate::error::ProtocolError;
use crate::hw::{DeviceSession, WriteMode};
use crate::protocol::EndpointId;
use crate::{FrameCodec, NotificationDecodeError, NotificationHandler, NotifyEvent, Rgb};

use super::FrameCodecError;

const METADATA_LEN: usize = 14;
const UNUSABLE_WRITE_WITHOUT_RESPONSE_LIMIT: usize = 20;
const GLYPH_PREFIX: [u8; 4] = [0x05, 0xFF, 0xFF, 0xFF];
const GLYPH_WIDTH: usize = 16;
const GLYPH_HEIGHT: usize = 32;
const GLYPH_BYTES: usize = (GLYPH_WIDTH * GLYPH_HEIGHT) / 8;
const FONT_BITMAP_WIDTH: usize = 8;
const FONT_BITMAP_HEIGHT: usize = 8;
const SCALE_X: usize = 2;
const SCALE_Y: usize = 4;

/// Errors returned by text upload operations.
#[derive(Debug, Error)]
pub enum TextUploadError {
    #[error("text upload request cannot be empty")]
    EmptyText,
    #[error("text upload has {count} characters but the protocol maximum is {max}")]
    TooManyCharacters { count: usize, max: usize },
    #[error("write chunk size cannot be zero")]
    InvalidChunkSize,
    #[error("notification acknowledgement timed out after {timeout_ms}ms")]
    NotifyAckTimeout { timeout_ms: u64 },
    #[error("notification stream ended before an acknowledgement was received")]
    MissingNotifyAck,
    #[error("received unexpected notification while waiting for an acknowledgement")]
    UnexpectedNotifyEvent,
    #[error("text upload path is unresolved for this device routing profile")]
    UnresolvedTextPath,
    #[error(transparent)]
    NotifyDecode(#[from] NotificationDecodeError),
}

/// Pacing strategy used while writing upload chunks.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum UploadPacing {
    /// Do not pace between transport chunks.
    None,
    /// Sleep for a fixed delay after each transport chunk.
    Delay { per_chunk: Duration },
    /// Wait for one notification acknowledgement after each transport chunk.
    NotifyAck { timeout: Duration },
}

/// Text upload rendering options.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct TextOptions {
    text_mode: u8,
    speed: u8,
    text_colour_mode: u8,
    text_colour: Rgb,
    background_mode: u8,
    background_colour: Rgb,
}

impl Default for TextOptions {
    fn default() -> Self {
        Self {
            text_mode: 0x00,
            speed: 0x20,
            text_colour_mode: 0x01,
            text_colour: Rgb::new(0xFF, 0xFF, 0xFF),
            background_mode: 0x00,
            background_colour: Rgb::new(0x00, 0x00, 0x00),
        }
    }
}

impl TextOptions {
    /// Creates text upload rendering options.
    ///
    /// ```
    /// use idm::{Rgb, TextOptions};
    ///
    /// let options = TextOptions::new(0x00, 0x20, 0x01, Rgb::new(255, 255, 255), 0x00, Rgb::new(0, 0, 0));
    /// let _ = options;
    /// ```
    #[must_use]
    pub fn new(
        text_mode: u8,
        speed: u8,
        text_colour_mode: u8,
        text_colour: Rgb,
        background_mode: u8,
        background_colour: Rgb,
    ) -> Self {
        Self {
            text_mode,
            speed,
            text_colour_mode,
            text_colour,
            background_mode,
            background_colour,
        }
    }
}

/// Text upload request.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct TextUploadRequest {
    text: String,
    options: TextOptions,
    pacing: UploadPacing,
}

impl TextUploadRequest {
    /// Creates a text upload request with default options and no pacing.
    ///
    /// ```
    /// use idm::TextUploadRequest;
    ///
    /// let request = TextUploadRequest::new("Hello");
    /// let _ = request;
    /// ```
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            options: TextOptions::default(),
            pacing: UploadPacing::None,
        }
    }

    /// Overrides text rendering options.
    ///
    /// ```
    /// use idm::{Rgb, TextOptions, TextUploadRequest};
    ///
    /// let request = TextUploadRequest::new("Hi").with_options(TextOptions::new(
    ///     0x00,
    ///     0x20,
    ///     0x01,
    ///     Rgb::new(255, 255, 255),
    ///     0x00,
    ///     Rgb::new(0, 0, 0),
    /// ));
    /// let _ = request;
    /// ```
    #[must_use]
    pub fn with_options(mut self, options: TextOptions) -> Self {
        self.options = options;
        self
    }

    /// Overrides upload pacing behaviour.
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use idm::{TextUploadRequest, UploadPacing};
    ///
    /// let request = TextUploadRequest::new("Hi").with_pacing(UploadPacing::Delay {
    ///     per_chunk: Duration::from_millis(25),
    /// });
    /// let _ = request;
    /// ```
    #[must_use]
    pub fn with_pacing(mut self, pacing: UploadPacing) -> Self {
        self.pacing = pacing;
        self
    }
}

/// Upload result metadata.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct UploadReceipt {
    bytes_written: usize,
    chunks_written: usize,
}

impl UploadReceipt {
    /// Creates an upload receipt.
    ///
    /// ```
    /// use idm::UploadReceipt;
    ///
    /// let receipt = UploadReceipt::new(123, 2);
    /// assert_eq!(123, receipt.bytes_written());
    /// ```
    #[must_use]
    pub fn new(bytes_written: usize, chunks_written: usize) -> Self {
        Self {
            bytes_written,
            chunks_written,
        }
    }

    /// Returns the total bytes written to `fa02`.
    ///
    /// ```
    /// use idm::UploadReceipt;
    ///
    /// let receipt = UploadReceipt::new(123, 2);
    /// assert_eq!(123, receipt.bytes_written());
    /// ```
    #[must_use]
    pub fn bytes_written(&self) -> usize {
        self.bytes_written
    }

    /// Returns the number of transport chunks written.
    ///
    /// ```
    /// use idm::UploadReceipt;
    ///
    /// let receipt = UploadReceipt::new(123, 2);
    /// assert_eq!(2, receipt.chunks_written());
    /// ```
    #[must_use]
    pub fn chunks_written(&self) -> usize {
        self.chunks_written
    }
}

/// Uploads scrolling text payloads to the iDotMatrix device.
pub struct TextUploadHandler;

impl TextUploadHandler {
    /// Uploads a text payload to the active session.
    ///
    /// ```
    /// # async fn demo(session: idm::DeviceSession) -> Result<(), idm::ProtocolError> {
    /// use idm::{TextUploadHandler, TextUploadRequest};
    ///
    /// let request = TextUploadRequest::new("Hello");
    /// let _receipt = TextUploadHandler::upload(&session, request).await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error when payload construction, BLE writes, or pacing fails.
    pub async fn upload(
        session: &DeviceSession,
        request: TextUploadRequest,
    ) -> Result<UploadReceipt, ProtocolError> {
        ensure_text_path_is_resolved(session)?;
        let frame = build_upload_frame(&request)?;
        let chunk_size = write_chunk_size(session)?;

        let mut chunks_written = 0usize;
        let endpoint = EndpointId::ReadNotifyCharacteristic;
        let use_notify = matches!(request.pacing, UploadPacing::NotifyAck { .. });

        if use_notify {
            session.subscribe_endpoint(endpoint).await?;
        }

        let upload_result = async {
            for chunk in frame.chunks(chunk_size) {
                session
                    .write_endpoint(
                        EndpointId::WriteCharacteristic,
                        chunk,
                        WriteMode::WithoutResponse,
                    )
                    .await?;
                chunks_written += 1;
                apply_pacing(session, request.pacing).await?;
            }

            Ok(UploadReceipt::new(frame.len(), chunks_written))
        }
        .await;

        if use_notify {
            match session.unsubscribe_endpoint(endpoint).await {
                Ok(()) => {}
                Err(error) => {
                    if upload_result.is_ok() {
                        return Err(error.into());
                    }
                    tracing::debug!(
                        ?error,
                        "failed to unsubscribe text-upload notifications cleanly"
                    );
                }
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
        return Err(TextUploadError::InvalidChunkSize.into());
    }
    Ok(chunk_size)
}

fn ensure_text_path_is_resolved(session: &DeviceSession) -> Result<(), ProtocolError> {
    if session
        .device_routing_profile()
        .is_some_and(|profile| profile.text_path.is_none())
    {
        return Err(TextUploadError::UnresolvedTextPath.into());
    }

    Ok(())
}

async fn apply_pacing(session: &DeviceSession, pacing: UploadPacing) -> Result<(), ProtocolError> {
    match pacing {
        UploadPacing::None => Ok(()),
        UploadPacing::Delay { per_chunk } => {
            if !per_chunk.is_zero() {
                tokio::time::sleep(per_chunk).await;
            }
            Ok(())
        }
        UploadPacing::NotifyAck {
            timeout: timeout_duration,
        } => {
            wait_for_notify_ack(session, timeout_duration).await?;
            Ok(())
        }
    }
}

async fn wait_for_notify_ack(
    session: &DeviceSession,
    timeout_duration: Duration,
) -> Result<(), ProtocolError> {
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
            Err(TextUploadError::NotifyAckTimeout { timeout_ms }.into())
        }
        Ok(Err(error)) => Err(error.into()),
        Ok(Ok(_summary)) => {
            let Some(event_result) = decoded_event else {
                return Err(TextUploadError::MissingNotifyAck.into());
            };
            let event = event_result?;
            match event {
                NotifyEvent::ChunkAck | NotifyEvent::UploadComplete => Ok(()),
                NotifyEvent::Unknown(_) => Err(TextUploadError::UnexpectedNotifyEvent.into()),
            }
        }
    }
}

fn build_upload_frame(request: &TextUploadRequest) -> Result<Vec<u8>, ProtocolError> {
    let metadata = encode_metadata(&request.text, request.options)?;
    let glyph_stream = encode_glyph_stream(&request.text)?;

    let mut logical_payload = Vec::with_capacity(metadata.len() + glyph_stream.len());
    logical_payload.extend_from_slice(&metadata);
    logical_payload.extend_from_slice(&glyph_stream);

    let crc32 = hash(&logical_payload);
    let payload_len_u32 = u32::try_from(logical_payload.len()).map_err(|_overflow| {
        FrameCodecError::HeaderPayloadTooLarge {
            payload_len: u16::MAX,
            max_payload_len: u16::MAX - 16,
        }
    })?;
    let header_fields = crate::TextHeaderFields::new(
        u16::try_from(logical_payload.len()).map_err(|_overflow| {
            FrameCodecError::HeaderPayloadTooLarge {
                payload_len: u16::MAX,
                max_payload_len: u16::MAX - 16,
            }
        })?,
        payload_len_u32,
        crc32,
    )?;
    let header = FrameCodec::encode_text_header(header_fields);

    let mut full_frame = Vec::with_capacity(header.len() + logical_payload.len());
    full_frame.extend_from_slice(&header);
    full_frame.extend_from_slice(&logical_payload);
    Ok(full_frame)
}

fn encode_metadata(text: &str, options: TextOptions) -> Result<[u8; METADATA_LEN], ProtocolError> {
    let char_count = text.chars().count();
    if char_count == 0 {
        return Err(TextUploadError::EmptyText.into());
    }
    if char_count > usize::from(u16::MAX) {
        return Err(TextUploadError::TooManyCharacters {
            count: char_count,
            max: usize::from(u16::MAX),
        }
        .into());
    }
    let char_count_u16 =
        u16::try_from(char_count).map_err(|_overflow| TextUploadError::TooManyCharacters {
            count: char_count,
            max: usize::from(u16::MAX),
        })?;

    let mut metadata = [0u8; METADATA_LEN];
    metadata[0..2].copy_from_slice(&char_count_u16.to_le_bytes());
    metadata[2] = 0x00;
    metadata[3] = 0x01;
    metadata[4] = options.text_mode;
    metadata[5] = options.speed;
    metadata[6] = options.text_colour_mode;
    metadata[7] = options.text_colour.r;
    metadata[8] = options.text_colour.g;
    metadata[9] = options.text_colour.b;
    metadata[10] = options.background_mode;
    metadata[11] = options.background_colour.r;
    metadata[12] = options.background_colour.g;
    metadata[13] = options.background_colour.b;
    Ok(metadata)
}

fn encode_glyph_stream(text: &str) -> Result<Vec<u8>, ProtocolError> {
    if text.is_empty() {
        return Err(TextUploadError::EmptyText.into());
    }

    let mut stream = Vec::new();
    for ch in text.chars() {
        stream.extend_from_slice(&GLYPH_PREFIX);
        stream.extend_from_slice(&encode_one_glyph(ch));
    }
    Ok(stream)
}

fn encode_one_glyph(ch: char) -> [u8; GLYPH_BYTES] {
    let bitmap = font_bitmap_for(ch);
    let mut glyph = [0u8; GLYPH_BYTES];

    for (font_y, row) in bitmap.iter().copied().enumerate() {
        for font_x in 0..FONT_BITMAP_WIDTH {
            let set = (row >> font_x) & 0x01 == 1;
            if !set {
                continue;
            }

            for dy in 0..SCALE_Y {
                for dx in 0..SCALE_X {
                    let x = font_x * SCALE_X + dx;
                    let y = font_y * SCALE_Y + dy;
                    let bit_index = y * GLYPH_WIDTH + x;
                    let byte_index = bit_index / 8;
                    let bit_offset = bit_index % 8;
                    glyph[byte_index] |= 1 << bit_offset;
                }
            }
        }
    }

    glyph
}

fn font_bitmap_for(ch: char) -> [u8; FONT_BITMAP_HEIGHT] {
    if let Some(bitmap) = font8x8::BASIC_FONTS.get(ch) {
        return bitmap;
    }

    if let Some(fallback_bitmap) = font8x8::BASIC_FONTS.get('?') {
        return fallback_bitmap;
    }

    [0u8; FONT_BITMAP_HEIGHT]
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::*;

    #[test]
    fn metadata_encodes_expected_default_fields() {
        let metadata =
            encode_metadata("AB", TextOptions::default()).expect("metadata should encode");
        assert_eq!(
            [
                0x02, 0x00, 0x00, 0x01, 0x00, 0x20, 0x01, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00,
            ],
            metadata
        );
    }

    #[test]
    fn metadata_rejects_empty_text() {
        let result = encode_metadata("", TextOptions::default());
        assert_matches!(result, Err(ProtocolError::TextUpload(_)));
    }

    #[test]
    fn glyph_stream_prepends_prefix_per_character() {
        let stream = encode_glyph_stream("A").expect("glyph stream should encode");
        assert_eq!(&GLYPH_PREFIX, &stream[0..4]);
        assert_eq!(4 + GLYPH_BYTES, stream.len());
    }

    #[test]
    fn upload_frame_has_header_and_payload() {
        let request = TextUploadRequest::new("Hi");
        let frame = build_upload_frame(&request).expect("frame should encode");

        let expected_payload = METADATA_LEN + (2 * (GLYPH_PREFIX.len() + GLYPH_BYTES));
        assert_eq!(16 + expected_payload, frame.len());
        assert_eq!(0x03, frame[2]);
        assert_eq!(0x0C, frame[15]);
    }

    #[rstest]
    #[case('A')]
    #[case('?')]
    #[case(' ')]
    fn glyph_encoder_returns_expected_size(#[case] ch: char) {
        let glyph = encode_one_glyph(ch);
        assert_eq!(GLYPH_BYTES, glyph.len());
    }
}
