use clap::Args;
use std::io;

use anyhow::Result;

use crate::hw::HardwareClient;
use crate::terminal::TerminalClient;

use super::IDM_NAME_PREFIX;
use super::ui::{ListenNotificationView, ListenReadyView, ListenSummaryView, Painter, Spinner};

/// Arguments for the `listen` command.
#[derive(Debug, Args)]
pub struct ListenArgs {
    /// Stop after this many notification packets. If omitted, listen until Ctrl+C.
    #[arg(long)]
    max_notifications: Option<usize>,
}

impl ListenArgs {
    /// Creates listen arguments with an optional notification limit.
    #[must_use]
    pub fn new(max_notifications: Option<usize>) -> Self {
        Self { max_notifications }
    }

    /// Returns the optional notification limit.
    #[must_use]
    pub(crate) fn max_notifications(&self) -> Option<usize> {
        self.max_notifications
    }
}

/// Executes the `listen` command.
pub(crate) async fn run<W>(
    client: Box<dyn HardwareClient>,
    args: &ListenArgs,
    out: &mut W,
    terminal_client: &dyn TerminalClient,
) -> Result<()>
where
    W: io::Write,
{
    run_with_limit(client, args.max_notifications(), out, terminal_client).await
}

/// Executes listen with an explicit notification limit.
pub(crate) async fn run_with_limit<W>(
    client: Box<dyn HardwareClient>,
    max_notifications: Option<usize>,
    out: &mut W,
    terminal_client: &dyn TerminalClient,
) -> Result<()>
where
    W: io::Write,
{
    let painter = Painter::new(terminal_client.stdout_is_terminal());
    let spinner = Spinner::new(terminal_client.stderr_is_terminal());
    let session = spinner
        .with_spinner(
            "Scanning for iDotMatrix devices and connecting...",
            || async move { client.prepare_listen_first_device(IDM_NAME_PREFIX).await },
        )
        .await?;

    writeln!(
        out,
        "{}",
        ListenReadyView::new(session.device(), session.initial_read(), &painter)
    )?;
    let mut write_error: Option<io::Error> = None;

    let summary = session
        .run(max_notifications, |index, payload| {
            if write_error.is_some() {
                return;
            }
            let view = ListenNotificationView::new(index, payload, &painter);
            if let Err(error) = writeln!(out, "{view}") {
                write_error = Some(error);
            }
        })
        .await?;

    if let Some(error) = write_error {
        return Err(error.into());
    }
    writeln!(out)?;
    writeln!(out, "{}", ListenSummaryView::new(&summary, &painter))?;

    Ok(())
}
