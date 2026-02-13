use std::time::Duration;

use bon::Builder;
use clap::{Parser, Subcommand};

use crate::cli::listen::ListenArgs;
use crate::error::{CliConfigError, FixtureError};
use crate::hw::{
    FakeBackendConfig, HardwareBackend, HexPayload, NotificationPayloads, ScanFixture,
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
        } = fake;

        self.fake = true;
        self.fake_scan = Some(scan_fixture);
        self.fake_read = initial_read;
        self.fake_notifications = notifications;
        self.fake_discovery_delay = Some(discovery_delay);
        self
    }
}

/// Runtime arguments resolved from parsed CLI input.
#[derive(Debug)]
pub(crate) struct RuntimeArgs {
    pub(crate) backend: HardwareBackend,
    pub(crate) command: Command,
}

impl TryFrom<Args> for RuntimeArgs {
    type Error = CliConfigError;

    fn try_from(args: Args) -> Result<Self, Self::Error> {
        let Args {
            fake,
            fake_scan,
            fake_read,
            fake_notifications,
            fake_discovery_delay,
            command,
        } = args;

        let backend = if fake {
            let Some(scan_fixture) = fake_scan else {
                return Err(CliConfigError::MissingFakeScanFixture);
            };
            let config = FakeBackendConfig::builder()
                .scan_fixture(scan_fixture)
                .maybe_initial_read(fake_read)
                .maybe_notifications(fake_notifications)
                .discovery_delay(fake_discovery_delay.unwrap_or(Duration::ZERO))
                .build();
            HardwareBackend::Fake(config)
        } else {
            HardwareBackend::Real
        };

        Ok(Self { backend, command })
    }
}

/// Fake backend arguments for programmatic runs.
#[derive(Debug, Builder)]
pub struct FakeArgs {
    #[builder(with = |value: &str| -> std::result::Result<_, FixtureError> { value.parse() })]
    scan_fixture: ScanFixture,
    #[builder(with = |value: &str| -> std::result::Result<_, FixtureError> { value.parse() })]
    initial_read: Option<HexPayload>,
    #[builder(with = |value: &str| -> std::result::Result<_, FixtureError> { value.parse() })]
    notifications: Option<NotificationPayloads>,
    #[builder(default)]
    discovery_delay: Duration,
}

/// Supported CLI commands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Scan until the first iDotMatrix device is found, connect, and print GATT details.
    Inspect,
    /// Scan until the first iDotMatrix device is found, connect, read once, then listen for notifications.
    Listen(ListenArgs),
}

fn parse_duration(value: &str) -> Result<Duration, String> {
    humantime::parse_duration(value).map_err(|error| error.to_string())
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
    fn fake_mode_builds_fake_backend_config() {
        let cli = Args::try_parse_from([
            "idm",
            "--fake",
            "--fake-scan",
            "hci0|AA:BB:CC|IDM-Clock|-43",
            "inspect",
        ])
        .expect("valid fake arguments should parse");

        let runtime: RuntimeArgs = cli
            .try_into()
            .expect("valid fake arguments should resolve backend");
        let RuntimeArgs {
            backend,
            command: _,
        } = runtime;
        assert_matches!(backend, HardwareBackend::Fake(_));
    }
}
