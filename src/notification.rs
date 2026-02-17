use thiserror::Error;
use tracing::instrument;

/// Typed notification events emitted by iDotMatrix devices.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum NotifyEvent {
    /// Per-chunk upload acknowledgement.
    ChunkAck,
    /// Upload completion acknowledgement.
    UploadComplete,
    /// Unrecognised notification payload preserved as raw bytes.
    Unknown(Vec<u8>),
}

/// Errors returned while decoding notification payloads.
#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum NotificationDecodeError {
    #[error("notification payload was empty")]
    EmptyPayload,
}

/// Decodes raw `fa03` notification payloads into typed events.
pub struct NotificationHandler;

impl NotificationHandler {
    /// Decodes one notification payload.
    ///
    /// # Errors
    ///
    /// Returns an error when the payload is empty.
    #[instrument(skip(payload), level = "trace", fields(payload_len = payload.len()))]
    pub fn decode(payload: &[u8]) -> Result<NotifyEvent, NotificationDecodeError> {
        if payload.is_empty() {
            return Err(NotificationDecodeError::EmptyPayload);
        }

        if payload == [0x05, 0x00, 0x01, 0x00, 0x01] {
            return Ok(NotifyEvent::ChunkAck);
        }

        if payload == [0x05, 0x00, 0x01, 0x00, 0x03] {
            return Ok(NotifyEvent::UploadComplete);
        }

        Ok(NotifyEvent::Unknown(payload.to_vec()))
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case([0x05, 0x00, 0x01, 0x00, 0x01], NotifyEvent::ChunkAck)]
    #[case([0x05, 0x00, 0x01, 0x00, 0x03], NotifyEvent::UploadComplete)]
    fn decode_maps_known_packets(#[case] payload: [u8; 5], #[case] expected: NotifyEvent) {
        let decoded =
            NotificationHandler::decode(&payload).expect("known packet should decode cleanly");
        assert_eq!(expected, decoded);
    }

    #[test]
    fn decode_preserves_unknown_payload() {
        let payload = [0xAA, 0x55, 0x01];
        let decoded = NotificationHandler::decode(&payload)
            .expect("unknown non-empty payload should decode as Unknown");
        assert_eq!(NotifyEvent::Unknown(payload.to_vec()), decoded);
    }

    #[test]
    fn decode_rejects_empty_payload() {
        let decoded = NotificationHandler::decode(&[]);
        assert_matches!(decoded, Err(NotificationDecodeError::EmptyPayload));
    }
}
