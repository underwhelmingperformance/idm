use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Args, Subcommand, ValueEnum};
use serde::Serialize;
use time::OffsetDateTime;
use tracing::instrument;

use crate::cli::OutputFormat;
use crate::hw::HardwareClient;
use crate::{
    Brightness, BrightnessHandler, FullscreenColourHandler, GifAnimation, GifUploadHandler,
    GifUploadRequest, PowerHandler, Rgb, ScreenPower, SessionHandler, TextOptions,
    TextUploadHandler, TextUploadRequest, TimeSyncHandler, UploadPacing,
};

/// JSON result emitted by a `control` action.
#[derive(Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum ControlResult {
    Power {
        state: String,
    },
    Brightness {
        value: u8,
    },
    Colour {
        red: u8,
        green: u8,
        blue: u8,
    },
    SyncTime {
        unix_timestamp: i64,
    },
    Text {
        bytes_written: usize,
        chunks_written: usize,
    },
    Gif {
        bytes_written: usize,
        chunks_written: usize,
        logical_chunks_sent: usize,
        cached: bool,
    },
}

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
    /// Upload GIF bytes from a file.
    Gif(GifArgs),
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
    /// Text content to render and upload using standard CLI defaults.
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

/// Arguments for `control gif`.
#[derive(Debug, Args)]
pub struct GifArgs {
    /// Path to a GIF file to upload.
    gif_file: PathBuf,
}

impl GifArgs {
    /// Creates GIF-upload arguments.
    ///
    /// ```
    /// use std::path::Path;
    /// use std::path::PathBuf;
    ///
    /// use idm::GifArgs;
    ///
    /// let args = GifArgs::new(PathBuf::from("demo.gif"));
    /// assert_eq!(Path::new("demo.gif"), args.path());
    /// ```
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            gif_file: path.into(),
        }
    }

    /// Returns the selected GIF file path.
    ///
    /// ```
    /// use std::path::Path;
    /// use std::path::PathBuf;
    ///
    /// use idm::GifArgs;
    ///
    /// let args = GifArgs::new(PathBuf::from("demo.gif"));
    /// assert_eq!(Path::new("demo.gif"), args.path());
    /// ```
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.gif_file
    }
}

fn parse_brightness(value: &str) -> Result<Brightness, String> {
    let parsed = value.parse::<u8>().map_err(|error| error.to_string())?;
    Brightness::new(parsed).map_err(|error| error.to_string())
}

/// Executes the `control` command.
#[instrument(skip(client, args, out), level = "info", fields(action = ?args.action, ?output_format))]
pub(crate) async fn run<W>(
    client: Box<dyn HardwareClient>,
    args: &ControlArgs,
    out: &mut W,
    output_format: OutputFormat,
) -> Result<()>
where
    W: io::Write,
{
    let session = SessionHandler::new(client).connect_first().await?;

    let command_result = run_with_session(&session, args, out, output_format).await;
    let close_result = session.close().await;

    if let Err(error) = close_result {
        if command_result.is_ok() {
            return Err(error.into());
        }
        tracing::trace!(?error, "failed to close control session cleanly");
    }

    command_result
}

