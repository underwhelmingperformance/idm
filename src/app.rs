use std::io;

use anyhow::Result;
use idm_macros::progress;
use owo_colors::OwoColorize;
use tracing::instrument;
use tracing_indicatif::span_ext::IndicatifSpanExt;

use crate::cli::{Command, FakeArgs, LogLevel};
use crate::hw::{
    DeviceSession, HardwareClient, ModelResolutionConfig,
    fake_hardware_client as build_fake_hardware_client,
    real_hardware_client as build_real_hardware_client,
    real_hardware_client_with_model_resolution as build_real_hardware_client_with_model_resolution,
};
use crate::telemetry;
use crate::terminal::{SystemTerminalClient, TerminalClient};

const DEFAULT_DEVICE_NAME_PREFIX: &str = "IDM-";

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
    #[progress(
        message = "Scanning for iDotMatrix devices and connecting",
        finished = format!("{} Connected", "✓".green()),
    )]
    #[instrument(skip(self), level = "info", fields(name_prefix = %self.name_prefix))]
    pub async fn connect_first(self) -> Result<DeviceSession> {
        let name_prefix = self.name_prefix;
        let hardware_client = self.hardware_client;
        match hardware_client
            .connect_first_device(name_prefix.as_str())
            .await
        {
            Ok(session) => Ok(session),
            Err(error) => {
                let finish_message = format!("{} Connection failed", "✗".red());
                tracing::Span::current().pb_set_finish_message(&finish_message);
                Err(error.into())
            }
        }
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
/// idm::run(command, &mut out, hardware_client).await?;
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// Returns an error if tracing initialisation fails, BLE interaction fails, or
/// output writing fails.
pub async fn run<W>(
    command: Command,
    out: &mut W,
    hardware_client: Box<dyn HardwareClient>,
) -> Result<()>
where
    W: io::Write,
{
    run_with_log_level(command, out, hardware_client, None).await
}

/// Runs the CLI command with an explicit telemetry log-level override.
///
/// ```
/// # async fn run() -> anyhow::Result<()> {
/// use clap::Parser;
///
/// let args = idm::Args::try_parse_from([
///     "idm",
///     "--log-level",
///     "debug",
///     "--fake",
///     "--fake-scan",
///     "hci0|AA:BB:CC|IDM-Clock|-43",
///     "inspect",
/// ])?;
/// let log_level = args.log_level();
/// let (command, maybe_fake_args) = args.into_command_and_fake_args()?;
/// let hardware_client = match maybe_fake_args {
///     Some(fake_args) => idm::fake_hardware_client(fake_args),
///     None => idm::real_hardware_client(),
/// };
/// let mut out = Vec::new();
/// idm::run_with_log_level(command, &mut out, hardware_client, log_level).await?;
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// Returns an error if tracing initialisation fails, BLE interaction fails, or
/// output writing fails.
pub async fn run_with_log_level<W>(
    command: Command,
    out: &mut W,
    hardware_client: Box<dyn HardwareClient>,
    log_level: Option<LogLevel>,
) -> Result<()>
where
    W: io::Write,
{
    run_with_clients_and_log_level(
        command,
        out,
        &SystemTerminalClient,
        hardware_client,
        log_level,
    )
    .await
}

/// Runs the CLI command with injected clients.
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
    run_with_clients_and_log_level(command, out, terminal_client, hardware_client, None).await
}

/// Runs the CLI command with injected clients and explicit telemetry settings.
///
/// ```
/// # async fn run() -> anyhow::Result<()> {
/// use clap::Parser;
///
/// struct FakeTerminal;
/// impl idm::TerminalClient for FakeTerminal {
///     fn stdout_is_terminal(&self) -> bool { false }
///     fn stderr_is_terminal(&self) -> bool { false }
/// }
///
/// let args = idm::Args::try_parse_from([
///     "idm",
///     "--log-level",
///     "trace",
///     "--fake",
///     "--fake-scan",
///     "hci0|AA:BB:CC|IDM-Clock|-43",
///     "inspect",
/// ])?;
/// let log_level = args.log_level();
/// let (command, maybe_fake_args) = args.into_command_and_fake_args()?;
/// let hardware_client = match maybe_fake_args {
///     Some(fake_args) => idm::fake_hardware_client(fake_args),
///     None => idm::real_hardware_client(),
/// };
/// let mut out = Vec::new();
/// idm::run_with_clients_and_log_level(
///     command,
///     &mut out,
///     &FakeTerminal,
///     hardware_client,
///     log_level,
/// ).await?;
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// Returns an error if tracing initialisation fails, BLE interaction fails, or
/// output writing fails.
#[instrument(
    skip(out, terminal_client, hardware_client),
    level = "info",
    fields(command = %command_name(&command), ?log_level)
)]
pub async fn run_with_clients_and_log_level<W>(
    command: Command,
    out: &mut W,
    terminal_client: &dyn TerminalClient,
    hardware_client: Box<dyn HardwareClient>,
    log_level: Option<LogLevel>,
) -> Result<()>
where
    W: io::Write,
{
    telemetry::initialise_tracing(
        "idm",
        terminal_client.stderr_is_terminal(),
        log_level.map(LogLevel::as_level_filter),
    )?;

    match command {
        Command::Inspect => crate::cli::inspect::run(hardware_client, out, terminal_client).await,
        Command::Listen(args) => {
            crate::cli::listen::run(hardware_client, &args, out, terminal_client).await
        }
        Command::Control(args) => crate::cli::control::run(hardware_client, &args, out).await,
    }
}

fn command_name(command: &Command) -> &'static str {
    match command {
        Command::Inspect => "inspect",
        Command::Listen(_args) => "listen",
        Command::Control(_args) => "control",
    }
}
