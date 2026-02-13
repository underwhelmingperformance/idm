use async_trait::async_trait;
use tracing::info;

use super::btleplug_backend::{BtleplugBackend, PreparedRealListen};
use super::fake_backend::{FakeBackend, FakeBackendConfig, PreparedFakeListen};
use super::model::{FoundDevice, InspectReport, ListenSummary};
use crate::error::InteractionError;

/// Runtime BLE backend selection.
#[derive(Debug)]
pub(crate) enum HardwareBackend {
    Real,
    Fake(FakeBackendConfig),
}

/// Builds an injected hardware client for the selected runtime backend.
pub(crate) async fn hardware_client_from_backend(
    backend: HardwareBackend,
) -> Result<Box<dyn HardwareClient>, InteractionError> {
    let client: Box<dyn HardwareClient> = match backend {
        HardwareBackend::Real => Box::new(RealHardwareClient::new().await?),
        HardwareBackend::Fake(config) => {
            info!("using fake BLE backend");
            Box::new(FakeHardwareClient::new(config))
        }
    };

    Ok(client)
}

#[async_trait]
pub trait HardwareClient: Send + Sync {
    /// Inspects the first matching iDotMatrix peripheral.
    async fn inspect_first_device(
        self: Box<Self>,
        name_prefix: &str,
    ) -> Result<InspectReport, InteractionError>;

    /// Prepares a listen session for the first matching iDotMatrix peripheral.
    async fn prepare_listen_first_device(
        self: Box<Self>,
        name_prefix: &str,
    ) -> Result<PreparedListenSession, InteractionError>;
}

#[derive(Debug)]
struct RealHardwareClient {
    backend: BtleplugBackend,
}

impl RealHardwareClient {
    async fn new() -> Result<Self, InteractionError> {
        Ok(Self {
            backend: BtleplugBackend::new().await?,
        })
    }
}

#[async_trait]
impl HardwareClient for RealHardwareClient {
    async fn inspect_first_device(
        self: Box<Self>,
        name_prefix: &str,
    ) -> Result<InspectReport, InteractionError> {
        self.backend
            .inspect_first_matching_device(name_prefix)
            .await
    }

    async fn prepare_listen_first_device(
        self: Box<Self>,
        name_prefix: &str,
    ) -> Result<PreparedListenSession, InteractionError> {
        let session = self
            .backend
            .prepare_listen_first_matching_device(name_prefix)
            .await?;

        Ok(PreparedListenSession {
            session: ListenSession::Real(session),
        })
    }
}

#[derive(Debug)]
struct FakeHardwareClient {
    backend: FakeBackend,
}

impl FakeHardwareClient {
    fn new(config: FakeBackendConfig) -> Self {
        Self {
            backend: FakeBackend::new(config),
        }
    }
}

#[async_trait]
impl HardwareClient for FakeHardwareClient {
    async fn inspect_first_device(
        self: Box<Self>,
        name_prefix: &str,
    ) -> Result<InspectReport, InteractionError> {
        let Self { backend } = *self;
        backend.inspect_first_matching_device(name_prefix).await
    }

    async fn prepare_listen_first_device(
        self: Box<Self>,
        name_prefix: &str,
    ) -> Result<PreparedListenSession, InteractionError> {
        let Self { backend } = *self;
        let session = backend
            .prepare_listen_first_matching_device(name_prefix)
            .await?;

        Ok(PreparedListenSession {
            session: ListenSession::Fake(session),
        })
    }
}

/// A connected session ready to receive notifications.
#[derive(Debug)]
pub struct PreparedListenSession {
    session: ListenSession,
}

impl PreparedListenSession {
    /// Returns details for the connected device.
    #[must_use]
    pub fn device(&self) -> &FoundDevice {
        match &self.session {
            ListenSession::Real(real) => real.device(),
            ListenSession::Fake(fake) => fake.device(),
        }
    }

    /// Returns the initial `fa03` read payload, if available.
    #[must_use]
    pub fn initial_read(&self) -> Option<&[u8]> {
        match &self.session {
            ListenSession::Real(real) => real.initial_read(),
            ListenSession::Fake(fake) => fake.initial_read(),
        }
    }

    /// Runs the listen loop and emits each notification to the callback.
    ///
    /// # Errors
    ///
    /// Returns an error if receiving notifications fails, handling an interrupt
    /// fails, or backend teardown fails.
    pub async fn run<F>(
        self,
        max_notifications: Option<usize>,
        on_notification: F,
    ) -> Result<ListenSummary, InteractionError>
    where
        F: FnMut(usize, &[u8]),
    {
        match self.session {
            ListenSession::Real(real) => real.run(max_notifications, on_notification).await,
            ListenSession::Fake(fake) => Ok(fake.run(max_notifications, on_notification)),
        }
    }
}

#[derive(Debug)]
enum ListenSession {
    Real(PreparedRealListen),
    Fake(PreparedFakeListen),
}
