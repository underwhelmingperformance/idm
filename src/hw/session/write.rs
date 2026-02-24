use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::{instrument, trace};

use super::chunk_sizer::AdaptiveChunkSizer;
use crate::error::ProtocolError;
use crate::handlers::upload_common::{UploadAckError, UploadAckOutcome, wait_for_transfer_ack};
use crate::hw::NotificationSubscription;
use crate::hw::hardware::{ConnectedBleSession, DeviceSession, WriteMode};
use crate::notification::TransferFamily;
use crate::protocol::EndpointId;

pub(crate) const DEFAULT_FRAGMENT_DELAY: Duration = Duration::from_millis(20);
pub(crate) const DEFAULT_ACK_TIMEOUT: Duration = Duration::from_secs(5);

const UNUSABLE_WRITE_WITHOUT_RESPONSE_LIMIT: usize = 20;

/// Stats returned by a write operation.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct WriteStats {
    pub(crate) bytes_written: usize,
    pub(crate) chunks_written: usize,
}

/// Chunk-sizer resolution captured at session creation.
#[derive(Debug, Clone)]
pub(in crate::hw) struct ResolvedChunkSizer {
    pub(in crate::hw) chunk_sizer: Arc<AdaptiveChunkSizer>,
    pub(in crate::hw) reported_write_without_response_limit: Option<usize>,
    pub(in crate::hw) profile_write_chunk_fallback: usize,
    pub(in crate::hw) baseline_transport_chunk_limit: usize,
    pub(in crate::hw) adaptive_transport_chunk_limit_initial: usize,
}

/// Resolves the adaptive chunk sizer from connection metadata.
pub(in crate::hw) fn resolve_chunk_sizer(session: &dyn ConnectedBleSession) -> ResolvedChunkSizer {
    let fallback = session.device_profile().write_without_response_fallback();
    let reported = session.write_without_response_limit();
    let baseline = match reported {
        Some(limit) if limit > UNUSABLE_WRITE_WITHOUT_RESPONSE_LIMIT => limit,
        _ => fallback,
    };
    let chunk_sizer = Arc::new(AdaptiveChunkSizer::from_baseline(baseline));
    let adaptive_transport_chunk_limit_initial = chunk_sizer.current();
    trace!(
        reported_write_without_response_limit = reported,
        profile_write_chunk_fallback = fallback,
        baseline_transport_chunk_limit = baseline,
        adaptive_transport_chunk_limit_initial,
        "resolved adaptive transport chunk sizer"
    );
    ResolvedChunkSizer {
        chunk_sizer,
        reported_write_without_response_limit: reported,
        profile_write_chunk_fallback: fallback,
        baseline_transport_chunk_limit: baseline,
        adaptive_transport_chunk_limit_initial,
    }
}

