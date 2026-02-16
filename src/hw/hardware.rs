use async_trait::async_trait;
use tracing::info;

use super::DeviceRoutingProfile;
use super::btleplug_backend::BtleplugBackend;
use super::fake_backend::{FakeBackend, FakeBackendConfig};
use super::model::{
    EndpointPresence, FoundDevice, InspectReport, ListenSummary, NotificationRunSummary,
};
use super::model_overrides::ModelResolutionConfig;
use super::profile::DeviceProfile;
use crate::error::InteractionError;
use crate::protocol::EndpointId;

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

/// Connected session operations provided by concrete transports.
#[async_trait(?Send)]
pub(crate) trait ConnectedBleSession: Send {
    /// Returns connected device details.
    fn device(&self) -> &FoundDevice;

    /// Returns a fresh inspect report for this session.
    fn inspect_report(&self) -> InspectReport;

    /// Returns the negotiated write-without-response payload limit, if known.
    fn write_without_response_limit(&self) -> Option<usize>;

    /// Returns the resolved device profile for this session.
    fn device_profile(&self) -> DeviceProfile;

    /// Returns the resolved routing profile for this session, when available.
    fn device_routing_profile(&self) -> Option<DeviceRoutingProfile>;

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

    /// Runs the notification stream for one endpoint.
    async fn run_notifications(
        &self,
        endpoint: EndpointId,
        max_notifications: Option<usize>,
        on_notification: &mut dyn FnMut(usize, Vec<u8>),
    ) -> Result<NotificationRunSummary, InteractionError>;

    /// Closes the session and disconnects from the peripheral.
    async fn close(self: Box<Self>) -> Result<(), InteractionError>;
}

/// Low-level transport capable of establishing iDotMatrix sessions.
#[async_trait]
pub(crate) trait BleTransport: Send {
    /// Connects to the first peripheral matching `name_prefix`.
    async fn connect_first_matching(
        self,
        name_prefix: &str,
    ) -> Result<Box<dyn ConnectedBleSession>, InteractionError>;
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
    ) -> Result<Box<dyn ConnectedBleSession>, InteractionError> {
        let session = self.connect_first_matching_device(name_prefix).await?;
        Ok(Box::new(session))
    }
}

#[async_trait]
impl BleTransport for FakeBackend {
    async fn connect_first_matching(
        self,
        name_prefix: &str,
    ) -> Result<Box<dyn ConnectedBleSession>, InteractionError> {
        let session = self.connect_first_matching_device(name_prefix).await?;
        Ok(Box::new(session))
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
    session: Box<dyn ConnectedBleSession>,
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

    /// Returns the resolved routing profile for this session, when available.
    #[must_use]
    pub fn device_routing_profile(&self) -> Option<DeviceRoutingProfile> {
        self.session.device_routing_profile()
    }

    /// Reads one endpoint value.
    ///
    /// # Errors
    ///
    /// Returns an error if the endpoint is unavailable or the read fails.
    pub async fn read_endpoint(&self, endpoint: EndpointId) -> Result<Vec<u8>, InteractionError> {
        self.session.read_endpoint(endpoint).await
    }

    /// Reads one endpoint value and allows no-value backends.
    ///
    /// # Errors
    ///
    /// Returns an error if the endpoint is unavailable or the read fails.
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
    pub async fn subscribe_endpoint(&self, endpoint: EndpointId) -> Result<(), InteractionError> {
        self.session.subscribe_endpoint(endpoint).await
    }

    /// Unsubscribes endpoint notifications.
    ///
    /// # Errors
    ///
    /// Returns an error if unsubscription fails.
    pub async fn unsubscribe_endpoint(&self, endpoint: EndpointId) -> Result<(), InteractionError> {
        self.session.unsubscribe_endpoint(endpoint).await
    }

    /// Runs notification listening for one endpoint.
    ///
    /// # Errors
    ///
    /// Returns an error if notification handling fails.
    pub async fn run_notifications<F>(
        &self,
        endpoint: EndpointId,
        max_notifications: Option<usize>,
        mut on_notification: F,
    ) -> Result<NotificationRunSummary, InteractionError>
    where
        F: FnMut(usize, &[u8]),
    {
        let mut adapter = |index: usize, payload: Vec<u8>| on_notification(index, &payload);
        self.session
            .run_notifications(endpoint, max_notifications, &mut adapter)
            .await
    }

    /// Runs the standard iDotMatrix listen flow on `fa03` and closes the session.
    ///
    /// # Errors
    ///
    /// Returns an error if reads, subscriptions, notification handling, or teardown fails.
    pub async fn run_listen<F>(
        self,
        max_notifications: Option<usize>,
        mut on_notification: F,
    ) -> Result<ListenSummary, InteractionError>
    where
        F: FnMut(usize, &[u8]),
    {
        let device = self.device().clone();
        let endpoint = EndpointId::ReadNotifyCharacteristic;

        let initial_read = self.read_endpoint_optional(endpoint).await?;
        self.subscribe_endpoint(endpoint).await?;

        let run_result = self
            .session
            .run_notifications(endpoint, max_notifications, &mut |index, payload| {
                on_notification(index, &payload);
            })
            .await;

        if let Err(error) = self.unsubscribe_endpoint(endpoint).await {
            tracing::debug!(?error, "failed to unsubscribe cleanly");
        }

        self.close().await?;

        let run_result = run_result?;
        Ok(ListenSummary::new(
            device,
            initial_read,
            run_result.received_notifications(),
            run_result.stop_reason().clone(),
        ))
    }

    /// Closes the session and disconnects.
    ///
    /// # Errors
    ///
    /// Returns an error if teardown fails.
    pub async fn close(self) -> Result<(), InteractionError> {
        self.session.close().await
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

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
}
