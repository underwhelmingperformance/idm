use std::fmt;

use thiserror::Error;

const SHORT_FRAME_HEADER_LEN: usize = 4;
const SHORT_FRAME_MAX_PAYLOAD_LEN: usize = u16::MAX as usize - SHORT_FRAME_HEADER_LEN;
const HEADER_LEN: u16 = 16;
const HEADER_MAX_PAYLOAD_LEN: u16 = u16::MAX - HEADER_LEN;
const DIY_PREFIX_LEN: u16 = 9;
const DIY_PREFIX_MAX_PAYLOAD_LEN: u16 = u16::MAX - DIY_PREFIX_LEN;
const OTA_CHUNK_HEADER_LEN: u16 = 13;
const OTA_CHUNK_MAX_PAYLOAD_LEN: u16 = u16::MAX - OTA_CHUNK_HEADER_LEN;
const MEDIA_SLOT_NO_TIME_SIGNATURE: u8 = 12;
const MEDIA_SLOT_SHOW_NOW: u8 = 13;

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
    /// Material time-sign is outside the supported `0..=4` range.
    #[error("invalid material time-sign {value}; supported values are 0, 1, 2, 3, 4")]
    InvalidMaterialTimeSign { value: u8 },
    /// Timed media-tail mode was requested with the no-time-signature slot (`0x0C`).
    #[error(
        "invalid timed media slot {value}; slot 12 (0x0C) is no-time-signature and must use MediaHeaderTail::NoTimeSignature"
    )]
    InvalidTimedMediaSlot { value: u8 },
}

/// Stored material time-sign value used by media headers.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum MaterialTimeSign {
    /// 5-second display duration.
    FiveSeconds,
    /// 10-second display duration.
    TenSeconds,
    /// 30-second display duration.
    ThirtySeconds,
    /// 60-second display duration.
    SixtySeconds,
    /// 300-second display duration.
    ThreeHundredSeconds,
}

impl MaterialTimeSign {
    /// Returns the raw protocol value used by app settings.
    ///
    /// ```
    /// use idm::MaterialTimeSign;
    ///
    /// assert_eq!(0, MaterialTimeSign::FiveSeconds.as_raw());
    /// assert_eq!(4, MaterialTimeSign::ThreeHundredSeconds.as_raw());
    /// ```
    #[must_use]
    pub const fn as_raw(self) -> u8 {
        match self {
            Self::FiveSeconds => 0,
            Self::TenSeconds => 1,
            Self::ThirtySeconds => 2,
            Self::SixtySeconds => 3,
            Self::ThreeHundredSeconds => 4,
        }
    }

    /// Returns the converted material duration in seconds.
    ///
    /// This follows `DeviceMaterialTimeConvert.ConvertTime` from the official app.
    ///
    /// ```
    /// use idm::MaterialTimeSign;
    ///
    /// assert_eq!(5, MaterialTimeSign::FiveSeconds.duration_seconds());
    /// assert_eq!(300, MaterialTimeSign::ThreeHundredSeconds.duration_seconds());
    /// ```
    #[must_use]
    pub const fn duration_seconds(self) -> u16 {
        match self {
            Self::FiveSeconds => 5,
            Self::TenSeconds => 10,
            Self::ThirtySeconds => 30,
            Self::SixtySeconds => 60,
            Self::ThreeHundredSeconds => 300,
        }
    }
}

impl Default for MaterialTimeSign {
    fn default() -> Self {
        Self::FiveSeconds
    }
}

impl fmt::Display for MaterialTimeSign {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_raw())
    }
}

impl TryFrom<u8> for MaterialTimeSign {
    type Error = FrameCodecError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::FiveSeconds),
            1 => Ok(Self::TenSeconds),
            2 => Ok(Self::ThirtySeconds),
            3 => Ok(Self::SixtySeconds),
            4 => Ok(Self::ThreeHundredSeconds),
            _ => Err(FrameCodecError::InvalidMaterialTimeSign { value }),
        }
    }
}

