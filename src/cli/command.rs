use std::path::PathBuf;
use std::time::Duration;

use bon::Builder;
use clap::{Parser, Subcommand, ValueEnum};
use tracing_subscriber::filter::LevelFilter;

use crate::cli::control::ControlArgs;
use crate::cli::listen::ListenArgs;
use crate::error::CliConfigError;
use crate::hw::{
    FakeBackendConfig, HexPayload, ModelResolutionConfig, NotificationPayloads, ScanFixture,
};

/// Command-line options for the iDotMatrix BLE tool.
#[derive(Debug, Parser)]
#[command(name = "idm", about = "Interact with iDotMatrix BLE devices.")]
pub struct Args {
    /// Uses the fake BLE backend with fixture-driven discovery and payloads.
    #[arg(long, global = true)]
    fake: bool,
    /// Fake scan fixtures in the form `adapter|device_id|local_name|rssi;...`.
    #[arg(long, global = true, requires = "fake", required_if_eq("fake", "true"))]
    fake_scan: Option<ScanFixture>,
    /// Fake `fa03` initial read payload as hexadecimal bytes.
    #[arg(long, global = true, requires = "fake")]
    fake_read: Option<HexPayload>,
    /// Fake notification payloads as comma-separated hexadecimal payloads.
    #[arg(long, global = true, requires = "fake")]
    fake_notifications: Option<NotificationPayloads>,
    /// Artificial fake scan delay (e.g. `250ms`, `2s`).
    #[arg(long, global = true, requires = "fake", value_parser = parse_duration)]
    fake_discovery_delay: Option<Duration>,
    /// Explicit LED type override used to resolve ambiguous scan shapes.
    #[arg(long, global = true, value_parser = parse_led_type)]
    model_led_type: Option<u8>,
    /// Path to the persisted model-overrides file.
    #[arg(long, global = true)]
    model_overrides_path: Option<PathBuf>,
    /// Override the telemetry log verbosity.
    #[arg(long, global = true, value_enum)]
    log_level: Option<LogLevel>,
    /// Output format for command results. Defaults to `pretty` when stdout is a
    /// terminal, `json` otherwise.
    #[arg(long, global = true, value_enum)]
    output_format: Option<OutputFormat>,
    #[command(subcommand)]
    command: Command,
}

impl Args {
    /// Creates argument values directly without CLI parsing.
    ///
    /// ```
    /// use idm::{Args, Command, ListenArgs};
    ///
    /// let inspect = Args::new(Command::Inspect);
    /// let listen = Args::new(Command::Listen(ListenArgs::new(Some(10))));
    /// let _ = (inspect, listen);
    /// ```
    #[must_use]
    pub fn new(command: Command) -> Self {
        Self {
            fake: false,
            fake_scan: None,
            fake_read: None,
            fake_notifications: None,
            fake_discovery_delay: None,
            model_led_type: None,
            model_overrides_path: None,
            log_level: None,
            output_format: None,
            command,
        }
    }

    /// Enables fake backend mode with pre-parsed fake configuration.
    #[must_use]
    pub fn with_fake(mut self, fake: FakeArgs) -> Self {
        let FakeArgs {
            scan_fixture,
            initial_read,
            notifications,
            discovery_delay,
            model_led_type,
            model_overrides_path,
        } = fake;

        self.fake = true;
        self.fake_scan = Some(scan_fixture);
        self.fake_read = initial_read;
        self.fake_notifications = notifications;
        self.fake_discovery_delay = Some(discovery_delay);
        self.model_led_type = model_led_type;
        self.model_overrides_path = model_overrides_path;
        self
    }

    /// Returns model-resolution options derived from CLI arguments.
    #[must_use]
    pub fn model_resolution(&self) -> ModelResolutionConfig {
        ModelResolutionConfig::new(self.model_led_type, self.model_overrides_path.clone())
    }

    /// Returns an optional CLI override for telemetry log level.
    #[must_use]
    pub fn log_level(&self) -> Option<LogLevel> {
        self.log_level
    }

