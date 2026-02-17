use clap::Args;
use std::io;

use anyhow::Result;
use tracing::instrument;

use crate::hw::{HardwareClient, ListenSummary};
use crate::protocol::EndpointId;
use crate::terminal::TerminalClient;
use crate::{NotificationHandler, NotifyEvent, TransferFamily};

use super::ui::{ListenNotificationView, ListenReadyView, ListenSummaryView, Painter};

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
#[instrument(
    skip(client, args, out, terminal_client),
    level = "info",
    fields(max_notifications = ?args.max_notifications())
)]
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
#[instrument(
    skip(client, out, terminal_client),
    level = "info",
    fields(max_notifications = ?max_notifications)
)]
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
    let session = crate::SessionHandler::new(client).connect_first().await?;
    let device = session.device().clone();
    let endpoint = EndpointId::ReadNotifyCharacteristic;
    let initial_read = match session.read_endpoint_optional(endpoint).await {
        Ok(payload) => payload,
        Err(error) => {
            session.close().await?;
            return Err(error.into());
        }
    };
    if let Err(error) = session.subscribe_endpoint(endpoint).await {
        session.close().await?;
        return Err(error.into());
    }

    writeln!(
        out,
        "{}",
        ListenReadyView::new(&device, initial_read.as_deref(), &painter)
    )?;
    let mut write_error: Option<io::Error> = None;

    let run_result = session
        .run_notifications(endpoint, max_notifications, |index, payload| {
            if write_error.is_some() {
                return;
            }
            let event_label = match NotificationHandler::decode(payload) {
                Ok(NotifyEvent::NextPackage(family)) => {
                    Some(format!("next_package:{}", family_label(family)))
                }
                Ok(NotifyEvent::Finished(family)) => {
                    Some(format!("finished:{}", family_label(family)))
                }
                Ok(NotifyEvent::Error(family, status)) => {
                    Some(format!("error:{}:{status:#04X}", family_label(family)))
                }
                Ok(NotifyEvent::ScheduleSetup(status)) => {
                    Some(format!("schedule_setup:{status:#04X}"))
                }
                Ok(NotifyEvent::ScheduleMasterSwitch(status)) => {
                    Some(format!("schedule_master_switch:{status:#04X}"))
                }
                Ok(NotifyEvent::LedInfo(response)) => {
                    Some(format!("led_info:screen_type={}", response.screen_type))
                }
                Ok(NotifyEvent::ScreenLightTimeout(value)) => {
                    Some(format!("screen_light_timeout:{value}"))
                }
                Ok(NotifyEvent::Unknown(_unknown_payload)) => Some("unknown".to_string()),
                Err(error) => Some(format!("decode_error:{error}")),
            };
            let view = ListenNotificationView::new(index, payload, event_label, &painter);
            if let Err(error) = writeln!(out, "{view}") {
                write_error = Some(error);
            }
        })
        .await;

    if let Err(error) = session.unsubscribe_endpoint(endpoint).await {
        tracing::trace!(?error, "failed to unsubscribe cleanly");
    }
    session.close().await?;

    if let Some(error) = write_error {
        return Err(error.into());
    }
    let run_result = run_result?;
    let summary = ListenSummary::new(
        device,
        initial_read,
        run_result.received_notifications(),
        run_result.stop_reason().clone(),
    );
    writeln!(out)?;
    writeln!(out, "{}", ListenSummaryView::new(&summary, &painter))?;

    Ok(())
}

fn family_label(family: TransferFamily) -> &'static str {
    match family {
        TransferFamily::Text => "text",
        TransferFamily::Gif => "gif",
        TransferFamily::Image => "image",
        TransferFamily::Diy => "diy",
        TransferFamily::Timer => "timer",
        TransferFamily::Ota => "ota",
    }
}
