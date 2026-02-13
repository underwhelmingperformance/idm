use thiserror::Error;

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
    #[error("failed while waiting for Ctrl+C")]
    CtrlC(#[from] std::io::Error),
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
