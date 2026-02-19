use std::time::Duration;

use crc32fast::hash;
use font8x8::UnicodeFonts;
use idm_macros::progress;
use thiserror::Error;
use tokio::time::timeout;
use tracing::instrument;

use crate::error::ProtocolError;
use crate::hw::{DeviceSession, TextPath, WriteMode};
use crate::protocol::EndpointId;
use crate::{FrameCodec, NotificationDecodeError, NotifyEvent, Rgb, TransferFamily};

use super::FrameCodecError;

const METADATA_LEN: usize = 14;
const LOGICAL_CHUNK_SIZE: usize = 4096;
const DEFAULT_NOTIFY_ACK_TIMEOUT: Duration = Duration::from_secs(5);
const UNUSABLE_WRITE_WITHOUT_RESPONSE_LIMIT: usize = 20;
const FONT_BITMAP_WIDTH: usize = 8;
const FONT_BITMAP_HEIGHT: usize = 8;

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
    font_size: u8,
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
            font_size: 16,
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
            font_size: 16,
        }
    }

    /// Overrides font size used by `32x32` and `64x64` text paths.
    ///
    /// Supported values are `16`, `32`, and `64`. Invalid values are treated
    /// as `16` during encoding.
    #[must_use]
    pub fn with_font_size(mut self, font_size: u8) -> Self {
        self.font_size = font_size;
        self
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
    /// Creates a text upload request with default options and notify-ack pacing.
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
            pacing: UploadPacing::NotifyAck {
                timeout: DEFAULT_NOTIFY_ACK_TIMEOUT,
            },
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
    #[progress(
        message = "Uploading text payload",
        finished = "Text upload complete",
        skip_all,
        level = "info"
    )]
    pub async fn upload(
        session: &DeviceSession,
        request: TextUploadRequest,
    ) -> Result<UploadReceipt, ProtocolError> {
        tracing::trace!(
            text_char_count = request.text.chars().count(),
            ?request.pacing,
            "starting text upload"
        );
        ensure_text_path_is_resolved(session)?;
        let frame_blocks = build_upload_blocks(session, &request)?;
        let chunk_size = write_chunk_size(session)?;

        let mut chunks_written = 0usize;
        let mut bytes_written = 0usize;
        let endpoint = EndpointId::ReadNotifyCharacteristic;
        let use_notify = matches!(request.pacing, UploadPacing::NotifyAck { .. });

        if use_notify {
            session.subscribe_endpoint(endpoint).await?;
        }

        let upload_result = async {
            let mut transport_chunks_total = 0usize;

            for block in &frame_blocks {
                let block_transport_chunks = block.len().div_ceil(chunk_size);
                transport_chunks_total += block_transport_chunks;
                progress_inc_length!(block_transport_chunks);

                for transport_chunk in block.chunks(chunk_size) {
                    session
                        .write_endpoint(
                            EndpointId::WriteCharacteristic,
                            transport_chunk,
                            WriteMode::WithoutResponse,
                        )
                        .await?;
                    bytes_written += transport_chunk.len();
                    chunks_written += 1;
                    progress_inc!();
                    progress_trace!(chunks_written, transport_chunks_total);
                    apply_transport_pacing(request.pacing).await?;
                }
                apply_block_pacing(session, request.pacing).await?;
            }

            Ok(UploadReceipt::new(bytes_written, chunks_written))
        }
        .await;

        if use_notify {
            match session.unsubscribe_endpoint(endpoint).await {
                Ok(()) => {}
                Err(error) => {
                    if upload_result.is_ok() {
                        return Err(error.into());
                    }
                    tracing::trace!(
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
    let profile = session.device_profile();
    if profile.routing_profile_present() && profile.text_path().is_none() {
        return Err(TextUploadError::UnresolvedTextPath.into());
    }

    Ok(())
}

#[instrument(level = "trace", fields(?pacing))]
async fn apply_transport_pacing(pacing: UploadPacing) -> Result<(), ProtocolError> {
    match pacing {
        UploadPacing::None => Ok(()),
        UploadPacing::Delay { per_chunk } => {
            if !per_chunk.is_zero() {
                tokio::time::sleep(per_chunk).await;
            }
            Ok(())
        }
        UploadPacing::NotifyAck { timeout: _ } => Ok(()),
    }
}

#[instrument(skip(session), level = "trace", fields(?pacing))]
async fn apply_block_pacing(
    session: &DeviceSession,
    pacing: UploadPacing,
) -> Result<(), ProtocolError> {
    match pacing {
        UploadPacing::None | UploadPacing::Delay { .. } => Ok(()),
        UploadPacing::NotifyAck { timeout } => wait_for_notify_ack(session, timeout).await,
    }
}

#[instrument(skip(session), level = "trace", fields(timeout_ms = timeout_duration.as_millis()))]
async fn wait_for_notify_ack(
    session: &DeviceSession,
    timeout_duration: Duration,
) -> Result<(), ProtocolError> {
    tracing::trace!(
        timeout_ms = timeout_duration.as_millis(),
        "waiting for text-upload acknowledgement"
    );
    let mut decoded_event: Option<Result<NotifyEvent, NotificationDecodeError>> = None;
    let wait_result = timeout(
        timeout_duration,
        session.run_notifications(
            EndpointId::ReadNotifyCharacteristic,
            Some(1),
            |_index, event| {
                decoded_event = Some(event);
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
                NotifyEvent::NextPackage(TransferFamily::Text)
                | NotifyEvent::Finished(TransferFamily::Text) => Ok(()),
                _other => Err(TextUploadError::UnexpectedNotifyEvent.into()),
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct TextEncodingContext {
    text_path: TextPath,
    led_type: Option<u8>,
}

fn encoding_context(session: &DeviceSession) -> TextEncodingContext {
    let profile = session.device_profile();
    TextEncodingContext {
        text_path: profile.text_path().unwrap_or(TextPath::Path1616),
        led_type: profile.led_type(),
    }
}

fn build_upload_blocks(
    session: &DeviceSession,
    request: &TextUploadRequest,
) -> Result<Vec<Vec<u8>>, ProtocolError> {
    let context = encoding_context(session);
    let metadata = encode_metadata(&request.text, request.options, context)?;
    let glyph_stream = encode_glyph_stream(&request.text, request.options, context)?;

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

    let mut blocks = Vec::new();
    for (index, logical_chunk) in logical_payload.chunks(LOGICAL_CHUNK_SIZE).enumerate() {
        let chunk_len = u16::try_from(logical_chunk.len()).map_err(|_overflow| {
            FrameCodecError::HeaderPayloadTooLarge {
                payload_len: u16::MAX,
                max_payload_len: u16::MAX - 16,
            }
        })?;
        let header_fields = crate::TextHeaderFields::new(chunk_len, payload_len_u32, crc32)?;
        let mut header = FrameCodec::encode_text_header(header_fields);
        if index > 0 {
            header[4] = 0x02;
        }

        let mut block = Vec::with_capacity(header.len() + logical_chunk.len());
        block.extend_from_slice(&header);
        block.extend_from_slice(logical_chunk);
        blocks.push(block);
    }

    Ok(blocks)
}

fn encode_metadata(
    text: &str,
    options: TextOptions,
    context: TextEncodingContext,
) -> Result<[u8; METADATA_LEN], ProtocolError> {
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
    metadata[0..2].copy_from_slice(&char_count_u16.to_be_bytes());
    let (resolution_flag_1, resolution_flag_2) = text_path_resolution_flags(context.text_path);
    metadata[2] = resolution_flag_1;
    metadata[3] = resolution_flag_2;
    metadata[4] = adjusted_text_mode(options.text_mode, context.led_type);
    metadata[5] = options.speed;
    metadata[6] = options.text_colour_mode;
    let text_colour = guarded_text_colour(options.text_colour);
    metadata[7] = text_colour.r;
    metadata[8] = text_colour.g;
    metadata[9] = text_colour.b;
    metadata[10] = options.background_mode;
    metadata[11] = options.background_colour.r;
    metadata[12] = options.background_colour.g;
    metadata[13] = options.background_colour.b;
    Ok(metadata)
}

fn text_path_resolution_flags(text_path: TextPath) -> (u8, u8) {
    match text_path {
        TextPath::Path832 | TextPath::Path1664 => (0x00, 0x01),
        TextPath::Path1616 | TextPath::Path3232 | TextPath::Path6464 => (0x01, 0x01),
    }
}

fn adjusted_text_mode(text_mode: u8, led_type: Option<u8>) -> u8 {
    if led_type == Some(2) {
        text_mode.saturating_add(1)
    } else {
        text_mode
    }
}

fn guarded_text_colour(text_colour: Rgb) -> Rgb {
    if text_colour.r == 0x00 && text_colour.g == 0x00 && text_colour.b == 0x00 {
        Rgb::new(0x00, 0x00, 0x01)
    } else {
        text_colour
    }
}

fn encode_glyph_stream(
    text: &str,
    options: TextOptions,
    context: TextEncodingContext,
) -> Result<Vec<u8>, ProtocolError> {
    if text.is_empty() {
        return Err(TextUploadError::EmptyText.into());
    }

    let mut stream = Vec::new();
    for ch in text.chars() {
        stream.extend_from_slice(&encode_one_glyph(ch, options, context));
    }
    Ok(stream)
}

fn encode_one_glyph(ch: char, options: TextOptions, context: TextEncodingContext) -> Vec<u8> {
    match context.text_path {
        TextPath::Path832 => encode_832_glyph(ch),
        TextPath::Path1616 | TextPath::Path1664 => {
            encode_scaled_typed_glyph(ch, 0x02, 8, 16, 0x03, 16, 16)
        }
        TextPath::Path3232 => match normalised_font_size(options.font_size) {
            32 => encode_scaled_typed_glyph(ch, 0x05, 16, 32, 0x06, 32, 32),
            _ => encode_scaled_typed_glyph(ch, 0x02, 8, 16, 0x03, 16, 16),
        },
        TextPath::Path6464 => match normalised_font_size(options.font_size) {
            64 => encode_scaled_typed_glyph(ch, 0x07, 32, 64, 0x08, 64, 64),
            32 => encode_scaled_typed_glyph(ch, 0x05, 16, 32, 0x06, 32, 32),
            _ => encode_scaled_typed_glyph(ch, 0x02, 8, 16, 0x03, 16, 16),
        },
    }
}

fn normalised_font_size(font_size: u8) -> u8 {
    match font_size {
        32 => 32,
        64 => 64,
        _ => 16,
    }
}

fn encode_832_glyph(ch: char) -> Vec<u8> {
    if let Some(bitmap) = font_bitmap_exact(ch) {
        let mut glyph = Vec::with_capacity(4 + bitmap.len());
        glyph.extend_from_slice(&[0x04, 0xFF, 0xFF, 0xFF]);
        glyph.extend_from_slice(&bitmap);
        return glyph;
    }

    if is_wide_char(ch) {
        let mut glyph = Vec::with_capacity(4 + 24);
        glyph.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]);
        glyph.extend_from_slice(&encode_scaled_bitmap(ch, 16, 12));
        return glyph;
    }

    let mut glyph = Vec::with_capacity(4 + 8);
    glyph.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
    glyph.extend_from_slice(&encode_scaled_bitmap(ch, 8, 8));
    glyph
}

fn encode_scaled_typed_glyph(
    ch: char,
    ascii_tag: u8,
    ascii_width: usize,
    ascii_height: usize,
    wide_tag: u8,
    wide_width: usize,
    wide_height: usize,
) -> Vec<u8> {
    let is_wide = is_wide_char(ch);
    let (tag, width, height) = if is_wide {
        (wide_tag, wide_width, wide_height)
    } else {
        (ascii_tag, ascii_width, ascii_height)
    };

    let bitmap = encode_scaled_bitmap(ch, width, height);
    let mut glyph = Vec::with_capacity(4 + bitmap.len());
    glyph.extend_from_slice(&[tag, 0xFF, 0xFF, 0xFF]);
    glyph.extend_from_slice(&bitmap);
    glyph
}

fn encode_scaled_bitmap(ch: char, width: usize, height: usize) -> Vec<u8> {
    let source = font_bitmap_for(ch);
    let mut bitmap = vec![0u8; (width * height) / 8];

    for y in 0..height {
        let source_y = (y * FONT_BITMAP_HEIGHT) / height;
        let source_row = source[source_y];
        for x in 0..width {
            let source_x = (x * FONT_BITMAP_WIDTH) / width;
            let set = (source_row >> source_x) & 0x01 == 0x01;
            if !set {
                continue;
            }

            let bit_index = y * width + x;
            let byte_index = bit_index / 8;
            let bit_offset = bit_index % 8;
            bitmap[byte_index] |= 1 << bit_offset;
        }
    }

    bitmap
}

fn font_bitmap_exact(ch: char) -> Option<[u8; FONT_BITMAP_HEIGHT]> {
    font8x8::BASIC_FONTS.get(ch)
}

fn font_bitmap_for(ch: char) -> [u8; FONT_BITMAP_HEIGHT] {
    font_bitmap_exact(ch)
        .or_else(|| font_bitmap_exact('?'))
        .unwrap_or([0u8; FONT_BITMAP_HEIGHT])
}

fn is_wide_char(ch: char) -> bool {
    matches!(
        ch as u32,
        0x1100..=0x11FF
            | 0x2E80..=0x2FFF
            | 0x3000..=0x30FF
            | 0x3130..=0x318F
            | 0x31A0..=0x31BF
            | 0x31F0..=0x31FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xA960..=0xA97F
            | 0xAC00..=0xD7AF
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE4F
            | 0xFF00..=0xFFEF
    )
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::*;

    fn context(text_path: TextPath, led_type: Option<u8>) -> TextEncodingContext {
        TextEncodingContext {
            text_path,
            led_type,
        }
    }

    #[test]
    fn metadata_encodes_expected_default_fields() {
        let metadata = encode_metadata(
            "AB",
            TextOptions::default(),
            context(TextPath::Path1616, None),
        )
        .expect("metadata should encode");
        assert_eq!(
            [
                0x00, 0x02, 0x01, 0x01, 0x00, 0x20, 0x01, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00,
            ],
            metadata
        );
    }

    #[test]
    fn metadata_applies_led_type_mode_adjustment_and_colour_guard() {
        let options =
            TextOptions::new(0x00, 0x20, 0x01, Rgb::new(0, 0, 0), 0x00, Rgb::new(0, 0, 0));
        let metadata = encode_metadata("A", options, context(TextPath::Path832, Some(2)))
            .expect("metadata should encode");

        assert_eq!(0x01, metadata[4]);
        assert_eq!(0x00, metadata[7]);
        assert_eq!(0x00, metadata[8]);
        assert_eq!(0x01, metadata[9]);
    }

    #[test]
    fn metadata_rejects_empty_text() {
        let result = encode_metadata(
            "",
            TextOptions::default(),
            context(TextPath::Path1616, None),
        );
        assert_matches!(result, Err(ProtocolError::TextUpload(_)));
    }

    #[test]
    fn glyph_stream_path_1616_uses_expected_tag_and_length() {
        let stream = encode_glyph_stream(
            "A",
            TextOptions::default(),
            context(TextPath::Path1616, None),
        )
        .expect("glyph stream should encode");

        assert_eq!(&[0x02, 0xFF, 0xFF, 0xFF], &stream[0..4]);
        assert_eq!(4 + 16, stream.len());
    }

    #[test]
    fn glyph_stream_path_832_uses_compact_ascii_tag() {
        let stream = encode_glyph_stream(
            "A",
            TextOptions::default(),
            context(TextPath::Path832, Some(2)),
        )
        .expect("glyph stream should encode");

        assert_eq!(&[0x04, 0xFF, 0xFF, 0xFF], &stream[0..4]);
        assert_eq!(4 + 8, stream.len());
    }

    #[rstest]
    #[case('A', false)]
    #[case('?', false)]
    #[case('中', true)]
    #[case('あ', true)]
    #[case('한', true)]
    fn wide_char_detection_matches_expected(#[case] ch: char, #[case] expected: bool) {
        assert_eq!(expected, is_wide_char(ch));
    }
}
