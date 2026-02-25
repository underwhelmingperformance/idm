use std::time::Duration;

use thiserror::Error;
use tokio::time::{sleep, timeout};
use tokio_stream::StreamExt;
use tracing::{Span, instrument};

use crate::error::{InteractionError, ProtocolError};
use crate::hw::NotificationSubscription;
use crate::{NotificationDecodeError, NotifyEvent, TransferFamily};

const DRAIN_NOTIFICATION_TIMEOUT: Duration = Duration::from_millis(25);
const MAX_STALE_NOTIFICATION_DRAIN: usize = 8;

/// Applies optional per-fragment pacing delay.
pub(crate) async fn apply_fragment_delay(delay: Duration) {
    if !delay.is_zero() {
        sleep(delay).await;
    }
}

/// Drains stale notifications before starting a new transfer.
#[instrument(skip(stream), level = "trace", fields(upload_kind))]
pub(crate) async fn drain_stale_notifications(
    stream: &mut NotificationSubscription,
    upload_kind: &'static str,
) -> Result<(), ProtocolError> {
    let mut drained_count = 0usize;
    for _attempt in 0..MAX_STALE_NOTIFICATION_DRAIN {
        match timeout(DRAIN_NOTIFICATION_TIMEOUT, stream.next()).await {
            Err(_elapsed) => break,
            Ok(None) => break,
            Ok(Some(Err(error))) => return Err(error.into()),
            Ok(Some(Ok(_message))) => {
                drained_count += 1;
            }
        }
    }

    if drained_count > 0 {
        tracing::trace!(
            upload_kind,
            drained_notifications = drained_count,
            "drained stale notifications before upload"
        );
    }

    Ok(())
}

/// Common transfer acknowledgement outcomes.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum UploadAckOutcome {
    /// Transfer should continue with the next logical chunk.
    Continue,
    /// Transfer is complete.
    Finished,
}

/// Errors returned while waiting for transfer acknowledgements.
#[derive(Debug, Error)]
pub enum UploadAckError {
    #[error("notification acknowledgement timed out after {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },
    #[error("notification stream ended before an acknowledgement was received")]
    MissingAck,
    #[error("received unexpected notification while waiting for an acknowledgement")]
    UnexpectedEvent,
    #[error("transfer was rejected by device status 0x{status:02X}")]
    TransferRejected { status: u8 },
    #[error("device reported transfer completion at chunk {chunk_index} of {total_chunks}")]
    PrematureFinish {
        chunk_index: usize,
        total_chunks: usize,
    },
    #[error(transparent)]
    Interaction(#[from] InteractionError),
    #[error(transparent)]
    NotifyDecode(#[from] NotificationDecodeError),
}

/// Waits for one acknowledgement event for the requested transfer family.
#[instrument(
    skip(stream),
    level = "trace",
    fields(
        timeout_ms = timeout_duration.as_millis(),
        ?transfer_family,
        notify_event_kind = tracing::field::Empty,
        notify_status = tracing::field::Empty
    )
)]
pub(crate) async fn wait_for_transfer_ack(
    stream: &mut NotificationSubscription,
    timeout_duration: Duration,
    transfer_family: TransferFamily,
) -> Result<UploadAckOutcome, UploadAckError> {
    let span = Span::current();
    match timeout(timeout_duration, stream.next()).await {
        Err(_elapsed) => {
            span.record("notify_event_kind", "timeout");
            let timeout_ms = u64::try_from(timeout_duration.as_millis()).unwrap_or(u64::MAX);
            Err(UploadAckError::Timeout { timeout_ms })
        }
        Ok(None) => {
            span.record("notify_event_kind", "stream_closed");
            Err(UploadAckError::MissingAck)
        }
        Ok(Some(Err(error))) => {
            span.record("notify_event_kind", "stream_error");
            Err(UploadAckError::from(error))
        }
        Ok(Some(Ok(message))) => {
            let event = match message.event {
                Ok(event) => event,
                Err(error) => {
                    span.record("notify_event_kind", "decode_error");
                    return Err(UploadAckError::from(error));
                }
            };
            match event {
                NotifyEvent::NextPackage(family) if family == transfer_family => {
                    span.record("notify_event_kind", "next_package");
                    Ok(UploadAckOutcome::Continue)
                }
                NotifyEvent::Finished(family) if family == transfer_family => {
                    span.record("notify_event_kind", "finished");
                    Ok(UploadAckOutcome::Finished)
                }
                NotifyEvent::Error(family, status) if family == transfer_family => {
                    tracing::record_all!(
                        span,
                        notify_event_kind = "error",
                        notify_status = u64::from(status)
                    );
                    Err(UploadAckError::TransferRejected { status })
                }
                _other => {
                    span.record("notify_event_kind", "unexpected_event");
                    Err(UploadAckError::UnexpectedEvent)
                }
            }
        }
    }
}
