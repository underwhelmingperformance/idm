use std::sync::Arc;
use std::time::{Duration, Instant};

use bon::Builder;
use tokio_util::sync::CancellationToken;
use tracing::{instrument, trace};

use super::chunk_sizer::AdaptiveChunkSizer;
use crate::error::ProtocolError;
use crate::handlers::upload_common::{
    UploadAckError, UploadAckOutcome, drain_stale_notifications, wait_for_transfer_ack,
};
use crate::hw::NotificationSubscription;
use crate::hw::hardware::{ConnectedBleSession, DeviceSession, WriteMode};
use crate::notification::TransferFamily;
use crate::protocol::EndpointId;

pub(crate) const DEFAULT_FRAGMENT_DELAY: Duration = Duration::from_millis(20);
pub(crate) const DEFAULT_ACK_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) const LOGICAL_CHUNK_SIZE: usize = 4096;

const UNUSABLE_WRITE_WITHOUT_RESPONSE_LIMIT: usize = 20;

/// Stats returned by a write operation.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct WriteStats {
    pub(crate) bytes_written: usize,
    pub(crate) chunks_written: usize,
    pub(crate) logical_chunks_sent: usize,
    pub(crate) total_logical_chunks: usize,
}

/// Acknowledgement level for a write operation.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum Ack {
    /// ATT write-without-response. No transfer ack.
    None,
    /// ATT write-with-response. No transfer ack.
    Transport,
    /// Wait for device-protocol notify ack after each logical chunk.
    /// Default ATT mode is WithoutResponse; override via `write_mode`.
    Transfer(TransferFamily),
}

/// Per-chunk header encoding closure.
///
/// Arguments: `(logical_chunk, chunk_index, total_payload_len, crc32)`.
pub(crate) type HeaderEncoder = dyn Fn(&[u8], usize, u32, u32) -> Result<Vec<u8>, ProtocolError>;

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

/// The single write path for all device communication.
///
/// `SessionWriter` handles the full lifecycle of sending data to an
/// iDotMatrix device: CRC32 computation, 4 KiB logical chunking,
/// per-chunk header encoding, transport fragmentation with adaptive
/// chunk sizing, and transfer-acknowledgement flow control.
///
/// # Two modes of operation
///
/// Without a `header`, the payload is written directly to the device
/// as a single logical frame, fragmented only at the transport layer.
/// This is the path used by control handlers (brightness, power,
/// time-sync, colour, screen-light-timeout).
///
/// With a `header`, the payload is CRC'd, split into 4096-byte
/// logical chunks, and each chunk is prepended with a protocol header
/// produced by the caller's [`HeaderEncoder`] closure. This is the
/// path used by upload handlers (text, GIF, image, DIY).
///
/// # Acknowledgement levels
///
/// The [`Ack`] parameter controls both the ATT write mode and whether
/// the writer waits for a device-protocol notify after each chunk:
///
/// - [`Ack::None`] — ATT write-without-response, no notify wait.
/// - [`Ack::Transport`] — ATT write-with-response, no notify wait.
/// - [`Ack::Transfer`] — ATT write-without-response (overridable via
///   `write_mode`), plus a notify-ack wait after each logical chunk.
///
/// # Notification stream ownership
///
/// When `ack` is [`Ack::Transfer`] and no external `stream` is
/// provided, `send()` creates an internal notification subscription
/// and drains stale events before the first chunk. When a `stream` is
/// provided, the caller owns its lifecycle — no drain is performed,
/// which allows long-lived streams to be reused across multiple
/// uploads (as DIY active mode does).
///
/// # Early finish
///
/// When `allow_early_finish` is set and the device reports `Finished`
/// before all chunks are sent, `send()` returns `Ok` with partial
/// stats instead of an error. The caller inspects
/// `logical_chunks_sent` vs `total_logical_chunks` to distinguish a
/// cache hit from a complete transfer. When `allow_early_finish` is
/// not set, a premature `Finished` returns
/// [`UploadAckError::PrematureFinish`].
#[derive(Builder)]
pub(crate) struct SessionWriter<'a> {
    /// Active device session to write through.
    session: &'a DeviceSession,

    /// Raw payload bytes. For control commands this is the encoded
    /// frame; for uploads this is the logical media payload (before
    /// chunking and header prepending).
    payload: &'a [u8],

    /// Acknowledgement level — determines ATT write mode and whether
    /// to wait for device-protocol notify acks.
    ack: Ack,

    /// Overrides the ATT write mode derived from `ack`. Needed when a
    /// transfer family requires a non-default mode (e.g. DIY uses
    /// `Transfer(Diy)` + `WithResponse`).
    write_mode: Option<WriteMode>,

    /// Per-chunk header encoder. When present, `send()` CRCs the
    /// payload, splits it into 4096-byte logical chunks, and calls
    /// this closure for each to produce the protocol header bytes.
    /// When absent, the payload is sent as a single logical frame.
    header: Option<&'a HeaderEncoder>,

    /// External notification subscription. When provided with
    /// `Ack::Transfer`, `send()` reads acks from this stream instead
    /// of creating one internally. The caller is responsible for
    /// draining stale events before the first call.
    stream: Option<&'a mut NotificationSubscription>,

    /// When `true`, a device `Finished` on a non-final chunk returns
    /// `Ok(stats)` with partial counts rather than an error. Used by
    /// the GIF handler for cache-hit detection.
    #[builder(default = false)]
    allow_early_finish: bool,
}