/// Material slot/type byte encoded in media-header byte `15`.
#[derive(
    Debug, Clone, Copy, Eq, PartialEq, derive_more::Display, derive_more::From, derive_more::Into,
)]
#[display("{_0}")]
pub struct MaterialSlot(u8);

impl MaterialSlot {
    /// Slot value that disables time-sign encoding (`0x0C`).
    pub const NO_TIME_SIGNATURE: Self = Self(MEDIA_SLOT_NO_TIME_SIGNATURE);

    /// Slot value used by immediate "show now" uploads (`0x0D`).
    pub const SHOW_NOW: Self = Self(MEDIA_SLOT_SHOW_NOW);

    /// Creates a slot from a raw protocol byte.
    ///
    /// ```
    /// use idm::MaterialSlot;
    ///
    /// let slot = MaterialSlot::new(27);
    /// assert_eq!(27, slot.value());
    /// ```
    #[must_use]
    pub const fn new(value: u8) -> Self {
        Self(value)
    }

    /// Returns the raw slot byte.
    ///
    /// ```
    /// use idm::MaterialSlot;
    ///
    /// assert_eq!(12, MaterialSlot::NO_TIME_SIGNATURE.value());
    /// assert_eq!(13, MaterialSlot::SHOW_NOW.value());
    /// ```
    #[must_use]
    pub const fn value(self) -> u8 {
        self.0
    }

    /// Returns whether this slot should encode duration bytes.
    ///
    /// ```
    /// use idm::MaterialSlot;
    ///
    /// assert!(!MaterialSlot::NO_TIME_SIGNATURE.uses_time_signature());
    /// assert!(MaterialSlot::SHOW_NOW.uses_time_signature());
    /// ```
    #[must_use]
    pub const fn uses_time_signature(self) -> bool {
        self.0 != MEDIA_SLOT_NO_TIME_SIGNATURE
    }
}

impl Default for MaterialSlot {
    fn default() -> Self {
        Self::SHOW_NOW
    }
}

/// Material slot used by timed media tails.
///
/// This excludes slot `0x0C`, which is reserved for no-time-signature uploads.
#[derive(Debug, Clone, Copy, Eq, PartialEq, derive_more::Display, derive_more::Into)]
#[display("{_0}")]
pub struct TimedMaterialSlot(u8);

impl TimedMaterialSlot {
    /// Slot value used by immediate "show now" timed uploads (`0x0D`).
    pub const SHOW_NOW: Self = Self(MEDIA_SLOT_SHOW_NOW);

    /// Creates a timed media slot from a raw protocol byte.
    ///
    /// # Errors
    ///
    /// Returns an error when `value` is `0x0C` (`NO_TIME_SIGNATURE`).
    ///
    /// ```
    /// use idm::{FrameCodecError, TimedMaterialSlot};
    ///
    /// let slot = TimedMaterialSlot::new(0x2A)?;
    /// assert_eq!(0x2A, slot.value());
    ///
    /// let err = TimedMaterialSlot::new(0x0C).expect_err("0x0C is not valid for timed slots");
    /// assert!(matches!(err, FrameCodecError::InvalidTimedMediaSlot { value: 0x0C }));
    /// # Ok::<(), idm::FrameCodecError>(())
    /// ```
    pub fn new(value: u8) -> Result<Self, FrameCodecError> {
        if value == MEDIA_SLOT_NO_TIME_SIGNATURE {
            return Err(FrameCodecError::InvalidTimedMediaSlot { value });
        }
        Ok(Self(value))
    }

    /// Returns the raw slot byte.
    ///
    /// ```
    /// use idm::TimedMaterialSlot;
    ///
    /// let slot = TimedMaterialSlot::SHOW_NOW;
    /// assert_eq!(0x0D, slot.value());
    /// ```
    #[must_use]
    pub const fn value(self) -> u8 {
        self.0
    }
}

impl Default for TimedMaterialSlot {
    fn default() -> Self {
        Self::SHOW_NOW
    }
}

