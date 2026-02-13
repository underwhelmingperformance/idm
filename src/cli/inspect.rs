use std::io;

use anyhow::Result;

use crate::hw::HardwareClient;
use crate::terminal::TerminalClient;

use super::IDM_NAME_PREFIX;
use super::ui::{InspectReportView, Painter, Spinner};

/// Executes the `inspect` command.
pub(crate) async fn run<W>(
    client: Box<dyn HardwareClient>,
    out: &mut W,
    terminal_client: &dyn TerminalClient,
) -> Result<()>
where
    W: io::Write,
{
    let painter = Painter::new(terminal_client.stdout_is_terminal());
    let spinner = Spinner::new(terminal_client.stderr_is_terminal());
    let report = spinner
        .with_spinner(
            "Scanning for iDotMatrix devices and connecting...",
            || async move { client.inspect_first_device(IDM_NAME_PREFIX).await },
        )
        .await?;

    writeln!(out, "{}", InspectReportView::new(&report, &painter))?;

    Ok(())
}
