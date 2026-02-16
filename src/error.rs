use derive_more::From;
use thiserror::Error;

use crate::handlers::{BrightnessError, FrameCodecError, TextUploadError};
use crate::notification::NotificationDecodeError;
use crate::protocol::{EndpointId, endpoint_metadata};

/// Errors returned by BLE interaction operations.
#[derive(Debug, Error)]
pub enum InteractionError {
    #[error("BLE operation failed")]
    Ble(#[from] btleplug::Error),
    #[error("no BLE adapters were found")]
    NoAdapters,
    #[error("no iDotMatrix device matching `{prefix}*` was found in the fake fixture")]
    NoMatchingFixtureDevice { prefix: String },
    #[error(
        "required endpoint `{name}` ({uuid}) was not found on the connected device",
        name = endpoint_metadata(*endpoint).name(),
        uuid = endpoint_metadata(*endpoint).uuid()
    )]
    MissingEndpoint { endpoint: EndpointId },
    #[error("required iDotMatrix endpoints are missing: {missing}")]
    MissingRequiredEndpoints { missing: String },
    #[error("failed while waiting for Ctrl+C")]
    CtrlC { source: std::io::Error },
    #[error("failed while reading or writing model overrides")]
    ModelOverrideIo { source: std::io::Error },
    #[error("invalid persisted model-override record: `{record}`")]
    InvalidModelOverrideRecord { record: String },
    #[error("invalid LED type override value `{value}`")]
    InvalidLedTypeOverride { value: u8 },
    #[error(
        "ambiguous model shape `{shape}` for device `{device_id}` is unresolved; pass --model-led-type or persist a choice in the model-overrides file"
    )]
    AmbiguousShapeSelectionRequired { device_id: String, shape: i8 },
    #[error(transparent)]
    Fixture(#[from] FixtureError),
}

/// Errors returned when parsing fake interaction fixtures.
#[derive(Debug, Error)]
pub enum FixtureError {
    #[error("the fake discovery fixture is empty")]
    EmptyFixture,
    #[error("fixture records must contain four pipe-delimited fields")]
    InvalidRecordFieldCount,
    #[error("fixture records cannot contain empty mandatory fields")]
    EmptyRecordField,
    #[error("failed to parse RSSI value")]
    InvalidRssi(#[from] std::num::ParseIntError),
    #[error("hex payload length must be even")]
    InvalidHexLength,
    #[error("hex payload contains invalid byte `{value}`")]
    InvalidHexByte { value: String },
    #[error("scan model payload is not a valid iDotMatrix manufacturer payload")]
    InvalidScanModelPayload,
}

/// Errors returned when validating runtime backend options.
#[derive(Debug, Error)]
pub(crate) enum CliConfigError {
    #[error("missing fake scan fixture while fake mode is enabled")]
    MissingFakeScanFixture,
}

/// Errors returned by telemetry initialisation.
#[derive(Debug, Error)]
pub(crate) enum TelemetryError {
    #[error("failed to install tracing subscriber")]
    Subscriber(#[from] tracing_subscriber::util::TryInitError),
}

/// Top-level protocol errors wrapping module-specific error types.
#[derive(Debug, Error, From)]
pub enum ProtocolError {
    #[error(transparent)]
    #[from(NotificationDecodeError, Box<NotificationDecodeError>)]
    Notification(Box<NotificationDecodeError>),
    #[error(transparent)]
    #[from(FrameCodecError, Box<FrameCodecError>)]
    FrameCodec(Box<FrameCodecError>),
    #[error(transparent)]
    #[from(BrightnessError, Box<BrightnessError>)]
    Brightness(Box<BrightnessError>),
    #[error(transparent)]
    #[from(TextUploadError, Box<TextUploadError>)]
    TextUpload(Box<TextUploadError>),
    #[error(transparent)]
    #[from(InteractionError, Box<InteractionError>)]
    Interaction(Box<InteractionError>),
}
