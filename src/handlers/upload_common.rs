use std::time::Duration;

use thiserror::Error;
use tokio::time::{sleep, timeout};
use tokio_stream::StreamExt;
use tracing::{Span, instrument};

use super::transport_chunk_sizer::AdaptiveChunkSizer;
use crate::error::{InteractionError, ProtocolError};
use crate::hw::{DeviceSession, NotificationSubscription};
use crate::{NotificationDecodeError, NotifyEvent, TransferFamily};

const DRAIN_NOTIFICATION_TIMEOUT: Duration = Duration::from_millis(25);
const MAX_STALE_NOTIFICATION_DRAIN: usize = 8;
const UNUSABLE_WRITE_WITHOUT_RESPONSE_LIMIT: usize = 20;

/// Baseline and probe chunk sizes resolved for one upload session.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) struct UploadChunkSizing {
    reported_limit: Option<usize>,
    fallback_chunk: usize,
    baseline_chunk_size: usize,
    initial_probe_chunk_size: usize,
}

impl UploadChunkSizing {
    /// Returns the reported session write-without-response limit.
    #[must_use]
    pub(super) const fn reported_limit(self) -> Option<usize> {
        self.reported_limit
    }

    /// Returns the configured conservative fallback chunk.
    #[must_use]
    pub(super) const fn fallback_chunk(self) -> usize {
        self.fallback_chunk
    }

    /// Returns the baseline chunk size selected from session metadata.
    #[must_use]
    pub(super) const fn baseline_chunk_size(self) -> usize {
        self.baseline_chunk_size
    }

    /// Returns the first chunk size used by adaptive probing.
    #[must_use]
    pub(super) const fn initial_probe_chunk_size(self) -> usize {
        self.initial_probe_chunk_size
    }

    /// Returns whether adaptive probing starts above baseline.
    #[must_use]
    pub(super) const fn probing_enabled(self) -> bool {
        self.initial_probe_chunk_size > self.baseline_chunk_size
    }

    /// Returns whether baseline selection resolved to the fallback value.
    #[must_use]
    pub(super) const fn using_fallback_baseline(self) -> bool {
        self.baseline_chunk_size == self.fallback_chunk
    }
}

/// Resolves upload chunk sizing from session metadata and adaptive probing rules.
#[must_use]
pub(super) fn resolve_upload_chunk_sizing(session: &DeviceSession) -> UploadChunkSizing {
    let fallback_chunk = session.device_profile().write_without_response_fallback();
    let reported_limit = session.write_without_response_limit();
    let baseline_chunk_size = match reported_limit {
        Some(limit) if limit > UNUSABLE_WRITE_WITHOUT_RESPONSE_LIMIT => limit,
        _ => fallback_chunk,
    };
    let initial_probe_chunk_size = AdaptiveChunkSizer::from_baseline(baseline_chunk_size).current();

    UploadChunkSizing {
        reported_limit,
        fallback_chunk,
        baseline_chunk_size,
        initial_probe_chunk_size,
    }
}

/// Applies optional per-fragment pacing delay.
pub(super) async fn apply_fragment_delay(delay: Duration) {
    if !delay.is_zero() {
        sleep(delay).await;
    }
}

/// Computes remaining transport fragments for progress accounting.
#[must_use]
pub(super) fn remaining_transport_chunks(
    logical_chunk_sizes: &[usize],
    current_index: usize,
    current_block_offset: usize,
    chunk_size: usize,
) -> usize {
    let current_remaining = logical_chunk_sizes[current_index].saturating_sub(current_block_offset);
    current_remaining.div_ceil(chunk_size)
        + logical_chunk_sizes
            .iter()
            .skip(current_index + 1)
            .map(|block_len| block_len.div_ceil(chunk_size))
            .sum::<usize>()
}

/// Drains stale notifications before starting a new transfer.
#[instrument(skip(stream), level = "trace", fields(upload_kind))]
pub(super) async fn drain_stale_notifications(
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
pub(super) enum UploadAckOutcome {
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
pub(super) async fn wait_for_transfer_ack(
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
                },
            }
        }
    }
}
