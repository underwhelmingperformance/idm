use thiserror::Error;

const SHORT_FRAME_HEADER_LEN: usize = 4;
const SHORT_FRAME_MAX_PAYLOAD_LEN: usize = u16::MAX as usize - SHORT_FRAME_HEADER_LEN;
const HEADER_LEN: u16 = 16;
const HEADER_MAX_PAYLOAD_LEN: u16 = u16::MAX - HEADER_LEN;

/// Errors returned by frame encoding and decoding.
#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum FrameCodecError {
    /// The frame has fewer than the mandatory 4 short-frame header bytes.
    #[error("short frame is too short: expected at least 4 bytes, got {actual}")]
    ShortFrameTooShort { actual: usize },
    /// The declared frame length does not match the provided byte slice length.
    #[error("short frame length mismatch: declared {declared} bytes but frame has {actual} bytes")]
    ShortFrameLengthMismatch { declared: usize, actual: usize },
    /// The payload is too large to fit in a 16-bit short-frame length field.
    #[error("short frame payload is too large: {payload_len} bytes exceeds max {max_payload_len}")]
    ShortFramePayloadTooLarge {
        payload_len: usize,
        max_payload_len: usize,
    },
    /// The payload is too large to fit inside a 16-byte headered block.
    #[error("header payload is too large: {payload_len} bytes exceeds max {max_payload_len}")]
    HeaderPayloadTooLarge {
        payload_len: u16,
        max_payload_len: u16,
    },
}

/// Decoded short control frame.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ShortFrame<'a> {
    command_id: u8,
    command_ns: u8,
    payload: &'a [u8],
}

impl ShortFrame<'_> {
    /// Returns the command identifier byte.
    ///
    /// ```
    /// use idm::FrameCodec;
    ///
    /// let frame = FrameCodec::decode_short(&[0x05, 0x00, 0x07, 0x01, 0x01])?;
    /// assert_eq!(0x07, frame.command_id());
    /// # Ok::<(), idm::FrameCodecError>(())
    /// ```
    #[must_use]
    pub fn command_id(&self) -> u8 {
        self.command_id
    }

    /// Returns the command namespace byte.
    ///
    /// ```
    /// use idm::FrameCodec;
    ///
    /// let frame = FrameCodec::decode_short(&[0x05, 0x00, 0x07, 0x01, 0x01])?;
    /// assert_eq!(0x01, frame.command_ns());
    /// # Ok::<(), idm::FrameCodecError>(())
    /// ```
    #[must_use]
    pub fn command_ns(&self) -> u8 {
        self.command_ns
    }

    /// Returns the decoded payload bytes.
    ///
    /// ```
    /// use idm::FrameCodec;
    ///
    /// let frame = FrameCodec::decode_short(&[0x05, 0x00, 0x07, 0x01, 0x01])?;
    /// assert_eq!(&[0x01], frame.payload());
    /// # Ok::<(), idm::FrameCodecError>(())
    /// ```
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        self.payload
    }
}

/// Fields used when encoding a text upload header.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct TextHeaderFields {
    chunk_payload_len: u16,
    payload_len: u32,
    crc32: u32,
}

impl TextHeaderFields {
    /// Creates text-header fields.
    ///
    /// # Errors
    ///
    /// Returns an error when `chunk_payload_len` cannot fit in a 16-byte framed block.
    ///
    /// ```
    /// use idm::TextHeaderFields;
    ///
    /// let fields = TextHeaderFields::new(14, 14, 0x11223344)?;
    /// assert_eq!(14, fields.chunk_payload_len());
    /// # Ok::<(), idm::FrameCodecError>(())
    /// ```
    pub fn new(
        chunk_payload_len: u16,
        payload_len: u32,
        crc32: u32,
    ) -> Result<Self, FrameCodecError> {
        if chunk_payload_len > HEADER_MAX_PAYLOAD_LEN {
            return Err(FrameCodecError::HeaderPayloadTooLarge {
                payload_len: chunk_payload_len,
                max_payload_len: HEADER_MAX_PAYLOAD_LEN,
            });
        }

        Ok(Self {
            chunk_payload_len,
            payload_len,
            crc32,
        })
    }

