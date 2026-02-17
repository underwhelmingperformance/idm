use std::io;

use anyhow::{Context, Result};
use clap::{Args, Subcommand, ValueEnum};
use time::OffsetDateTime;
use tracing::instrument;

use crate::hw::HardwareClient;
use crate::{
    Brightness, BrightnessHandler, FullscreenColourHandler, PowerHandler, Rgb, ScreenPower,
    SessionHandler, TextUploadHandler, TextUploadRequest, TimeSyncHandler,
};

/// Arguments for the `control` command.
#[derive(Debug, Args)]
pub struct ControlArgs {
    #[command(subcommand)]
    action: ControlAction,
}

impl ControlArgs {
    /// Creates control arguments for one action.
    ///
    /// ```
    /// use idm::{ControlAction, ControlArgs, SyncTimeArgs};
    ///
    /// let args = ControlArgs::new(ControlAction::SyncTime(SyncTimeArgs::new(None)));
    /// let _ = args;
    /// ```
    #[must_use]
    pub fn new(action: ControlAction) -> Self {
        Self { action }
    }
}

/// Action performed by the `control` command.
#[derive(Debug, Subcommand)]
pub enum ControlAction {
    /// Turn the screen on or off.
    Power(PowerArgs),
    /// Set panel brightness (0..=100).
    Brightness(BrightnessArgs),
    /// Fill the display with one RGB colour.
    Colour(ColourArgs),
    /// Synchronise device time.
    SyncTime(SyncTimeArgs),
    /// Upload text content.
    Text(TextArgs),
}

/// Arguments for `control power`.
#[derive(Debug, Args)]
pub struct PowerArgs {
    #[arg(value_enum)]
    state: PowerState,
}

impl PowerArgs {
    /// Creates power-control arguments.
    ///
    /// ```
    /// use idm::{PowerArgs, PowerState};
    ///
    /// let args = PowerArgs::new(PowerState::On);
    /// let _ = args;
    /// ```
    #[must_use]
    pub fn new(state: PowerState) -> Self {
        Self { state }
    }
}

/// Requested power state.
#[derive(Debug, Clone, Copy, Eq, PartialEq, ValueEnum)]
pub enum PowerState {
    /// Turn the screen off.
    Off,
    /// Turn the screen on.
    On,
}

impl PowerState {
    fn to_handler_power(self) -> ScreenPower {
        match self {
            Self::Off => ScreenPower::Off,
            Self::On => ScreenPower::On,
        }
    }
}

impl std::fmt::Display for PowerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Off => write!(f, "off"),
            Self::On => write!(f, "on"),
        }
    }
}

/// Arguments for `control brightness`.
#[derive(Debug, Args)]
pub struct BrightnessArgs {
    #[arg(value_parser = parse_brightness)]
    brightness: Brightness,
}

impl BrightnessArgs {
    /// Creates brightness-control arguments.
    ///
    /// # Errors
    ///
    /// Returns an error when `value` is outside `0..=100`.
    ///
    /// ```
    /// use idm::BrightnessArgs;
    ///
    /// let args = BrightnessArgs::new(75)?;
    /// assert_eq!(75, args.value());
    /// # Ok::<(), idm::BrightnessError>(())
    /// ```
    pub fn new(value: u8) -> Result<Self, crate::BrightnessError> {
        let brightness = Brightness::new(value)?;
        Ok(Self { brightness })
    }

    /// Returns the validated brightness value.
    #[must_use]
    pub fn value(&self) -> u8 {
        self.brightness.value()
    }
}

/// Arguments for `control colour`.
#[derive(Debug, Args)]
pub struct ColourArgs {
    red: u8,
    green: u8,
    blue: u8,
}

impl ColourArgs {
    /// Creates colour-control arguments.
    ///
    /// ```
    /// use idm::ColourArgs;
    ///
    /// let args = ColourArgs::new(0x11, 0x22, 0x33);
    /// assert_eq!(0x11, args.red());
    /// ```
    #[must_use]
    pub fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    /// Returns the red byte.
    #[must_use]
    pub fn red(&self) -> u8 {
        self.red
    }
}

