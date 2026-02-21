use std::time::Duration;

use derive_more::Display;
use tokio::time::timeout;
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use crate::error::{InteractionError, ProtocolError};
use crate::hw::diagnostics::{DiagnosticRow, DiagnosticSectionSnapshot};
use crate::hw::{DeviceSession, WriteMode};
use crate::notification::NotifyEvent;
use crate::protocol::EndpointId;
use crate::utils::format_hex;

use super::{FrameCodec, FrameCodecError};

const SCREEN_LIGHT_COMMAND_ID: u8 = 0x0F;
const SCREEN_LIGHT_NAMESPACE: u8 = 0x80;
const SCREEN_LIGHT_READ_SENTINEL: u8 = 0xFF;
const SCREEN_LIGHT_QUERY_TIMEOUT: Duration = Duration::from_millis(1_000);

#[derive(Debug, Clone, Copy, Eq, PartialEq, Display)]
pub enum ScreenLightTimeoutProbeOutcome {
    #[display("no_response")]
    NoResponse,
    #[display("invalid_response")]
    InvalidResponse,
    #[display("parsed_notify")]
    ParsedNotify,
    #[display("parsed_read")]
    ParsedRead,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ScreenLightTimeoutProbe {
    timeout: Option<u8>,
    outcome: ScreenLightTimeoutProbeOutcome,
    write_modes_attempted: Vec<String>,
    last_payload: Option<Vec<u8>>,
}

impl ScreenLightTimeoutProbe {
    fn resolved(
        timeout: u8,
        outcome: ScreenLightTimeoutProbeOutcome,
        write_modes_attempted: Vec<String>,
    ) -> Self {
        Self {
            timeout: Some(timeout),
            outcome,
            write_modes_attempted,
            last_payload: None,
        }
    }

    fn unresolved(
        outcome: ScreenLightTimeoutProbeOutcome,
        write_modes_attempted: Vec<String>,
        last_payload: Option<Vec<u8>>,
    ) -> Self {
        Self {
            timeout: None,
            outcome,
            write_modes_attempted,
            last_payload,
        }
    }

    /// Returns the decoded timeout value, when a response was parsed.
    ///
    /// ```no_run
    /// # async fn demo(session: idm::DeviceSession) -> Result<(), idm::ProtocolError> {
    /// let probe = idm::ScreenLightTimeoutHandler::read_timeout(&session).await?;
    /// assert_eq!(Some(30), probe.timeout());
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn timeout(&self) -> Option<u8> {
        self.timeout
    }

    /// Returns the query outcome.
    ///
    /// ```no_run
    /// # async fn demo(session: idm::DeviceSession) -> Result<(), idm::ProtocolError> {
    /// let probe = idm::ScreenLightTimeoutHandler::read_timeout(&session).await?;
    /// let _ = probe.outcome();
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn outcome(&self) -> ScreenLightTimeoutProbeOutcome {
        self.outcome
    }

    /// Returns write-mode attempts recorded during the probe.
    ///
    /// ```no_run
    /// # async fn demo(session: idm::DeviceSession) -> Result<(), idm::ProtocolError> {
    /// let probe = idm::ScreenLightTimeoutHandler::read_timeout(&session).await?;
    /// let _ = probe.write_modes_attempted();
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn write_modes_attempted(&self) -> &[String] {
        &self.write_modes_attempted
    }

    /// Returns the last invalid payload observed during readback, when present.
    ///
    /// ```no_run
    /// # async fn demo(session: idm::DeviceSession) -> Result<(), idm::ProtocolError> {
    /// let probe = idm::ScreenLightTimeoutHandler::read_timeout(&session).await?;
    /// let _ = probe.last_payload();
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn last_payload(&self) -> Option<&[u8]> {
        self.last_payload.as_deref()
    }

