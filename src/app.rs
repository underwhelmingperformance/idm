use std::io;

use anyhow::Result;

use crate::cli::command::RuntimeArgs;
use crate::cli::{Args, Command};
use crate::hw::hardware_client_from_backend;
use crate::telemetry;
use crate::terminal::{SystemTerminalClient, TerminalClient};

/// Runs the CLI with already parsed arguments.
///
/// ```
/// # async fn run() -> anyhow::Result<()> {
/// use clap::Parser;
///
/// let args = idm::Args::try_parse_from(["idm", "--fake", "--fake-scan", "hci0|AA:BB:CC|IDM-Clock|-43", "inspect"])?;
/// let mut out = Vec::new();
/// idm::run(args, &mut out).await?;
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// Returns an error if tracing initialisation fails, CLI backend configuration is
/// invalid, BLE interaction fails, or output writing fails.
pub async fn run<W>(args: Args, out: &mut W) -> Result<()>
where
    W: io::Write,
{
    run_with_terminal_client(args, out, &SystemTerminalClient).await
}

/// Runs the CLI with already parsed arguments and an injected terminal client.
///
/// # Errors
///
/// Returns an error if tracing initialisation fails, CLI backend configuration is
/// invalid, BLE interaction fails, or output writing fails.
pub async fn run_with_terminal_client<W>(
    args: Args,
    out: &mut W,
    terminal_client: &dyn TerminalClient,
) -> Result<()>
where
    W: io::Write,
{
    telemetry::initialise_tracing("idm")?;
    let runtime: RuntimeArgs = args.try_into()?;
    let RuntimeArgs { backend, command } = runtime;
    let client = hardware_client_from_backend(backend).await?;

    match command {
        Command::Inspect => crate::cli::inspect::run(client, out, terminal_client).await,
        Command::Listen(args) => crate::cli::listen::run(client, &args, out, terminal_client).await,
    }
}