/// Arguments for `control sync-time`.
#[derive(Debug, Args)]
pub struct SyncTimeArgs {
    /// Unix timestamp in UTC seconds. Uses current UTC time when omitted.
    #[arg(long)]
    unix: Option<i64>,
}

impl SyncTimeArgs {
    /// Creates sync-time arguments.
    ///
    /// ```
    /// use idm::SyncTimeArgs;
    ///
    /// let args = SyncTimeArgs::new(Some(1_700_000_000));
    /// let _ = args;
    /// ```
    #[must_use]
    pub fn new(unix: Option<i64>) -> Self {
        Self { unix }
    }

    fn resolve_timestamp(&self) -> Result<OffsetDateTime> {
        match self.unix {
            Some(value) => OffsetDateTime::from_unix_timestamp(value)
                .with_context(|| format!("invalid unix timestamp: {value}")),
            None => Ok(OffsetDateTime::now_utc()),
        }
    }
}

/// Arguments for `control text`.
#[derive(Debug, Args)]
pub struct TextArgs {
    /// Text content to render and upload.
    text: String,
}

impl TextArgs {
    /// Creates text-upload arguments.
    ///
    /// ```
    /// use idm::TextArgs;
    ///
    /// let args = TextArgs::new("Hello");
    /// let _ = args;
    /// ```
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

fn parse_brightness(value: &str) -> Result<Brightness, String> {
    let parsed = value.parse::<u8>().map_err(|error| error.to_string())?;
    Brightness::new(parsed).map_err(|error| error.to_string())
}

/// Executes the `control` command.
#[instrument(skip(client, args, out), level = "info", fields(action = ?args.action))]
pub(crate) async fn run<W>(
    client: Box<dyn HardwareClient>,
    args: &ControlArgs,
    out: &mut W,
) -> Result<()>
where
    W: io::Write,
{
    let session = SessionHandler::new(client).connect_first().await?;

    let command_result = run_with_session(&session, args, out).await;
    let close_result = session.close().await;

    if let Err(error) = close_result {
        if command_result.is_ok() {
            return Err(error.into());
        }
        tracing::trace!(?error, "failed to close control session cleanly");
    }

    command_result
}

#[instrument(skip(session, args, out), level = "debug", fields(action = ?args.action))]
async fn run_with_session<W>(
    session: &crate::DeviceSession,
    args: &ControlArgs,
    out: &mut W,
) -> Result<()>
where
    W: io::Write,
{
    match &args.action {
        ControlAction::Power(power_args) => {
            PowerHandler::set_power(session, power_args.state.to_handler_power()).await?;
            writeln!(out, "Applied power state: {}", power_args.state)?;
        }
        ControlAction::Brightness(brightness_args) => {
            BrightnessHandler::set_brightness(session, brightness_args.brightness).await?;
            writeln!(
                out,
                "Applied brightness: {}",
                brightness_args.brightness.value()
            )?;
        }
        ControlAction::Colour(colour_args) => {
            let colour = Rgb::new(colour_args.red, colour_args.green, colour_args.blue);
            FullscreenColourHandler::set_colour(session, colour).await?;
            writeln!(
                out,
                "Applied fullscreen colour: #{:02X}{:02X}{:02X}",
                colour.r, colour.g, colour.b
            )?;
        }
        ControlAction::SyncTime(sync_time_args) => {
            let timestamp = sync_time_args.resolve_timestamp()?;
            TimeSyncHandler::sync_time(session, timestamp).await?;
            writeln!(
                out,
                "Synced time (UTC unix): {}",
                timestamp.unix_timestamp()
            )?;
        }
        ControlAction::Text(text_args) => {
            let receipt =
                TextUploadHandler::upload(session, TextUploadRequest::new(text_args.text.clone()))
                    .await?;
            writeln!(
                out,
                "Uploaded text payload: {} bytes in {} chunk(s)",
                receipt.bytes_written(),
                receipt.chunks_written(),
            )?;
        }
    }

    Ok(())
}
