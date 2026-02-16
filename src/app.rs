use std::io;

use anyhow::Result;

use crate::cli::ui::Spinner;
use crate::cli::{Command, FakeArgs};
use crate::hw::{
    DeviceSession, HardwareClient, ModelResolutionConfig,
    fake_hardware_client as build_fake_hardware_client,
    real_hardware_client as build_real_hardware_client,
    real_hardware_client_with_model_resolution as build_real_hardware_client_with_model_resolution,
};
use crate::telemetry;
use crate::terminal::TerminalClient;

const DEFAULT_DEVICE_NAME_PREFIX: &str = "IDM-";
const CONNECTING_SPINNER_MESSAGE: &str = "Scanning for iDotMatrix devices and connecting...";

/// Creates a hardware client backed by the real BLE transport.
#[must_use]
pub fn real_hardware_client() -> Box<dyn HardwareClient> {
    build_real_hardware_client()
}

/// Creates a hardware client backed by the real BLE transport with model-resolution options.
#[must_use]
pub fn real_hardware_client_with_model_resolution(
    model_resolution: ModelResolutionConfig,
) -> Box<dyn HardwareClient> {
    build_real_hardware_client_with_model_resolution(model_resolution)
}

/// Creates a hardware client backed by fake BLE fixtures.
#[must_use]
pub fn fake_hardware_client(fake_args: FakeArgs) -> Box<dyn HardwareClient> {
    build_fake_hardware_client(fake_args.into_backend_config())
}

/// Session-level app helper for acquiring an iDotMatrix connection.
pub struct SessionHandler {
    hardware_client: Box<dyn HardwareClient>,
    name_prefix: String,
}

impl SessionHandler {
    /// Creates a session handler using the default iDotMatrix name prefix.
    ///
    /// ```
    /// # async fn demo() -> anyhow::Result<()> {
    /// let handler = idm::SessionHandler::new(idm::real_hardware_client());
    /// let _ = handler;
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn new(hardware_client: Box<dyn HardwareClient>) -> Self {
        Self {
            hardware_client,
            name_prefix: DEFAULT_DEVICE_NAME_PREFIX.to_string(),
        }
    }

    /// Overrides the BLE local-name prefix used when scanning for devices.
    ///
    /// ```
    /// # async fn demo() -> anyhow::Result<()> {
    /// let handler = idm::SessionHandler::new(idm::real_hardware_client())
    ///     .with_name_prefix("IDM_");
    /// let _ = handler;
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn with_name_prefix(mut self, name_prefix: impl Into<String>) -> Self {
        self.name_prefix = name_prefix.into();
        self
    }

    /// Connects to the first matching iDotMatrix peripheral.
    ///
    /// # Errors
    ///
    /// Returns an error if discovery or connection fails.
    pub async fn connect_first(
        self,
        terminal_client: &dyn TerminalClient,
    ) -> Result<DeviceSession> {
        let spinner = Spinner::new(terminal_client.stderr_is_terminal());
        let name_prefix = self.name_prefix;
        let hardware_client = self.hardware_client;

        let session = spinner
            .with_spinner(CONNECTING_SPINNER_MESSAGE, || async move {
                hardware_client
                    .connect_first_device(name_prefix.as_str())
                    .await
            })
            .await?;

        Ok(session)
    }
}

/// Runs the CLI command with injected clients.
///
/// ```
/// # async fn run() -> anyhow::Result<()> {
/// use clap::Parser;
///
/// let args = idm::Args::try_parse_from([
///     "idm",
///     "--fake",
///     "--fake-scan",
///     "hci0|AA:BB:CC|IDM-Clock|-43",
///     "inspect",
/// ])?;
/// let (command, maybe_fake_args) = args.into_command_and_fake_args()?;
/// let hardware_client = match maybe_fake_args {
///     Some(fake_args) => idm::fake_hardware_client(fake_args),
///     None => idm::real_hardware_client(),
/// };
/// let mut out = Vec::new();
/// idm::run_with_clients(command, &mut out, &idm::SystemTerminalClient, hardware_client).await?;
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// Returns an error if tracing initialisation fails, BLE interaction fails, or
/// output writing fails.
pub async fn run_with_clients<W>(
    command: Command,
    out: &mut W,
    terminal_client: &dyn TerminalClient,
    hardware_client: Box<dyn HardwareClient>,
) -> Result<()>
where
    W: io::Write,
{
    telemetry::initialise_tracing("idm")?;

    match command {
        Command::Inspect => crate::cli::inspect::run(hardware_client, out, terminal_client).await,
        Command::Listen(args) => {
            crate::cli::listen::run(hardware_client, &args, out, terminal_client).await
        }
        Command::Control(args) => {
            crate::cli::control::run(hardware_client, &args, out, terminal_client).await
        }
    }
}