    /// Returns the explicitly selected output format, if any.
    #[must_use]
    pub fn output_format(&self) -> Option<OutputFormat> {
        self.output_format
    }

    /// Splits parsed CLI arguments into command and optional fake-client settings.
    ///
    /// # Errors
    ///
    /// Returns an error if CLI backend configuration is invalid.
    pub fn into_command_and_fake_args(self) -> anyhow::Result<(Command, Option<FakeArgs>)> {
        let Args {
            fake,
            fake_scan,
            fake_read,
            fake_notifications,
            fake_discovery_delay,
            model_led_type,
            model_overrides_path,
            log_level: _,
            output_format: _,
            command,
        } = self;

        let fake_args = if fake {
            let Some(scan_fixture) = fake_scan else {
                return Err(CliConfigError::MissingFakeScanFixture.into());
            };
            Some(FakeArgs {
                scan_fixture,
                initial_read: fake_read,
                notifications: fake_notifications,
                discovery_delay: fake_discovery_delay.unwrap_or(Duration::ZERO),
                model_led_type,
                model_overrides_path,
            })
        } else {
            None
        };

        Ok((command, fake_args))
    }
}

/// Output format for command results.
#[derive(Debug, Clone, Copy, Eq, PartialEq, ValueEnum)]
pub enum OutputFormat {
    /// Human-readable styled output.
    Pretty,
    /// Machine-readable JSON output.
    Json,
}

/// Log verbosity override for tracing and log events.
#[derive(Debug, Clone, Copy, Eq, PartialEq, ValueEnum)]
pub enum LogLevel {
    /// Error-level events only.
    Error,
    /// Warning and error events.
    Warn,
    /// Informational, warning, and error events.
    Info,
    /// Debug and above.
    Debug,
    /// Full trace verbosity.
    Trace,
}

impl LogLevel {
    #[must_use]
    pub(crate) fn as_level_filter(self) -> LevelFilter {
        match self {
            Self::Error => LevelFilter::ERROR,
            Self::Warn => LevelFilter::WARN,
            Self::Info => LevelFilter::INFO,
            Self::Debug => LevelFilter::DEBUG,
            Self::Trace => LevelFilter::TRACE,
        }
    }
}

/// Fake backend arguments for programmatic runs.
#[derive(Debug, Builder)]
pub struct FakeArgs {
    #[builder(with = |value: &str| -> std::result::Result<_, crate::error::FixtureError> { value.parse() })]
    scan_fixture: ScanFixture,
    #[builder(with = |value: &str| -> std::result::Result<_, crate::error::FixtureError> { value.parse() })]
    initial_read: Option<HexPayload>,
    #[builder(with = |value: &str| -> std::result::Result<_, crate::error::FixtureError> { value.parse() })]
    notifications: Option<NotificationPayloads>,
    #[builder(default)]
    discovery_delay: Duration,
    model_led_type: Option<u8>,
    model_overrides_path: Option<PathBuf>,
}

impl FakeArgs {
    pub(crate) fn into_backend_config(self) -> FakeBackendConfig {
        let Self {
            scan_fixture,
            initial_read,
            notifications,
            discovery_delay,
            model_led_type,
            model_overrides_path,
        } = self;

        FakeBackendConfig::builder()
            .scan_fixture(scan_fixture)
            .maybe_initial_read(initial_read)
            .maybe_notifications(notifications)
            .discovery_delay(discovery_delay)
            .model_resolution(ModelResolutionConfig::new(
                model_led_type,
                model_overrides_path,
            ))
            .build()
    }
}

/// Supported CLI commands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Scan until the first iDotMatrix device is found, connect, and print GATT details.
    Inspect,
    /// Scan until the first iDotMatrix device is found, connect, read once, then listen for notifications.
    Listen(ListenArgs),
    /// Scan until the first iDotMatrix device is found, connect, then send one control command.
    Control(ControlArgs),
}

fn parse_duration(value: &str) -> Result<Duration, String> {
    humantime::parse_duration(value).map_err(|error| error.to_string())
}