impl TryFrom<u8> for TimedMaterialSlot {
    type Error = FrameCodecError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<TimedMaterialSlot> for MaterialSlot {
    fn from(value: TimedMaterialSlot) -> Self {
        Self::new(value.value())
    }
}

/// Tail-byte policy for media headers (`bytes 13..15`).
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum MediaHeaderTail {
    /// No-time-signature mode (`0x0C`) with zeroed duration bytes.
    NoTimeSignature,
    /// Timed mode with explicit slot and duration metadata.
    Timed {
        /// Timed material slot (non-`0x0C`).
        slot: TimedMaterialSlot,
        /// Material time-sign used for duration conversion.
        time_sign: MaterialTimeSign,
    },
}

impl MediaHeaderTail {
    /// Creates a timed media-header tail policy.
    ///
    /// ```
    /// use idm::{MaterialTimeSign, MediaHeaderTail, TimedMaterialSlot};
    ///
    /// let tail = MediaHeaderTail::timed(TimedMaterialSlot::SHOW_NOW, MaterialTimeSign::TenSeconds);
    /// assert_eq!([10, 0, 13], tail.bytes());
    /// ```
    #[must_use]
    pub const fn timed(slot: TimedMaterialSlot, time_sign: MaterialTimeSign) -> Self {
        Self::Timed { slot, time_sign }
    }

    /// Returns the configured slot.
    ///
    /// ```
    /// use idm::{MaterialSlot, MediaHeaderTail};
    ///
    /// let tail = MediaHeaderTail::default();
    /// assert_eq!(MaterialSlot::NO_TIME_SIGNATURE, tail.slot());
    /// ```
    #[must_use]
    pub const fn slot(self) -> MaterialSlot {
        match self {
            Self::NoTimeSignature => MaterialSlot::NO_TIME_SIGNATURE,
            Self::Timed { slot, .. } => MaterialSlot::new(slot.value()),
        }
    }

    /// Returns the configured time-sign value.
    ///
    /// ```
    /// use idm::{MaterialTimeSign, MediaHeaderTail};
    ///
    /// let tail = MediaHeaderTail::default();
    /// assert_eq!(None, tail.time_sign());
    ///
    /// let no_time = MediaHeaderTail::NoTimeSignature;
    /// assert_eq!(None, no_time.time_sign());
    /// ```
    #[must_use]
    pub const fn time_sign(self) -> Option<MaterialTimeSign> {
        match self {
            Self::NoTimeSignature => None,
            Self::Timed { time_sign, .. } => Some(time_sign),
        }
    }

    /// Returns encoded media-tail bytes `[13, 14, 15]`.
    ///
    /// ```
    /// use idm::{MaterialTimeSign, MediaHeaderTail, TimedMaterialSlot};
    ///
    /// let timed = MediaHeaderTail::timed(TimedMaterialSlot::SHOW_NOW, MaterialTimeSign::ThirtySeconds);
    /// assert_eq!([30, 0, 13], timed.bytes());
    ///
    /// let no_time = MediaHeaderTail::NoTimeSignature;
    /// assert_eq!([0, 0, 12], no_time.bytes());
    /// ```
    #[must_use]
    pub const fn bytes(self) -> [u8; 3] {
        match self {
            Self::NoTimeSignature => [0x00, 0x00, MEDIA_SLOT_NO_TIME_SIGNATURE],
            Self::Timed { slot, time_sign } => {
                let duration = time_sign.duration_seconds();
                let duration_bytes = duration.to_le_bytes();
                [duration_bytes[0], duration_bytes[1], slot.value()]
            }
        }
    }