impl DeviceSession {
    /// Writes a payload to the device.
    ///
    /// The payload is transparently split into transport-sized chunks with
    /// a default 20 ms pacing delay between successive writes. On write
    /// failure the chunk size is reduced and the failing chunk is retried.
    #[instrument(
        skip(self, frame),
        level = "trace",
        fields(
            ?write_mode,
            frame_len = frame.len(),
            adaptive_transport_chunk_limit_start = tracing::field::Empty,
            adaptive_transport_chunk_limit_end = tracing::field::Empty,
            chunks_written = tracing::field::Empty,
            bytes_written = tracing::field::Empty
        )
    )]
    pub(crate) async fn write(
        &self,
        frame: &[u8],
        write_mode: WriteMode,
    ) -> Result<WriteStats, ProtocolError> {
        let span = tracing::Span::current();
        let fragment_delay = DEFAULT_FRAGMENT_DELAY;
        let mut bytes_written = 0usize;
        let mut chunks_written = 0usize;
        let mut chunk_index = 0usize;
        let mut offset = 0usize;
        span.record(
            "adaptive_transport_chunk_limit_start",
            self.chunk_sizer.current(),
        );

        while offset < frame.len() {
            let chunk_size = self.chunk_sizer.current();
            let end = usize::min(offset + chunk_size, frame.len());
            let chunk = &frame[offset..end];

            match self
                .session
                .write_endpoint(EndpointId::WriteCharacteristic, chunk, write_mode)
                .await
            {
                Ok(()) => {
                    chunk_index = chunk_index.saturating_add(1);
                    trace!(
                        chunk_index,
                        chunk_len = chunk.len(),
                        chunk_limit = chunk_size,
                        chunk_offset_start = offset,
                        chunk_offset_end = end,
                        frame_len = frame.len(),
                        "wrote transport chunk"
                    );
                    bytes_written += chunk.len();
                    chunks_written += 1;
                    offset = end;
                    if !fragment_delay.is_zero() {
                        tokio::time::sleep(fragment_delay).await;
                    }
                }
                Err(error) => {
                    let previous = chunk_size;
                    if !self.chunk_sizer.reduce_on_failure() {
                        return Err(error.into());
                    }
                    tracing::debug!(
                        ?error,
                        previous_chunk_size = previous,
                        next_chunk_size = self.chunk_sizer.current(),
                        "write failed; reducing chunk size and retrying"
                    );
                }
            }
        }
        tracing::record_all!(
            span,
            adaptive_transport_chunk_limit_end = self.chunk_sizer.current(),
            chunks_written = chunks_written,
            bytes_written = bytes_written
        );

        Ok(WriteStats {
            bytes_written,
            chunks_written,
        })
    }

    /// Writes a payload then waits for one device acknowledgement.
    ///
    /// Combines [`write`](Self::write) with
    /// [`wait_for_transfer_ack`](crate::handlers::upload_common::wait_for_transfer_ack)
    /// using the default 5 s ack timeout.
    #[instrument(
        skip(self, frame, stream),
        level = "trace",
        fields(
            ?write_mode,
            ?transfer_family,
            frame_len = frame.len(),
            adaptive_transport_chunk_limit_start = tracing::field::Empty,
            ack_timeout_ms = DEFAULT_ACK_TIMEOUT.as_millis(),
            ack_outcome = tracing::field::Empty,
            ack_status = tracing::field::Empty,
            ack_latency_ms = tracing::field::Empty
        )
    )]
    pub(crate) async fn write_with_ack(
        &self,
        frame: &[u8],
        write_mode: WriteMode,
        stream: &mut NotificationSubscription,
        transfer_family: TransferFamily,
    ) -> Result<(WriteStats, UploadAckOutcome), ProtocolError> {
        let span = tracing::Span::current();
        span.record(
            "adaptive_transport_chunk_limit_start",
            self.chunk_sizer.current(),
        );
        let stats = self.write(frame, write_mode).await?;
        let ack_started = Instant::now();
        let ack_outcome =
            match wait_for_transfer_ack(stream, DEFAULT_ACK_TIMEOUT, transfer_family).await {
                Ok(ack_outcome) => {
                    let ack_latency_ms = ack_started.elapsed().as_millis();
                    tracing::record_all!(
                        span,
                        ack_outcome = ?ack_outcome,
                        ack_latency_ms = ack_latency_ms
                    );
                    ack_outcome
                }
                Err(error) => {
                    let ack_latency_ms = ack_started.elapsed().as_millis();
                    match &error {
                        UploadAckError::Timeout { .. } => {
                            tracing::record_all!(
                                span,
                                ack_outcome = "timeout",
                                ack_latency_ms = ack_latency_ms
                            );
                        }
                        UploadAckError::MissingAck => {
                            tracing::record_all!(
                                span,
                                ack_outcome = "missing_ack",
                                ack_latency_ms = ack_latency_ms
                            );
                        }
                        UploadAckError::UnexpectedEvent => {
                            tracing::record_all!(
                                span,
                                ack_outcome = "unexpected_event",
                                ack_latency_ms = ack_latency_ms
                            );
                        }
                        UploadAckError::TransferRejected { status } => {
                            tracing::record_all!(
                                span,
                                ack_outcome = "transfer_rejected",
                                ack_latency_ms = ack_latency_ms,
                                ack_status = u64::from(*status)
                            );
                        }
                        UploadAckError::Interaction(..) => {
                            tracing::record_all!(
                                span,
                                ack_outcome = "interaction_error",
                                ack_latency_ms = ack_latency_ms
                            );
                        }
                        UploadAckError::NotifyDecode(..) => {
                            tracing::record_all!(
                                span,
                                ack_outcome = "notify_decode_error",
                                ack_latency_ms = ack_latency_ms
                            );
                        }
                    }
                    return Err(error.into());
                }
            };
        Ok((stats, ack_outcome))
    }
}