fn parse_led_type(value: &str) -> Result<u8, String> {
    let parsed = value.parse::<u8>().map_err(|error| error.to_string())?;
    if !matches!(parsed, 1 | 2 | 3 | 4 | 6 | 7 | 11) {
        return Err("supported values are 1, 2, 3, 4, 6, 7, 11".to_string());
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use clap::error::ErrorKind;
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn fake_mode_requires_scan_fixture() {
        let result = Args::try_parse_from(["idm", "--fake", "inspect"]);

        let error = result.expect_err("missing --fake-scan should fail argument parsing");
        assert_eq!(ErrorKind::MissingRequiredArgument, error.kind());
    }

    #[test]
    fn fake_fixture_flags_require_fake_mode() {
        let result = Args::try_parse_from(["idm", "--fake-read", "DEADBEEF", "inspect"]);

        let error = result.expect_err("fake payload flags should require --fake");
        assert_eq!(ErrorKind::MissingRequiredArgument, error.kind());
    }

    #[test]
    fn fake_scan_requires_fake_mode() {
        let result = Args::try_parse_from([
            "idm",
            "--fake-scan",
            "hci0|AA:BB:CC|IDM-Clock|-43",
            "inspect",
        ]);

        let error = result.expect_err("--fake-scan should require --fake");
        assert_eq!(ErrorKind::MissingRequiredArgument, error.kind());
    }

    #[test]
    fn fake_mode_builds_fake_settings() {
        let cli = Args::try_parse_from([
            "idm",
            "--fake",
            "--fake-scan",
            "hci0|AA:BB:CC|IDM-Clock|-43",
            "inspect",
        ])
        .expect("valid fake arguments should parse");

        let (command, fake_args) = cli
            .into_command_and_fake_args()
            .expect("valid fake arguments should resolve fake settings");
        assert_matches!(command, Command::Inspect);
        assert_matches!(fake_args, Some(_));
    }

    #[test]
    fn model_led_type_rejects_unsupported_value() {
        let result = Args::try_parse_from([
            "idm",
            "--model-led-type",
            "9",
            "--fake",
            "--fake-scan",
            "hci0|AA:BB:CC|IDM-Clock|-43",
            "inspect",
        ]);

        let error = result.expect_err("unsupported model-led-type should fail parsing");
        assert_eq!(ErrorKind::ValueValidation, error.kind());
    }

    #[test]
    fn model_args_are_exposed_via_model_resolution() {
        let cli = Args::try_parse_from([
            "idm",
            "--model-led-type",
            "2",
            "--model-overrides-path",
            "/tmp/idm-overrides.tsv",
            "--fake",
            "--fake-scan",
            "hci0|AA:BB:CC|IDM-Clock|-43",
            "inspect",
        ])
        .expect("model arguments should parse");

        let model_resolution = cli.model_resolution();
        assert_eq!(Some(2), model_resolution.led_type_override());
        assert_eq!(
            Some(std::path::Path::new("/tmp/idm-overrides.tsv")),
            model_resolution.overrides_path()
        );
    }

    #[test]
    fn output_format_argument_parses() {
        let cli = Args::try_parse_from([
            "idm",
            "--output-format",
            "json",
            "--fake",
            "--fake-scan",
            "hci0|AA:BB:CC|IDM-Clock|-43",
            "inspect",
        ])
        .expect("output-format should parse as a value enum");

        assert_eq!(Some(OutputFormat::Json), cli.output_format());
    }

    #[test]
    fn output_format_defaults_to_none() {
        let cli = Args::try_parse_from([
            "idm",
            "--fake",
            "--fake-scan",
            "hci0|AA:BB:CC|IDM-Clock|-43",
            "inspect",
        ])
        .expect("should parse without output-format");

        assert_eq!(None, cli.output_format());
    }

    #[test]
    fn log_level_argument_parses() {
        let cli = Args::try_parse_from([
            "idm",
            "--log-level",
            "trace",
            "--fake",
            "--fake-scan",
            "hci0|AA:BB:CC|IDM-Clock|-43",
            "inspect",
        ])
        .expect("log-level should parse as a value enum");

        assert_eq!(Some(LogLevel::Trace), cli.log_level());
    }
}
