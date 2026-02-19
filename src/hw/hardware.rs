use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use async_trait::async_trait;
use tokio::time::timeout;
use tokio_stream::Stream;
use tokio_util::sync::CancellationToken;
use tracing::{info, instrument, trace};

use super::btleplug_backend::BtleplugBackend;
use super::fake_backend::{FakeBackend, FakeBackendConfig};
use super::model::{
    EndpointPresence, FoundDevice, InspectReport, ListenStopReason, NotificationRunSummary,
};
use super::model_overrides::ModelResolutionConfig;
use super::profile::DeviceProfile;
use crate::error::InteractionError;
use crate::notification::{NotificationDecodeError, NotificationHandler, NotifyEvent};
use crate::protocol::EndpointId;

const SESSION_CLOSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Creates a hardware client backed by the real BLE transport.
pub(crate) fn real_hardware_client() -> Box<dyn HardwareClient> {
    real_hardware_client_with_model_resolution(ModelResolutionConfig::default())
}

/// Creates a hardware client backed by the real BLE transport with model-resolution settings.
pub(crate) fn real_hardware_client_with_model_resolution(
    model_resolution: ModelResolutionConfig,
) -> Box<dyn HardwareClient> {
    Box::new(RealHardwareClient::new(model_resolution))
}

/// Creates a hardware client backed by fake BLE fixtures.
pub(crate) fn fake_hardware_client(config: FakeBackendConfig) -> Box<dyn HardwareClient> {
    info!("using fake BLE backend");
    Box::new(FakeHardwareClient::new(config))
}

/// Write mode used for characteristic writes.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum WriteMode {
    /// Use ATT write-with-response.
    WithResponse,
    /// Use ATT write-without-response.
    WithoutResponse,
}

/// Boxed stream of raw notification payloads from a BLE backend.
pub(crate) type PayloadStream = Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>;

/// Connected session operations provided by concrete transports.
#[async_trait]
pub(crate) trait ConnectedBleSession: Send + Sync {
    /// Returns connected device details.
    fn device(&self) -> &FoundDevice;

    /// Returns a fresh inspect report for this session.
    fn inspect_report(&self) -> InspectReport;

    /// Returns the negotiated write-without-response payload limit, if known.
    fn write_without_response_limit(&self) -> Option<usize>;

    /// Returns the resolved device profile for this session.
    fn device_profile(&self) -> DeviceProfile;

    /// Reads one endpoint value.
    async fn read_endpoint(&self, endpoint: EndpointId) -> Result<Vec<u8>, InteractionError>;

    /// Reads one endpoint value, returning `None` when unavailable.
    async fn read_endpoint_optional(
        &self,
        endpoint: EndpointId,
    ) -> Result<Option<Vec<u8>>, InteractionError>;

    /// Writes one payload to an endpoint.
    async fn write_endpoint(
        &self,
        endpoint: EndpointId,
        payload: &[u8],
        mode: WriteMode,
    ) -> Result<(), InteractionError>;

    /// Subscribes to notifications for an endpoint.
    async fn subscribe_endpoint(&self, endpoint: EndpointId) -> Result<(), InteractionError>;

    /// Unsubscribes notifications for an endpoint.
    async fn unsubscribe_endpoint(&self, endpoint: EndpointId) -> Result<(), InteractionError>;

    /// Returns a raw payload stream for one endpoint.
    ///
    /// The caller must have already subscribed via [`subscribe_endpoint`] before
    /// calling this method.  Each item is the raw `Vec<u8>` payload of one
    /// BLE notification for the given endpoint, with UUID filtering applied
    /// by the backend.
    async fn notification_payloads(
        &self,
        endpoint: EndpointId,
    ) -> Result<PayloadStream, InteractionError>;

    /// Closes the session and disconnects from the peripheral.
    async fn close(self: Arc<Self>) -> Result<(), InteractionError>;
}