    pub(crate) fn diagnostics_section(&self) -> DiagnosticSectionSnapshot {
        let timeout_display = self
            .timeout
            .map_or_else(|| "<none>".to_string(), |value| value.to_string());
        let write_modes_display = if self.write_modes_attempted.is_empty() {
            "<none>".to_string()
        } else {
            self.write_modes_attempted.join(",")
        };
        let last_payload_display = self
            .last_payload
            .as_deref()
            .map_or_else(|| "<none>".to_string(), format_hex);

        DiagnosticSectionSnapshot::new(
            "screen_light_timeout_probe".to_string(),
            "Screen-light timeout probe".to_string(),
            vec![
                DiagnosticRow::new("Query outcome", self.outcome),
                DiagnosticRow::new("Write modes attempted", write_modes_display),
                DiagnosticRow::new("Timeout value", timeout_display),
                DiagnosticRow::new("Last payload", last_payload_display),
            ],
        )
    }
}

enum ProbeStep {
    Parsed(u8),
    InvalidPayload(Vec<u8>),
    NoResponse,
}

enum NotifyProbeStep {
    Parsed(u8),
    NoResponse,
}

/// Handler for screen-light timeout set/read operations.
pub struct ScreenLightTimeoutHandler;

impl ScreenLightTimeoutHandler {
    fn mode_label(mode: WriteMode) -> &'static str {
        match mode {
            WriteMode::WithResponse => "with_response",
            WriteMode::WithoutResponse => "without_response",
        }
    }

    fn set_frame(timeout_value: u8) -> Result<Vec<u8>, FrameCodecError> {
        FrameCodec::encode_short(
            SCREEN_LIGHT_COMMAND_ID,
            SCREEN_LIGHT_NAMESPACE,
            &[timeout_value],
        )
    }

    fn read_frame() -> Result<Vec<u8>, FrameCodecError> {
        FrameCodec::encode_short(
            SCREEN_LIGHT_COMMAND_ID,
            SCREEN_LIGHT_NAMESPACE,
            &[SCREEN_LIGHT_READ_SENTINEL],
        )
    }

    fn parse_payload_timeout(payload: &[u8]) -> Option<u8> {
        if payload.len() < 5 {
            return None;
        }
        if payload[2] != SCREEN_LIGHT_COMMAND_ID || payload[3] != SCREEN_LIGHT_NAMESPACE {
            return None;
        }
        Some(payload[4])
    }

    async fn query_via_read(
        session: &DeviceSession,
        query: &[u8],
        mode: WriteMode,
    ) -> Result<ProbeStep, InteractionError> {
        session
            .write_endpoint(EndpointId::WriteCharacteristic, query, mode)
            .await?;
        match timeout(
            SCREEN_LIGHT_QUERY_TIMEOUT,
            session.read_endpoint_optional(EndpointId::ReadNotifyCharacteristic),
        )
        .await
        {
            Ok(Ok(Some(payload))) => {
                if let Some(timeout_value) = Self::parse_payload_timeout(&payload) {
                    Ok(ProbeStep::Parsed(timeout_value))
                } else {
                    Ok(ProbeStep::InvalidPayload(payload))
                }
            }
            Ok(Ok(None)) => Ok(ProbeStep::NoResponse),
            Ok(Err(error)) => Err(error),
            Err(_elapsed) => Ok(ProbeStep::NoResponse),
        }
    }