impl<'a> SessionWriter<'a> {
    /// Consumes the writer and sends the payload to the device.
    ///
    /// Returns [`WriteStats`] with byte, transport-chunk, and
    /// logical-chunk counts. On `Ack::Transfer`, blocks on a notify
    /// ack after each logical chunk, with a 5-second timeout.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError`] on transport write failure, ack
    /// timeout, transfer rejection, or (when `allow_early_finish` is
    /// false) premature device `Finished`.
    #[instrument(
        skip_all,
        level = "trace",
        fields(
            payload_len,
            total_logical_chunks,
            logical_chunks_sent,
            bytes_written,
            chunks_written,
        )
    )]
    pub(crate) async fn send(self) -> Result<WriteStats, ProtocolError> {
        let span = tracing::Span::current();
        let SessionWriter {
            session,
            payload,
            ack,
            write_mode,
            header,
            mut stream,
            allow_early_finish,
        } = self;
        span.record("payload_len", payload.len());

        let write_mode = write_mode.unwrap_or(match ack {
            Ack::None | Ack::Transfer(_) => WriteMode::WithoutResponse,
            Ack::Transport => WriteMode::WithResponse,
        });

        let encoder = match header {
            None => {
                let frag_stats = session.write(payload, write_mode).await?;
                let stats = WriteStats {
                    bytes_written: frag_stats.bytes_written,
                    chunks_written: frag_stats.chunks_written,
                    logical_chunks_sent: 1,
                    total_logical_chunks: 1,
                };
                tracing::record_all!(
                    span,
                    total_logical_chunks = 1usize,
                    logical_chunks_sent = 1usize,
                    bytes_written = stats.bytes_written,
                    chunks_written = stats.chunks_written
                );
                return Ok(stats);
            }
            Some(encoder) => encoder,
        };

        let crc32 = crc32fast::hash(payload);
        let payload_len = u32::try_from(payload.len()).map_err(|_overflow| {
            crate::handlers::FrameCodecError::HeaderPayloadTooLarge {
                payload_len: u16::MAX,
                max_payload_len: u16::MAX,
            }
        })?;
        let total_logical_chunks = payload.chunks(LOGICAL_CHUNK_SIZE).count();
        span.record("total_logical_chunks", total_logical_chunks);

        let transfer_family = match ack {
            Ack::Transfer(family) => Some(family),
            _ => None,
        };

        let mut internal_stream = if transfer_family.is_some() && stream.is_none() {
            let endpoint = EndpointId::ReadNotifyCharacteristic;
            let mut s = session
                .notification_stream(endpoint, None, CancellationToken::new())
                .await?;
            drain_stale_notifications(&mut s, "session_writer").await?;
            Some(s)
        } else {
            None
        };

        let mut bytes_written = 0usize;
        let mut chunks_written = 0usize;
        let mut logical_chunks_sent = 0usize;

        for (index, logical_chunk) in payload.chunks(LOGICAL_CHUNK_SIZE).enumerate() {
            let header_bytes = encoder(logical_chunk, index, payload_len, crc32)?;
            let mut frame_block = Vec::with_capacity(header_bytes.len() + logical_chunk.len());
            frame_block.extend_from_slice(&header_bytes);
            frame_block.extend_from_slice(logical_chunk);

            let frag_stats = session.write(&frame_block, write_mode).await?;
            bytes_written += frag_stats.bytes_written;
            chunks_written += frag_stats.chunks_written;
            logical_chunks_sent += 1;

            if let Some(family) = transfer_family {
                let ack_stream = if let Some(s) = stream.as_deref_mut() {
                    s
                } else {
                    internal_stream
                        .as_mut()
                        .expect("internal stream must exist for Transfer ack")
                };
                let ack_started = Instant::now();
                match wait_for_transfer_ack(ack_stream, DEFAULT_ACK_TIMEOUT, family).await {
                    Ok(UploadAckOutcome::Continue) => {
                        trace!(
                            logical_chunk_index = index,
                            ack_outcome = "continue",
                            ack_latency_ms = ack_started.elapsed().as_millis() as u64,
                            "logical chunk acknowledged"
                        );
                    }
                    Ok(UploadAckOutcome::Finished) => {
                        trace!(
                            logical_chunk_index = index,
                            ack_outcome = "finished",
                            ack_latency_ms = ack_started.elapsed().as_millis() as u64,
                            "transfer finished"
                        );
                        let chunk_number = index + 1;
                        if chunk_number < total_logical_chunks {
                            if allow_early_finish {
                                let stats = WriteStats {
                                    bytes_written,
                                    chunks_written,
                                    logical_chunks_sent,
                                    total_logical_chunks,
                                };
                                tracing::record_all!(
                                    span,
                                    logical_chunks_sent = stats.logical_chunks_sent,
                                    bytes_written = stats.bytes_written,
                                    chunks_written = stats.chunks_written
                                );
                                return Ok(stats);
                            }
                            return Err(UploadAckError::PrematureFinish {
                                chunk_index: chunk_number,
                                total_chunks: total_logical_chunks,
                            }
                            .into());
                        }
                        break;
                    }
                    Err(error) => {
                        trace!(
                            logical_chunk_index = index,
                            ack_outcome = "error",
                            ack_latency_ms = ack_started.elapsed().as_millis() as u64,
                            "logical chunk ack error"
                        );
                        return Err(error.into());
                    }
                }
            }
        }

        let stats = WriteStats {
            bytes_written,
            chunks_written,
            logical_chunks_sent,
            total_logical_chunks,
        };
        tracing::record_all!(
            span,
            logical_chunks_sent = stats.logical_chunks_sent,
            bytes_written = stats.bytes_written,
            chunks_written = stats.chunks_written
        );
        Ok(stats)
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
    async fn write(
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
            logical_chunks_sent: 0,
            total_logical_chunks: 0,
        })
    }
}