    /// Applies this tail policy to media header bytes `13..15`.
    ///
    /// ```
    /// use idm::{MaterialTimeSign, MediaHeaderTail, TimedMaterialSlot};
    ///
    /// let mut header = [0_u8; 16];
    /// MediaHeaderTail::timed(TimedMaterialSlot::SHOW_NOW, MaterialTimeSign::SixtySeconds)
    ///     .apply_to_header(&mut header);
    /// assert_eq!([60, 0, 13], [header[13], header[14], header[15]]);
    /// ```
    pub fn apply_to_header(self, header: &mut [u8; 16]) {
        let tail = self.bytes();
        header[13] = tail[0];
        header[14] = tail[1];
        header[15] = tail[2];
    }
}

impl Default for MediaHeaderTail {
    fn default() -> Self {
        Self::NoTimeSignature
    }
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

/// Fields used when encoding an image upload header.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct ImageHeaderFields {
    chunk_payload_len: u16,
    chunk_flag: GifChunkFlag,
    payload_len: u32,
    crc32: u32,
}

impl ImageHeaderFields {
    /// Creates image-header fields.
    ///
    /// # Errors
    ///
    /// Returns an error when `chunk_payload_len` cannot fit in a 16-byte framed block.
    ///
    /// ```
    /// use idm::{GifChunkFlag, ImageHeaderFields};
    ///
    /// let fields = ImageHeaderFields::new(4096, GifChunkFlag::First, 8192, 0xDEADBEEF)?;
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
    /// use idm::{GifChunkFlag, ImageHeaderFields};
    ///
    /// let fields = ImageHeaderFields::new(2048, GifChunkFlag::Continuation, 4096, 0)?;
    /// assert_eq!(2048, fields.chunk_payload_len());
    /// # Ok::<(), idm::FrameCodecError>(())
    /// ```
    #[must_use]
    pub fn chunk_payload_len(&self) -> u16 {
        self.chunk_payload_len
    }
}

/// Fields used when encoding a DIY 9-byte transfer prefix.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct DiyPrefixFields {
    chunk_payload_len: u16,
    chunk_flag: GifChunkFlag,
    payload_len: u32,
}

impl DiyPrefixFields {
    /// Creates DIY-prefix fields.
    ///
    /// # Errors
    ///
    /// Returns an error when `chunk_payload_len` cannot fit in a 9-byte framed block.
    ///
    /// ```
    /// use idm::{DiyPrefixFields, GifChunkFlag};
    ///
    /// let fields = DiyPrefixFields::new(4096, GifChunkFlag::First, 8192)?;
    /// assert_eq!(4096, fields.chunk_payload_len());
    /// # Ok::<(), idm::FrameCodecError>(())
    /// ```
    pub fn new(
        chunk_payload_len: u16,
        chunk_flag: GifChunkFlag,
        payload_len: u32,
    ) -> Result<Self, FrameCodecError> {
        if chunk_payload_len > DIY_PREFIX_MAX_PAYLOAD_LEN {
            return Err(FrameCodecError::HeaderPayloadTooLarge {
                payload_len: chunk_payload_len,
                max_payload_len: DIY_PREFIX_MAX_PAYLOAD_LEN,
            });
        }

        Ok(Self {
            chunk_payload_len,
            chunk_flag,
            payload_len,
        })
    }

    /// Returns the payload byte count for this frame block.
    ///
    /// ```
    /// use idm::{DiyPrefixFields, GifChunkFlag};
    ///
    /// let fields = DiyPrefixFields::new(1024, GifChunkFlag::Continuation, 4096)?;
    /// assert_eq!(1024, fields.chunk_payload_len());
    /// # Ok::<(), idm::FrameCodecError>(())
    /// ```
    #[must_use]
    pub fn chunk_payload_len(&self) -> u16 {
        self.chunk_payload_len
    }
}

/// Fields used when encoding an OTA 13-byte chunk header.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct OtaChunkHeaderFields {
    package_index: u8,
    chunk_crc32: u32,
    chunk_len: u32,
}

impl OtaChunkHeaderFields {
    /// Creates OTA chunk-header fields.
    ///
    /// # Errors
    ///
    /// Returns an error when `chunk_len` cannot fit in a 13-byte framed block.
    ///
    /// ```
    /// use idm::OtaChunkHeaderFields;
    ///
    /// let fields = OtaChunkHeaderFields::new(2, 0x11223344, 4096)?;
    /// assert_eq!(2, fields.package_index());
    /// # Ok::<(), idm::FrameCodecError>(())
    /// ```
    pub fn new(
        package_index: u8,
        chunk_crc32: u32,
        chunk_len: u32,
    ) -> Result<Self, FrameCodecError> {
        let chunk_payload_len = u16::try_from(chunk_len).map_err(|_overflow| {
            FrameCodecError::HeaderPayloadTooLarge {
                payload_len: u16::MAX,
                max_payload_len: OTA_CHUNK_MAX_PAYLOAD_LEN,
            }
        })?;
        if chunk_payload_len > OTA_CHUNK_MAX_PAYLOAD_LEN {
            return Err(FrameCodecError::HeaderPayloadTooLarge {
                payload_len: chunk_payload_len,
                max_payload_len: OTA_CHUNK_MAX_PAYLOAD_LEN,
            });
        }

        Ok(Self {
            package_index,
            chunk_crc32,
            chunk_len,
        })
    }