    /// Returns the payload byte count for this frame block.
    ///
    /// ```
    /// use idm::TextHeaderFields;
    ///
    /// let fields = TextHeaderFields::new(32, 128, 0)?;
    /// assert_eq!(32, fields.chunk_payload_len());
    /// # Ok::<(), idm::FrameCodecError>(())
    /// ```
    #[must_use]
    pub fn chunk_payload_len(&self) -> u16 {
        self.chunk_payload_len
    }
}

/// Chunk flag used in GIF headers.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum GifChunkFlag {
    /// First chunk in an upload.
    First,
    /// Continuation chunk in an upload.
    Continuation,
}

impl GifChunkFlag {
    fn as_protocol_byte(self) -> u8 {
        match self {
            Self::First => 0x00,
            Self::Continuation => 0x02,
        }
    }
}

/// Fields used when encoding a GIF upload header.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct GifHeaderFields {
    chunk_payload_len: u16,
    chunk_flag: GifChunkFlag,
    payload_len: u32,
    crc32: u32,
}

impl GifHeaderFields {
    /// Creates GIF-header fields.
    ///
    /// # Errors
    ///
    /// Returns an error when `chunk_payload_len` cannot fit in a 16-byte framed block.
    ///
    /// ```
    /// use idm::{GifChunkFlag, GifHeaderFields};
    ///
    /// let fields = GifHeaderFields::new(4096, GifChunkFlag::First, 8192, 0xDEADBEEF)?;
    /// assert_eq!(4096, fields.chunk_payload_len());
    /// # Ok::<(), idm::FrameCodecError>(())
    /// ```
    pub fn new(
        chunk_payload_len: u16,
        chunk_flag: GifChunkFlag,
        payload_len: u32,
        crc32: u32,
    ) -> Result<Self, FrameCodecError> {
        if chunk_payload_len > HEADER_MAX_PAYLOAD_LEN {
            return Err(FrameCodecError::HeaderPayloadTooLarge {
                payload_len: chunk_payload_len,
                max_payload_len: HEADER_MAX_PAYLOAD_LEN,
            });
        }

        Ok(Self {
            chunk_payload_len,
            chunk_flag,
            payload_len,
            crc32,
        })
    }

    /// Returns the payload byte count for this frame block.
    ///
    /// ```
    /// use idm::{GifChunkFlag, GifHeaderFields};
    ///
    /// let fields = GifHeaderFields::new(2048, GifChunkFlag::Continuation, 4096, 0)?;
    /// assert_eq!(2048, fields.chunk_payload_len());
    /// # Ok::<(), idm::FrameCodecError>(())
    /// ```
    #[must_use]
    pub fn chunk_payload_len(&self) -> u16 {
        self.chunk_payload_len
    }
}

/// Encodes and decodes iDotMatrix protocol frames.
pub struct FrameCodec;

impl FrameCodec {
    /// Encodes a short control frame.
    ///
    /// # Errors
    ///
    /// Returns an error when `payload` is too large to fit in a 16-bit frame length.
    ///
    /// ```
    /// use idm::FrameCodec;
    ///
    /// let frame = FrameCodec::encode_short(0x07, 0x01, &[0x01])?;
    /// assert_eq!(vec![0x05, 0x00, 0x07, 0x01, 0x01], frame);
    /// # Ok::<(), idm::FrameCodecError>(())
    /// ```
    pub fn encode_short(
        command_id: u8,
        command_ns: u8,
        payload: &[u8],
    ) -> Result<Vec<u8>, FrameCodecError> {
        if payload.len() > SHORT_FRAME_MAX_PAYLOAD_LEN {
            return Err(FrameCodecError::ShortFramePayloadTooLarge {
                payload_len: payload.len(),
                max_payload_len: SHORT_FRAME_MAX_PAYLOAD_LEN,
            });
        }

        let frame_len = SHORT_FRAME_HEADER_LEN + payload.len();
        let frame_len_u16 = u16::try_from(frame_len).map_err(|_overflow| {
            FrameCodecError::ShortFramePayloadTooLarge {
                payload_len: payload.len(),
                max_payload_len: SHORT_FRAME_MAX_PAYLOAD_LEN,
            }
        })?;

        let mut frame = Vec::with_capacity(frame_len);
        frame.extend_from_slice(&frame_len_u16.to_le_bytes());
        frame.push(command_id);
        frame.push(command_ns);
        frame.extend_from_slice(payload);
        Ok(frame)
    }

