use std::io;

use anyhow::Result;
use tracing::instrument;

use crate::cli::OutputFormat;
use crate::hw::HardwareClient;
use crate::terminal::TerminalClient;

use super::ui::{InspectReportView, Painter};

/// Executes the `inspect` command.
#[instrument(skip(client, out, terminal_client), level = "info", fields(?output_format))]
pub(crate) async fn run<W>(
    client: Box<dyn HardwareClient>,
    out: &mut W,
    terminal_client: &dyn TerminalClient,
    output_format: OutputFormat,
) -> Result<()>
where
    W: io::Write,
{
    let session = crate::SessionHandler::new(client).connect_first().await?;
    let report = session.inspect_report();
    session.close().await?;

    match output_format {
        OutputFormat::Pretty => {
            let painter = Painter::new(terminal_client.stdout_is_terminal());
            writeln!(out, "{}", InspectReportView::new(&report, &painter))?;
        }
        OutputFormat::Json => {
            serde_json::to_writer_pretty(&mut *out, &report)?;
            writeln!(out)?;
        }
    }

    Ok(())
}