    /// Returns the OTA package index.
    ///
    /// ```
    /// use idm::OtaChunkHeaderFields;
    ///
    /// let fields = OtaChunkHeaderFields::new(3, 0xAABBCCDD, 1024)?;
    /// assert_eq!(3, fields.package_index());
    /// # Ok::<(), idm::FrameCodecError>(())
    /// ```
    #[must_use]
    pub fn package_index(&self) -> u8 {
        self.package_index
    }

    /// Returns the current chunk length.
    ///
    /// ```
    /// use idm::OtaChunkHeaderFields;
    ///
    /// let fields = OtaChunkHeaderFields::new(3, 0xAABBCCDD, 1024)?;
    /// assert_eq!(1024, fields.chunk_len());
    /// # Ok::<(), idm::FrameCodecError>(())
    /// ```
    #[must_use]
    pub fn chunk_len(&self) -> u32 {
        self.chunk_len
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
    ///     [0xC9, 0x08, 0x01, 0x00, 0x02, 0xB9, 0x18, 0x00, 0x00, 0xDB, 0x42, 0xCB, 0x14, 0x00, 0x00, 0x0C],
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
        header[13] = 0x00;
        header[14] = 0x00;
        header[15] = MEDIA_SLOT_NO_TIME_SIGNATURE;
        header
    }

    /// Encodes a 16-byte image header.
    ///
    /// ```
    /// use idm::{FrameCodec, GifChunkFlag, ImageHeaderFields};
    ///
    /// let fields = ImageHeaderFields::new(0x1000, GifChunkFlag::Continuation, 0x0000_2000, 0x1122_3344)?;
    /// let header = FrameCodec::encode_image_header(fields);
    /// assert_eq!(
    ///     [0x10, 0x10, 0x02, 0x00, 0x02, 0x00, 0x20, 0x00, 0x00, 0x44, 0x33, 0x22, 0x11, 0x00, 0x00, 0x0C],
    ///     header
    /// );
    /// # Ok::<(), idm::FrameCodecError>(())
    /// ```
    #[must_use]
    pub fn encode_image_header(fields: ImageHeaderFields) -> [u8; 16] {
        let mut header = [0u8; 16];
        let block_len = HEADER_LEN + fields.chunk_payload_len;

        header[0..2].copy_from_slice(&block_len.to_le_bytes());
        header[2] = 0x02;
        header[3] = 0x00;
        header[4] = fields.chunk_flag.as_protocol_byte();
        header[5..9].copy_from_slice(&fields.payload_len.to_le_bytes());
        header[9..13].copy_from_slice(&fields.crc32.to_le_bytes());
        header[13] = 0x00;
        header[14] = 0x00;
        header[15] = MEDIA_SLOT_NO_TIME_SIGNATURE;
        header
    }

    /// Encodes a 9-byte DIY upload prefix.
    ///
    /// ```
    /// use idm::{DiyPrefixFields, FrameCodec, GifChunkFlag};
    ///
    /// let fields = DiyPrefixFields::new(0x1000, GifChunkFlag::Continuation, 0x0000_18B9)?;
    /// let prefix = FrameCodec::encode_diy_prefix(fields);
    /// assert_eq!(
    ///     [0x09, 0x10, 0x00, 0x00, 0x02, 0xB9, 0x18, 0x00, 0x00],
    ///     prefix
    /// );
    /// # Ok::<(), idm::FrameCodecError>(())
    /// ```
    #[must_use]
    pub fn encode_diy_prefix(fields: DiyPrefixFields) -> [u8; 9] {
        let mut prefix = [0u8; 9];
        let block_len = DIY_PREFIX_LEN + fields.chunk_payload_len;

        prefix[0..2].copy_from_slice(&block_len.to_le_bytes());
        prefix[2] = 0x00;
        prefix[3] = 0x00;
        prefix[4] = fields.chunk_flag.as_protocol_byte();
        prefix[5..9].copy_from_slice(&fields.payload_len.to_le_bytes());
        prefix
    }