    async fn query_via_notify(
        session: &DeviceSession,
        query: &[u8],
        mode: WriteMode,
    ) -> Result<NotifyProbeStep, InteractionError> {
        let cancel = CancellationToken::new();
        let mut stream = session
            .notification_stream(EndpointId::ReadNotifyCharacteristic, None, cancel)
            .await?;
        session
            .write_endpoint(EndpointId::WriteCharacteristic, query, mode)
            .await?;

        let deadline = tokio::time::Instant::now() + SCREEN_LIGHT_QUERY_TIMEOUT;
        loop {
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return Ok(NotifyProbeStep::NoResponse);
            }
            let remaining = deadline - now;
            match timeout(remaining, stream.next()).await {
                Ok(Some(Ok(message))) => {
                    if let Ok(NotifyEvent::ScreenLightTimeout(timeout_value)) = message.event {
                        return Ok(NotifyProbeStep::Parsed(timeout_value));
                    }
                }
                Ok(Some(Err(error))) => return Err(error),
                Ok(None) | Err(_) => return Ok(NotifyProbeStep::NoResponse),
            }
        }
    }

    /// Reads the current screen-light timeout value from the connected device.
    ///
    /// ```
    /// # async fn demo(session: idm::DeviceSession) -> Result<(), idm::ProtocolError> {
    /// use idm::ScreenLightTimeoutHandler;
    ///
    /// let probe = ScreenLightTimeoutHandler::read_timeout(&session).await?;
    /// let _ = probe.timeout();
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error when frame encoding fails or all transport attempts fail.
    #[instrument(skip(session), level = "debug")]
    pub async fn read_timeout(
        session: &DeviceSession,
    ) -> Result<ScreenLightTimeoutProbe, ProtocolError> {
        let frame = Self::read_frame()?;
        let mut write_modes_attempted = Vec::with_capacity(4);
        let mut last_payload = None;
        let mut first_transport_error = None;

        for mode in [WriteMode::WithoutResponse, WriteMode::WithResponse] {
            write_modes_attempted
                .push(format!("{}:read_screen_light_read", Self::mode_label(mode)));
            match Self::query_via_read(session, &frame, mode).await {
                Ok(ProbeStep::Parsed(timeout_value)) => {
                    return Ok(ScreenLightTimeoutProbe::resolved(
                        timeout_value,
                        ScreenLightTimeoutProbeOutcome::ParsedRead,
                        write_modes_attempted,
                    ));
                }
                Ok(ProbeStep::InvalidPayload(payload)) => {
                    last_payload = Some(payload);
                    continue;
                }
                Ok(ProbeStep::NoResponse) => {}
                Err(error) => {
                    if first_transport_error.is_none() {
                        first_transport_error = Some(error);
                    }
                }
            }

            write_modes_attempted.push(format!(
                "{}:read_screen_light_notify",
                Self::mode_label(mode)
            ));
            match Self::query_via_notify(session, &frame, mode).await {
                Ok(NotifyProbeStep::Parsed(timeout_value)) => {
                    return Ok(ScreenLightTimeoutProbe::resolved(
                        timeout_value,
                        ScreenLightTimeoutProbeOutcome::ParsedNotify,
                        write_modes_attempted,
                    ));
                }
                Ok(NotifyProbeStep::NoResponse) => {}
                Err(error) => {
                    if first_transport_error.is_none() {
                        first_transport_error = Some(error);
                    }
                }
            }
        }

        if let Some(payload) = last_payload {
            return Ok(ScreenLightTimeoutProbe::unresolved(
                ScreenLightTimeoutProbeOutcome::InvalidResponse,
                write_modes_attempted,
                Some(payload),
            ));
        }
        if let Some(error) = first_transport_error {
            return Err(error.into());
        }

        Ok(ScreenLightTimeoutProbe::unresolved(
            ScreenLightTimeoutProbeOutcome::NoResponse,
            write_modes_attempted,
            None,
        ))
    }

    /// Sets the screen-light timeout value on the connected device.
    ///
    /// ```
    /// # async fn demo(session: idm::DeviceSession) -> Result<(), idm::ProtocolError> {
    /// use idm::ScreenLightTimeoutHandler;
    ///
    /// ScreenLightTimeoutHandler::set_timeout(&session, 30).await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error when frame encoding fails or both write modes fail.
    #[instrument(skip(session), level = "debug", fields(timeout_value))]
    pub async fn set_timeout(
        session: &DeviceSession,
        timeout_value: u8,
    ) -> Result<(), ProtocolError> {
        let frame = Self::set_frame(timeout_value)?;
        let mut first_error = None;

        for mode in [WriteMode::WithoutResponse, WriteMode::WithResponse] {
            match session
                .write_endpoint(EndpointId::WriteCharacteristic, &frame, mode)
                .await
            {
                Ok(()) => return Ok(()),
                Err(error) => {
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
        }

        match first_error {
            Some(error) => Err(error.into()),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::*;

    #[test]
    fn set_frame_matches_protocol_shape() {
        let frame =
            ScreenLightTimeoutHandler::set_frame(30).expect("screen-light set frame should encode");
        assert_eq!(vec![0x05, 0x00, 0x0F, 0x80, 0x1E], frame);
    }

    #[test]
    fn read_frame_matches_protocol_shape() {
        let frame =
            ScreenLightTimeoutHandler::read_frame().expect("screen-light read frame should encode");
        assert_eq!(vec![0x05, 0x00, 0x0F, 0x80, 0xFF], frame);
    }

    #[rstest]
    #[case::valid_screen_light_response(&[0x05, 0x00, 0x0F, 0x80, 0x1E], Some(0x1E))]
    #[case::wrong_command_id(&[0x05, 0x00, 0x01, 0x80, 0x1E], None)]
    #[case::too_short_for_timeout(&[0x04, 0x00, 0x0F, 0x80], None)]
    fn parse_payload_timeout_matches_expected(
        #[case] payload: &[u8],
        #[case] expected: Option<u8>,
    ) {
        assert_eq!(
            expected,
            ScreenLightTimeoutHandler::parse_payload_timeout(payload)
        );
    }
}