    /// Decodes and validates a short control frame.
    ///
    /// # Errors
    ///
    /// Returns an error when the frame is shorter than 4 bytes or declares a
    /// different length than the provided byte slice.
    ///
    /// ```
    /// use idm::FrameCodec;
    ///
    /// let frame = FrameCodec::decode_short(&[0x05, 0x00, 0x07, 0x01, 0x01])?;
    /// assert_eq!(0x07, frame.command_id());
    /// # Ok::<(), idm::FrameCodecError>(())
    /// ```
    pub fn decode_short(frame: &[u8]) -> Result<ShortFrame<'_>, FrameCodecError> {
        if frame.len() < SHORT_FRAME_HEADER_LEN {
            return Err(FrameCodecError::ShortFrameTooShort {
                actual: frame.len(),
            });
        }

        let declared = usize::from(u16::from_le_bytes([frame[0], frame[1]]));
        let actual = frame.len();
        if declared != actual {
            return Err(FrameCodecError::ShortFrameLengthMismatch { declared, actual });
        }

        Ok(ShortFrame {
            command_id: frame[2],
            command_ns: frame[3],
            payload: &frame[4..],
        })
    }

    /// Encodes a 16-byte text header.
    ///
    /// ```
    /// use idm::{FrameCodec, TextHeaderFields};
    ///
    /// let fields = TextHeaderFields::new(14, 14, 0x11223344)?;
    /// let header = FrameCodec::encode_text_header(fields);
    /// assert_eq!(
    ///     [0x1E, 0x00, 0x03, 0x00, 0x00, 0x0E, 0x00, 0x00, 0x00, 0x44, 0x33, 0x22, 0x11, 0x00, 0x00, 0x0C],
    ///     header
    /// );
    /// # Ok::<(), idm::FrameCodecError>(())
    /// ```
    #[must_use]
    pub fn encode_text_header(fields: TextHeaderFields) -> [u8; 16] {
        let mut header = [0u8; 16];
        let block_len = HEADER_LEN + fields.chunk_payload_len;

        header[0..2].copy_from_slice(&block_len.to_le_bytes());
        header[2] = 0x03;
        header[3] = 0x00;
        header[4] = 0x00;
        header[5..9].copy_from_slice(&fields.payload_len.to_le_bytes());
        header[9..13].copy_from_slice(&fields.crc32.to_le_bytes());
        header[13] = 0x00;
        header[14] = 0x00;
        header[15] = 0x0C;
        header
    }

    /// Encodes a 16-byte GIF header.
    ///
    /// ```
    /// use idm::{FrameCodec, GifChunkFlag, GifHeaderFields};
    ///
    /// let fields = GifHeaderFields::new(0x08B9, GifChunkFlag::Continuation, 0x0000_18B9, 0x14CB_42DB)?;
    /// let header = FrameCodec::encode_gif_header(fields);
    /// assert_eq!(
    ///     [0xC9, 0x08, 0x01, 0x00, 0x02, 0xB9, 0x18, 0x00, 0x00, 0xDB, 0x42, 0xCB, 0x14, 0x05, 0x00, 0x0D],
    ///     header
    /// );
    /// # Ok::<(), idm::FrameCodecError>(())
    /// ```
    #[must_use]
    pub fn encode_gif_header(fields: GifHeaderFields) -> [u8; 16] {
        let mut header = [0u8; 16];
        let block_len = HEADER_LEN + fields.chunk_payload_len;

        header[0..2].copy_from_slice(&block_len.to_le_bytes());
        header[2] = 0x01;
        header[3] = 0x00;
        header[4] = fields.chunk_flag.as_protocol_byte();
        header[5..9].copy_from_slice(&fields.payload_len.to_le_bytes());
        header[9..13].copy_from_slice(&fields.crc32.to_le_bytes());
        header[13] = 0x05;
        header[14] = 0x00;
        header[15] = 0x0D;
        header
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::*;

    #[test]
    fn encode_short_writes_length_and_payload() {
        let frame =
            FrameCodec::encode_short(0x07, 0x01, &[0x01]).expect("small short frame should encode");
        assert_eq!(vec![0x05, 0x00, 0x07, 0x01, 0x01], frame);
    }

    #[test]
    fn encode_short_rejects_oversized_payload() {
        let payload = vec![0x00; SHORT_FRAME_MAX_PAYLOAD_LEN + 1];
        let result = FrameCodec::encode_short(0x00, 0x00, &payload);
        assert_matches!(
            result,
            Err(FrameCodecError::ShortFramePayloadTooLarge {
                payload_len,
                max_payload_len: SHORT_FRAME_MAX_PAYLOAD_LEN,
            }) if payload_len == SHORT_FRAME_MAX_PAYLOAD_LEN + 1
        );
    }

    #[test]
    fn decode_short_rejects_short_input() {
        let result = FrameCodec::decode_short(&[0x01, 0x00, 0x07]);
        assert_matches!(
            result,
            Err(FrameCodecError::ShortFrameTooShort { actual: 3 })
        );
    }

    #[test]
    fn decode_short_rejects_length_mismatch() {
        let result = FrameCodec::decode_short(&[0x05, 0x00, 0x07, 0x01]);
        assert_matches!(
            result,
            Err(FrameCodecError::ShortFrameLengthMismatch {
                declared: 5,
                actual: 4,
            })
        );
    }

    #[test]
    fn decode_short_returns_fields() {
        let frame = FrameCodec::decode_short(&[0x05, 0x00, 0x07, 0x01, 0x01])
            .expect("well-formed short frame should decode");
        assert_eq!(0x07, frame.command_id());
        assert_eq!(0x01, frame.command_ns());
        assert_eq!(&[0x01], frame.payload());
    }

    #[test]
    fn encode_text_header_matches_expected_bytes() {
        let fields = TextHeaderFields::new(14, 14, 0x1122_3344)
            .expect("valid text header fields should construct");
        let header = FrameCodec::encode_text_header(fields);
        assert_eq!(
            [
                0x1E, 0x00, 0x03, 0x00, 0x00, 0x0E, 0x00, 0x00, 0x00, 0x44, 0x33, 0x22, 0x11, 0x00,
                0x00, 0x0C,
            ],
            header
        );
    }

    #[rstest]
    #[case(GifChunkFlag::First, [0x00])]
    #[case(GifChunkFlag::Continuation, [0x02])]
    fn encode_gif_header_sets_chunk_flag(#[case] flag: GifChunkFlag, #[case] expected: [u8; 1]) {
        let fields =
            GifHeaderFields::new(1, flag, 1, 0).expect("valid gif header fields should construct");
        let header = FrameCodec::encode_gif_header(fields);
        assert_eq!(expected[0], header[4]);
    }

    #[test]
    fn encode_gif_header_matches_captured_example() {
        let fields =
            GifHeaderFields::new(0x08B9, GifChunkFlag::Continuation, 0x0000_18B9, 0x14CB_42DB)
                .expect("captured values should construct");
        let header = FrameCodec::encode_gif_header(fields);
        assert_eq!(
            [
                0xC9, 0x08, 0x01, 0x00, 0x02, 0xB9, 0x18, 0x00, 0x00, 0xDB, 0x42, 0xCB, 0x14, 0x05,
                0x00, 0x0D,
            ],
            header
        );
    }
}
