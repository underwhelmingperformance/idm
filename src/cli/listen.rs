use clap::Args;
use std::io;

use anyhow::Result;
use serde::Serialize;
use tracing::instrument;

use crate::cli::OutputFormat;
use crate::hw::{HardwareClient, ListenSummary};
use crate::protocol::EndpointId;
use crate::terminal::TerminalClient;
use crate::{FoundDevice, NotificationHandler, NotifyEvent, TransferFamily};

use super::ui::{ListenNotificationView, ListenReadyView, ListenSummaryView, Painter};

/// NDJSON event emitted during a `listen` session.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ListenEvent<'a> {
    Ready {
        device: &'a FoundDevice,
        initial_read: Option<String>,
    },
    Notification {
        index: usize,
        payload: String,
        event_label: Option<String>,
    },
    Summary {
        #[serde(flatten)]
        data: &'a ListenSummary,
    },
}

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
    fields(max_notifications = ?args.max_notifications(), ?output_format)
)]
pub(crate) async fn run<W>(
    client: Box<dyn HardwareClient>,
    args: &ListenArgs,
    out: &mut W,
    terminal_client: &dyn TerminalClient,
    output_format: OutputFormat,
) -> Result<()>
where
    W: io::Write,
{
    run_with_limit(
        client,
        args.max_notifications(),
        out,
        terminal_client,
        output_format,
    )
    .await
}

/// Executes listen with an explicit notification limit.
#[instrument(
    skip(client, out, terminal_client),
    level = "info",
    fields(max_notifications = ?max_notifications, ?output_format)
)]
pub(crate) async fn run_with_limit<W>(
    client: Box<dyn HardwareClient>,
    max_notifications: Option<usize>,
    out: &mut W,
    terminal_client: &dyn TerminalClient,
    output_format: OutputFormat,
) -> Result<()>
where
    W: io::Write,
{
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

    match output_format {
        OutputFormat::Pretty => {
            let painter = Painter::new(terminal_client.stdout_is_terminal());
            writeln!(
                out,
                "{}",
                ListenReadyView::new(&device, initial_read.as_deref(), &painter)
            )?;
        }
        OutputFormat::Json => {
            serde_json::to_writer_pretty(
                &mut *out,
                &ListenEvent::Ready {
                    device: &device,
                    initial_read: initial_read.as_deref().map(hex::encode),
                },
            )?;
            writeln!(out)?;
        }
    }

    let mut write_error: Option<io::Error> = None;

    let run_result = session
        .run_notifications(endpoint, max_notifications, |index, payload| {
            if write_error.is_some() {
                return;
            }
            let event_label = decode_event_label(payload);
            let result = match output_format {
                OutputFormat::Pretty => {
                    let painter = Painter::new(terminal_client.stdout_is_terminal());
                    let view = ListenNotificationView::new(index, payload, event_label, &painter);
                    writeln!(out, "{view}")
                }
                OutputFormat::Json => serde_json::to_writer_pretty(
                    &mut *out,
                    &ListenEvent::Notification {
                        index,
                        payload: hex::encode(payload),
                        event_label,
                    },
                )
                .map_err(io::Error::other)
                .and_then(|()| writeln!(out)),
            };
            if let Err(error) = result {
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

    match output_format {
        OutputFormat::Pretty => {
            let painter = Painter::new(terminal_client.stdout_is_terminal());
            writeln!(out)?;
            writeln!(out, "{}", ListenSummaryView::new(&summary, &painter))?;
        }
        OutputFormat::Json => {
            serde_json::to_writer_pretty(&mut *out, &ListenEvent::Summary { data: &summary })?;
            writeln!(out)?;
        }
    }

    Ok(())
}

fn decode_event_label(payload: &[u8]) -> Option<String> {
    match NotificationHandler::decode(payload) {
        Ok(NotifyEvent::NextPackage(family)) => {
            Some(format!("{} next package", family_label(family)))
        }
        Ok(NotifyEvent::Finished(family)) => Some(format!("{} finished", family_label(family))),
        Ok(NotifyEvent::Error(family, status)) => {
            Some(format!("{} error ({status:#04X})", family_label(family)))
        }
        Ok(NotifyEvent::ScheduleSetup(status)) => Some(schedule_setup_message(status)),
        Ok(NotifyEvent::ScheduleMasterSwitch(status)) => {
            Some(schedule_master_switch_message(status))
        }
        Ok(NotifyEvent::LedInfo(response)) => Some(format!(
            "LED info: screen_type={} mcu={}.{} status={:#04X} password={}",
            response.screen_type,
            response.mcu_major_version,
            response.mcu_minor_version,
            response.status,
            if response.password_enabled {
                "on"
            } else {
                "off"
            }
        )),
        Ok(NotifyEvent::ScreenLightTimeout(value)) => {
            Some(format!("Screen-light timeout: {value}"))
        }
        Ok(NotifyEvent::Unknown(_unknown_payload)) => Some("Unknown event".to_string()),
        Err(error) => Some(format!("Decode error: {error}")),
    }
}

fn family_label(family: TransferFamily) -> &'static str {
    match family {
        TransferFamily::Text => "Text",
        TransferFamily::Gif => "GIF",
        TransferFamily::Image => "Image",
        TransferFamily::Diy => "DIY",
        TransferFamily::Timer => "Timer",
        TransferFamily::Ota => "OTA",
    }
}

fn schedule_setup_message(status: u8) -> String {
    match status {
        0x01 => "Schedule setup: success".to_string(),
        0x03 => "Schedule setup: continue".to_string(),
        other => format!("Schedule setup: failed ({other:#04X})"),
    }
}

fn schedule_master_switch_message(status: u8) -> String {
    match status {
        0x01 => "Schedule master switch: success".to_string(),
        other => format!("Schedule master switch: failed ({other:#04X})"),
    }
}