    /// Encodes a 13-byte OTA chunk header.
    ///
    /// ```
    /// use idm::{FrameCodec, OtaChunkHeaderFields};
    ///
    /// let fields = OtaChunkHeaderFields::new(0x02, 0x1122_3344, 0x1000)?;
    /// let header = FrameCodec::encode_ota_chunk_header(fields);
    /// assert_eq!(
    ///     [0x0D, 0x10, 0x01, 0xC0, 0x02, 0x44, 0x33, 0x22, 0x11, 0x00, 0x10, 0x00, 0x00],
    ///     header
    /// );
    /// # Ok::<(), idm::FrameCodecError>(())
    /// ```
    #[must_use]
    pub fn encode_ota_chunk_header(fields: OtaChunkHeaderFields) -> [u8; 13] {
        let mut header = [0u8; 13];
        let chunk_payload_len = u16::try_from(fields.chunk_len)
            .expect("chunk_len is validated by OtaChunkHeaderFields::new");
        let block_len = OTA_CHUNK_HEADER_LEN + chunk_payload_len;

        header[0..2].copy_from_slice(&block_len.to_le_bytes());
        header[2] = 0x01;
        header[3] = 0xC0;
        header[4] = fields.package_index;
        header[5..9].copy_from_slice(&fields.chunk_crc32.to_le_bytes());
        header[9..13].copy_from_slice(&fields.chunk_len.to_le_bytes());
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
                0xC9, 0x08, 0x01, 0x00, 0x02, 0xB9, 0x18, 0x00, 0x00, 0xDB, 0x42, 0xCB, 0x14, 0x00,
                0x00, 0x0C,
            ],
            header
        );
    }

    #[test]
    fn encode_image_header_matches_expected_bytes() {
        let fields =
            ImageHeaderFields::new(0x1000, GifChunkFlag::Continuation, 0x0000_2000, 0x1122_3344)
                .expect("captured image values should construct");
        let header = FrameCodec::encode_image_header(fields);
        assert_eq!(
            [
                0x10, 0x10, 0x02, 0x00, 0x02, 0x00, 0x20, 0x00, 0x00, 0x44, 0x33, 0x22, 0x11, 0x00,
                0x00, 0x0C,
            ],
            header
        );
    }

    #[test]
    fn encode_diy_prefix_matches_expected_bytes() {
        let fields = DiyPrefixFields::new(0x1000, GifChunkFlag::Continuation, 0x0000_18B9)
            .expect("captured DIY values should construct");
        let prefix = FrameCodec::encode_diy_prefix(fields);
        assert_eq!(
            [0x09, 0x10, 0x00, 0x00, 0x02, 0xB9, 0x18, 0x00, 0x00],
            prefix
        );
    }

    #[test]
    fn encode_ota_chunk_header_matches_expected_bytes() {
        let fields = OtaChunkHeaderFields::new(0x02, 0x1122_3344, 0x1000)
            .expect("captured OTA values should construct");
        let header = FrameCodec::encode_ota_chunk_header(fields);
        assert_eq!(
            [
                0x0D, 0x10, 0x01, 0xC0, 0x02, 0x44, 0x33, 0x22, 0x11, 0x00, 0x10, 0x00, 0x00
            ],
            header
        );
    }

    #[test]
    fn ota_header_fields_reject_oversized_chunk_len() {
        let result = OtaChunkHeaderFields::new(0x01, 0xDEAD_BEEF, u32::from(u16::MAX));
        assert_matches!(
            result,
            Err(FrameCodecError::HeaderPayloadTooLarge {
                payload_len: u16::MAX,
                max_payload_len: OTA_CHUNK_MAX_PAYLOAD_LEN
            })
        );
    }
}