/// Low-level transport capable of establishing iDotMatrix sessions.
#[async_trait]
pub(crate) trait BleTransport: Send {
    /// Connects to the first peripheral matching `name_prefix`.
    async fn connect_first_matching(
        self,
        name_prefix: &str,
    ) -> Result<Arc<dyn ConnectedBleSession>, InteractionError>;
}

/// Session builder over a selected BLE transport.
#[derive(Debug)]
pub(crate) struct SessionHandler<T: BleTransport> {
    transport: T,
}

impl<T: BleTransport> SessionHandler<T> {
    /// Creates a new session handler.
    pub(crate) fn new(transport: T) -> Self {
        Self { transport }
    }

    /// Connects to the first matching device and returns a session.
    #[instrument(skip(self), level = "debug", fields(prefix = name_prefix))]
    pub(crate) async fn connect_first(
        self,
        name_prefix: &str,
    ) -> Result<DeviceSession, InteractionError> {
        let session = self.transport.connect_first_matching(name_prefix).await?;
        Ok(DeviceSession { session })
    }
}

pub(crate) fn missing_required_endpoints(presence: &EndpointPresence) -> Vec<EndpointId> {
    const REQUIRED: [EndpointId; 3] = [
        EndpointId::ControlService,
        EndpointId::WriteCharacteristic,
        EndpointId::ReadNotifyCharacteristic,
    ];

    REQUIRED
        .into_iter()
        .filter(|endpoint| !presence.is_present(*endpoint))
        .collect()
}

#[async_trait]
impl BleTransport for BtleplugBackend {
    async fn connect_first_matching(
        self,
        name_prefix: &str,
    ) -> Result<Arc<dyn ConnectedBleSession>, InteractionError> {
        let session = self.connect_first_matching_device(name_prefix).await?;
        Ok(Arc::new(session))
    }
}

#[async_trait]
impl BleTransport for FakeBackend {
    async fn connect_first_matching(
        self,
        name_prefix: &str,
    ) -> Result<Arc<dyn ConnectedBleSession>, InteractionError> {
        let session = self.connect_first_matching_device(name_prefix).await?;
        Ok(Arc::new(session))
    }
}

#[async_trait]
pub trait HardwareClient: Send + Sync {
    /// Connects to the first matching iDotMatrix peripheral.
    async fn connect_first_device(
        self: Box<Self>,
        name_prefix: &str,
    ) -> Result<DeviceSession, InteractionError>;
}

#[derive(Debug)]
struct RealHardwareClient {
    model_resolution: ModelResolutionConfig,
}

impl RealHardwareClient {
    fn new(model_resolution: ModelResolutionConfig) -> Self {
        Self { model_resolution }
    }
}

#[async_trait]
impl HardwareClient for RealHardwareClient {
    #[instrument(skip(self), level = "info", fields(prefix = name_prefix))]
    async fn connect_first_device(
        self: Box<Self>,
        name_prefix: &str,
    ) -> Result<DeviceSession, InteractionError> {
        let Self { model_resolution } = *self;
        let backend = BtleplugBackend::new(model_resolution).await?;
        let handler = SessionHandler::new(backend);
        handler.connect_first(name_prefix).await
    }
}

#[derive(Debug)]
struct FakeHardwareClient {
    config: FakeBackendConfig,
}