#[instrument(skip(session, args, out), level = "debug", fields(action = ?args.action, ?output_format))]
async fn run_with_session<W>(
    session: &crate::DeviceSession,
    args: &ControlArgs,
    out: &mut W,
    output_format: OutputFormat,
) -> Result<()>
where
    W: io::Write,
{
    match &args.action {
        ControlAction::Power(power_args) => {
            PowerHandler::set_power(session, power_args.state.to_handler_power()).await?;
            match output_format {
                OutputFormat::Pretty => {
                    writeln!(out, "Applied power state: {}", power_args.state)?;
                }
                OutputFormat::Json => {
                    write_json_line(
                        out,
                        &ControlResult::Power {
                            state: power_args.state.to_string(),
                        },
                    )?;
                }
            }
        }
        ControlAction::Brightness(brightness_args) => {
            BrightnessHandler::set_brightness(session, brightness_args.brightness).await?;
            match output_format {
                OutputFormat::Pretty => {
                    writeln!(
                        out,
                        "Applied brightness: {}",
                        brightness_args.brightness.value()
                    )?;
                }
                OutputFormat::Json => {
                    write_json_line(
                        out,
                        &ControlResult::Brightness {
                            value: brightness_args.brightness.value(),
                        },
                    )?;
                }
            }
        }
        ControlAction::Colour(colour_args) => {
            let colour = Rgb::new(colour_args.red, colour_args.green, colour_args.blue);
            FullscreenColourHandler::set_colour(session, colour).await?;
            match output_format {
                OutputFormat::Pretty => {
                    writeln!(
                        out,
                        "Applied fullscreen colour: #{:02X}{:02X}{:02X}",
                        colour.r, colour.g, colour.b
                    )?;
                }
                OutputFormat::Json => {
                    write_json_line(
                        out,
                        &ControlResult::Colour {
                            red: colour.r,
                            green: colour.g,
                            blue: colour.b,
                        },
                    )?;
                }
            }
        }
        ControlAction::SyncTime(sync_time_args) => {
            let timestamp = sync_time_args.resolve_timestamp()?;
            TimeSyncHandler::sync_time(session, timestamp).await?;
            match output_format {
                OutputFormat::Pretty => {
                    writeln!(
                        out,
                        "Synced time (UTC unix): {}",
                        timestamp.unix_timestamp()
                    )?;
                }
                OutputFormat::Json => {
                    write_json_line(
                        out,
                        &ControlResult::SyncTime {
                            unix_timestamp: timestamp.unix_timestamp(),
                        },
                    )?;
                }
            }
        }
        ControlAction::Text(text_args) => {
            let receipt =
                TextUploadHandler::upload(session, default_cli_text_request(&text_args.text))
                    .await?;
            match output_format {
                OutputFormat::Pretty => {
                    writeln!(
                        out,
                        "Uploaded text payload: {} bytes in {} chunk(s)",
                        receipt.bytes_written(),
                        receipt.chunks_written(),
                    )?;
                }
                OutputFormat::Json => {
                    write_json_line(
                        out,
                        &ControlResult::Text {
                            bytes_written: receipt.bytes_written(),
                            chunks_written: receipt.chunks_written(),
                        },
                    )?;
                }
            }
        }
        ControlAction::Gif(gif_args) => {
            let receipt =
                GifUploadHandler::upload(session, default_cli_gif_request(&gif_args.gif_file)?)
                    .await?;
            match output_format {
                OutputFormat::Pretty => {
                    if receipt.cached() {
                        writeln!(
                            out,
                            "Uploaded GIF payload: {} bytes in {} chunk(s); device cache hit",
                            receipt.bytes_written(),
                            receipt.chunks_written(),
                        )?;
                    } else {
                        writeln!(
                            out,
                            "Uploaded GIF payload: {} bytes in {} chunk(s) across {} logical chunk(s)",
                            receipt.bytes_written(),
                            receipt.chunks_written(),
                            receipt.logical_chunks_sent(),
                        )?;
                    }
                }
                OutputFormat::Json => {
                    write_json_line(
                        out,
                        &ControlResult::Gif {
                            bytes_written: receipt.bytes_written(),
                            chunks_written: receipt.chunks_written(),
                            logical_chunks_sent: receipt.logical_chunks_sent(),
                            cached: receipt.cached(),
                        },
                    )?;
                }
            }
        }
    }

    Ok(())
}

fn write_json_line(out: &mut impl io::Write, value: &impl Serialize) -> Result<()> {
    serde_json::to_writer_pretty(&mut *out, value)?;
    writeln!(out)?;
    Ok(())
}

fn default_cli_text_request(text: &str) -> TextUploadRequest {
    TextUploadRequest::new(text.to_string())
        .with_options(TextOptions::new(
            0x00,
            0x20,
            0x01,
            Rgb::new(0xFF, 0xFF, 0xFF),
            0x00,
            Rgb::new(0x00, 0x00, 0x00),
        ))
        .with_pacing(UploadPacing::NotifyAck {
            timeout: Duration::from_secs(5),
        })
}

fn default_cli_gif_request(path: &Path) -> Result<GifUploadRequest> {
    let payload = std::fs::read(path)
        .with_context(|| format!("failed to read GIF file `{}`", path.display()))?;
    let gif = GifAnimation::try_from(payload)
        .with_context(|| format!("failed to decode GIF file `{}`", path.display()))?;
    Ok(GifUploadRequest::new(gif)
        .with_per_fragment_delay(Duration::from_millis(20))
        .with_ack_timeout(Duration::from_secs(5)))
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn default_cli_text_request_uses_stable_defaults() {
        let request = default_cli_text_request("Hello");
        let expected = TextUploadRequest::new("Hello")
            .with_options(TextOptions::new(
                0x00,
                0x20,
                0x01,
                Rgb::new(0xFF, 0xFF, 0xFF),
                0x00,
                Rgb::new(0x00, 0x00, 0x00),
            ))
            .with_pacing(UploadPacing::NotifyAck {
                timeout: Duration::from_secs(5),
            });

        assert_eq!(expected, request);
    }

    #[test]
    fn default_cli_gif_request_reads_file_and_applies_defaults() -> anyhow::Result<()> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos();
        let file_path = std::env::temp_dir().join(format!(
            "idm-control-gif-{}-{timestamp}.gif",
            std::process::id()
        ));
        let expected_payload = vec![
            0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0x00,
            0x00, 0x00, 0xFF, 0xFF, 0xFF, 0x21, 0xF9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, 0x2C,
            0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44, 0x01, 0x00,
            0x3B,
        ];
        std::fs::write(&file_path, &expected_payload)
            .with_context(|| format!("failed to write temporary file `{}`", file_path.display()))?;

        let request = default_cli_gif_request(&file_path)?;
        let expected = GifUploadRequest::new(
            GifAnimation::try_from(expected_payload).expect("test gif payload should decode"),
        )
        .with_per_fragment_delay(Duration::from_millis(20))
        .with_ack_timeout(Duration::from_secs(5));

        assert_eq!(expected, request);
        std::fs::remove_file(&file_path).with_context(|| {
            format!("failed to remove temporary file `{}`", file_path.display())
        })?;
        Ok(())
    }
}
