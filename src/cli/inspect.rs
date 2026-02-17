use std::io;

use anyhow::Result;
use tracing::instrument;

use crate::hw::HardwareClient;
use crate::terminal::TerminalClient;

use super::ui::{InspectReportView, Painter};

/// Executes the `inspect` command.
#[instrument(skip(client, out, terminal_client), level = "info")]
pub(crate) async fn run<W>(
    client: Box<dyn HardwareClient>,
    out: &mut W,
    terminal_client: &dyn TerminalClient,
) -> Result<()>
where
    W: io::Write,
{
    let painter = Painter::new(terminal_client.stdout_is_terminal());
    let session = crate::SessionHandler::new(client).connect_first().await?;
    let report = session.inspect_report();
    session.close().await?;

    writeln!(out, "{}", InspectReportView::new(&report, &painter))?;

    Ok(())
}