impl FakeHardwareClient {
    fn new(config: FakeBackendConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl HardwareClient for FakeHardwareClient {
    #[instrument(skip(self), level = "info", fields(prefix = name_prefix))]
    async fn connect_first_device(
        self: Box<Self>,
        name_prefix: &str,
    ) -> Result<DeviceSession, InteractionError> {
        let Self { config } = *self;
        let backend = FakeBackend::new(config);
        let handler = SessionHandler::new(backend);
        handler.connect_first(name_prefix).await
    }
}

/// A connected iDotMatrix session.
pub struct DeviceSession {
    session: Arc<dyn ConnectedBleSession>,
}

/// One typed notification item emitted by [`DeviceSession::notification_stream`].
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct NotificationMessage {
    /// One-based index of the notification in this stream.
    pub index: usize,
    /// Parsed notification event or decode error for the payload.
    pub event: Result<NotifyEvent, NotificationDecodeError>,
}

/// Single-consumer notification stream tied to one endpoint subscription.
pub struct NotificationSubscription {
    payloads: Option<PayloadStream>,
    session: Arc<dyn ConnectedBleSession>,
    endpoint: EndpointId,
    max_notifications: Option<usize>,
    cancel: CancellationToken,
    received: usize,
    summary: Option<NotificationRunSummary>,
}

impl Stream for NotificationSubscription {
    type Item = Result<NotificationMessage, InteractionError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.payloads.is_none() {
            return Poll::Ready(None);
        }

        if let Some(0) = this.max_notifications {
            this.payloads = None;
            this.summary = Some(NotificationRunSummary::new(
                0,
                ListenStopReason::ReachedLimit(0),
            ));
            return Poll::Ready(None);
        }

        {
            let cancel_fut = std::pin::pin!(this.cancel.cancelled());
            if cancel_fut.poll(cx).is_ready() {
                this.payloads = None;
                this.summary = Some(NotificationRunSummary::new(
                    this.received,
                    ListenStopReason::Interrupted,
                ));
                return Poll::Ready(None);
            }
        }

        let poll_result = this.payloads.as_mut().unwrap().as_mut().poll_next(cx);
        match poll_result {
            Poll::Ready(Some(payload)) => {
                this.received += 1;
                let message = NotificationMessage {
                    index: this.received,
                    event: NotificationHandler::decode(&payload),
                };

                if let Some(limit) = this.max_notifications
                    && this.received >= limit
                {
                    this.payloads = None;
                    this.summary = Some(NotificationRunSummary::new(
                        this.received,
                        ListenStopReason::ReachedLimit(limit),
                    ));
                }

                Poll::Ready(Some(Ok(message)))
            }
            Poll::Ready(None) => {
                this.payloads = None;
                this.summary = Some(NotificationRunSummary::new(
                    this.received,
                    ListenStopReason::NotificationStreamClosed,
                ));
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Converts a completed notification subscription into its run summary.
///
/// Returns an error when the stream has not yet reached completion.
///
/// ```no_run
/// # async fn demo(session: &idm::DeviceSession) -> Result<(), idm::InteractionError> {
/// use std::convert::TryInto as _;
/// use tokio_stream::StreamExt;
/// use tokio_util::sync::CancellationToken;
///
/// let mut stream = session
///     .notification_stream(
///         idm::EndpointId::ReadNotifyCharacteristic,
///         Some(1),
///         CancellationToken::new(),
///     )
///     .await?;
///
/// while stream.next().await.is_some() {}
/// let summary: idm::NotificationRunSummary = stream.try_into()?;
/// let _ = summary.received_notifications();
/// # Ok(())
/// # }
/// ```
impl TryFrom<NotificationSubscription> for NotificationRunSummary {
    type Error = InteractionError;

    fn try_from(mut subscription: NotificationSubscription) -> Result<Self, Self::Error> {
        subscription
            .summary
            .take()
            .ok_or(InteractionError::NotificationStreamIncomplete)
    }
}

impl Drop for NotificationSubscription {
    fn drop(&mut self) {
        let endpoint = self.endpoint;
        let session = Arc::clone(&self.session);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                if let Err(error) = session.unsubscribe_endpoint(endpoint).await {
                    trace!(
                        ?error,
                        ?endpoint,
                        "failed to unsubscribe notification stream cleanly"
                    );
                }
            });
        }
    }
}

impl DeviceSession {
    /// Returns connected device details.
    #[must_use]
    pub fn device(&self) -> &FoundDevice {
        self.session.device()
    }

    /// Returns an inspect report derived from the active session.
    #[must_use]
    pub fn inspect_report(&self) -> InspectReport {
        self.session.inspect_report()
    }

    /// Returns the negotiated write-without-response payload limit, if known.
    #[must_use]
    pub fn write_without_response_limit(&self) -> Option<usize> {
        self.session.write_without_response_limit()
    }

    /// Returns the resolved device profile for this session.
    ///
    /// ```
    /// # async fn demo(client: Box<dyn idm::HardwareClient>) -> Result<(), idm::InteractionError> {
    /// let session = client.connect_first_device("IDM-").await?;
    /// let _profile = session.device_profile();
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn device_profile(&self) -> DeviceProfile {
        self.session.device_profile()
    }

    /// Reads one endpoint value.
    ///
    /// # Errors
    ///
    /// Returns an error if the endpoint is unavailable or the read fails.
    #[instrument(skip(self), level = "trace", fields(?endpoint))]
    pub async fn read_endpoint(&self, endpoint: EndpointId) -> Result<Vec<u8>, InteractionError> {
        self.session.read_endpoint(endpoint).await
    }

    /// Reads one endpoint value and allows no-value backends.
    ///
    /// # Errors
    ///
    /// Returns an error if the endpoint is unavailable or the read fails.
    #[instrument(skip(self), level = "trace", fields(?endpoint))]
    pub async fn read_endpoint_optional(
        &self,
        endpoint: EndpointId,
    ) -> Result<Option<Vec<u8>>, InteractionError> {
        self.session.read_endpoint_optional(endpoint).await
    }

    /// Writes one payload to an endpoint.
    ///
    /// # Errors
    ///
    /// Returns an error if the endpoint is unavailable or the write fails.
    #[instrument(skip(self, payload), level = "trace", fields(?endpoint, ?mode, payload_len = payload.len()))]
    pub async fn write_endpoint(
        &self,
        endpoint: EndpointId,
        payload: &[u8],
        mode: WriteMode,
    ) -> Result<(), InteractionError> {
        self.session.write_endpoint(endpoint, payload, mode).await
    }

    /// Subscribes to endpoint notifications.
    ///
    /// # Errors
    ///
    /// Returns an error if subscription fails.
    #[instrument(skip(self), level = "trace", fields(?endpoint))]
    pub async fn subscribe_endpoint(&self, endpoint: EndpointId) -> Result<(), InteractionError> {
        self.session.subscribe_endpoint(endpoint).await
    }

    /// Unsubscribes endpoint notifications.
    ///
    /// # Errors
    ///
    /// Returns an error if unsubscription fails.
    #[instrument(skip(self), level = "trace", fields(?endpoint))]
    pub async fn unsubscribe_endpoint(&self, endpoint: EndpointId) -> Result<(), InteractionError> {
        self.session.unsubscribe_endpoint(endpoint).await
    }

    /// Creates a typed notification stream for one endpoint.
    ///
    /// The stream subscribes before yielding the first item and performs
    /// best-effort unsubscription when the stream completes or is dropped.
    ///
    /// Pass a `cancel` token to allow external interruption (e.g. Ctrl-C).
    /// When the token is cancelled, the stream terminates with
    /// [`ListenStopReason::Interrupted`].
    ///
    /// # Errors
    ///
    /// Returns an error if the initial endpoint subscription fails.
    #[instrument(skip(self, cancel), level = "trace", fields(?endpoint, ?max_notifications))]
    pub async fn notification_stream(
        &self,
        endpoint: EndpointId,
        max_notifications: Option<usize>,
        cancel: CancellationToken,
    ) -> Result<NotificationSubscription, InteractionError> {
        self.session.subscribe_endpoint(endpoint).await?;

        let payloads = match self.session.notification_payloads(endpoint).await {
            Ok(payloads) => payloads,
            Err(error) => {
                let _ = self.session.unsubscribe_endpoint(endpoint).await;
                return Err(error);
            }
        };

        Ok(NotificationSubscription {
            payloads: Some(payloads),
            session: Arc::clone(&self.session),
            endpoint,
            max_notifications,
            cancel,
            received: 0,
            summary: None,
        })
    }

    /// Closes the session and disconnects.
    ///
    /// # Errors
    ///
    /// Returns an error if teardown fails.
    #[instrument(skip(self), level = "debug")]
    pub async fn close(self) -> Result<(), InteractionError> {
        match timeout(SESSION_CLOSE_TIMEOUT, self.session.close()).await {
            Ok(result) => result,
            Err(_elapsed) => {
                let timeout_ms =
                    u64::try_from(SESSION_CLOSE_TIMEOUT.as_millis()).unwrap_or(u64::MAX);
                Err(InteractionError::SessionCloseTimeout { timeout_ms })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::future::pending;
    use std::sync::Arc;

    use async_trait::async_trait;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::*;

    fn presence_with_flags(
        control_service: bool,
        write_characteristic: bool,
        read_notify_characteristic: bool,
    ) -> EndpointPresence {
        let by_endpoint = HashMap::from([
            (EndpointId::ControlService, control_service),
            (EndpointId::WriteCharacteristic, write_characteristic),
            (
                EndpointId::ReadNotifyCharacteristic,
                read_notify_characteristic,
            ),
        ]);
        EndpointPresence::new(by_endpoint)
    }

    #[rstest]
    #[case::all_present(presence_with_flags(true, true, true), vec![])]
    #[case::missing_write(
        presence_with_flags(true, false, true),
        vec![EndpointId::WriteCharacteristic],
    )]
    #[case::missing_all(
        presence_with_flags(false, false, false),
        vec![
            EndpointId::ControlService,
            EndpointId::WriteCharacteristic,
            EndpointId::ReadNotifyCharacteristic,
        ],
    )]
    fn missing_required_endpoints_returns_expected_list(
        #[case] presence: EndpointPresence,
        #[case] expected: Vec<EndpointId>,
    ) {
        let missing = missing_required_endpoints(&presence);
        assert_eq!(expected, missing);
    }

    struct HangingCloseSession {
        device: FoundDevice,
    }

    #[async_trait]
    impl ConnectedBleSession for HangingCloseSession {
        fn device(&self) -> &FoundDevice {
            &self.device
        }

        fn inspect_report(&self) -> InspectReport {
            panic!("inspect_report should not be called in this test");
        }

        fn write_without_response_limit(&self) -> Option<usize> {
            None
        }

        fn device_profile(&self) -> DeviceProfile {
            panic!("device_profile should not be called in this test");
        }

        async fn read_endpoint(&self, _endpoint: EndpointId) -> Result<Vec<u8>, InteractionError> {
            panic!("read_endpoint should not be called in this test");
        }

        async fn read_endpoint_optional(
            &self,
            _endpoint: EndpointId,
        ) -> Result<Option<Vec<u8>>, InteractionError> {
            panic!("read_endpoint_optional should not be called in this test");
        }

        async fn write_endpoint(
            &self,
            _endpoint: EndpointId,
            _payload: &[u8],
            _mode: WriteMode,
        ) -> Result<(), InteractionError> {
            panic!("write_endpoint should not be called in this test");
        }

        async fn subscribe_endpoint(&self, _endpoint: EndpointId) -> Result<(), InteractionError> {
            panic!("subscribe_endpoint should not be called in this test");
        }

        async fn unsubscribe_endpoint(
            &self,
            _endpoint: EndpointId,
        ) -> Result<(), InteractionError> {
            panic!("unsubscribe_endpoint should not be called in this test");
        }

        async fn notification_payloads(
            &self,
            _endpoint: EndpointId,
        ) -> Result<PayloadStream, InteractionError> {
            panic!("notification_payloads should not be called in this test");
        }

        async fn close(self: Arc<Self>) -> Result<(), InteractionError> {
            pending::<Result<(), InteractionError>>().await
        }
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn close_times_out_when_backend_close_stalls() {
        let session = DeviceSession {
            session: Arc::new(HangingCloseSession {
                device: FoundDevice::new(
                    "hci0".to_string(),
                    "aa:bb:cc".to_string(),
                    Some("IDM-Test".to_string()),
                    Some(-42),
                ),
            }),
        };

        let result = session.close().await;
        match result {
            Err(InteractionError::SessionCloseTimeout { timeout_ms }) => {
                assert_eq!(3_000, timeout_ms);
            }
            other => panic!("expected SessionCloseTimeout, got {other:?}"),
        }
    }
}
