use std::collections::VecDeque;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bon::Builder;
use strum_macros::EnumString;
use tokio::time::sleep;
use tracing::instrument;

use super::DeviceProfile;
use super::hardware::{ConnectedBleSession, PayloadStream, WriteMode, missing_required_endpoints};
use super::model::{
    CharacteristicInfo, EndpointPresence, FoundDevice, InspectReport, ServiceInfo, SessionMetadata,
};
use super::model_overrides::{ModelResolutionConfig, is_supported_led_type};
use super::profile::{resolve_device_profile, resolve_device_routing_profile};
use super::scan_model::ScanModelHandler;
use super::session::{FA_SERVICE_UUID, FA_WRITE_UUID, negotiate_session_endpoints};
use crate::error::{FixtureError, InteractionError};
use crate::notification::{NotifyEvent, TransferFamily};
use crate::protocol::{self, EndpointId};

const DEFAULT_INITIAL_READ: [u8; 5] = [0x05, 0x00, 0x01, 0x00, 0x01];
const DEFAULT_WRITE_WITHOUT_RESPONSE_LIMIT: Option<usize> =
    Some(protocol::TRANSPORT_CHUNK_MTU_READY);
const NOTIFY_PREFIX_LEN: u8 = 0x05;
const NOTIFY_PREFIX_NS: u8 = 0x00;
const STATUS_NEXT_PACKAGE: u8 = 0x01;
const STATUS_FINISHED: u8 = 0x03;
const SCHEDULE_SETUP_ID: u8 = 0x05;
const SCHEDULE_MASTER_SWITCH_ID: u8 = 0x07;
const SCHEDULE_NS: u8 = 0x80;
const SCREEN_LIGHT_TIMEOUT_ID: u8 = 0x0F;
const GIF_COMMAND_ID: u8 = 0x01;
const IMAGE_COMMAND_ID: u8 = 0x02;
const TEXT_COMMAND_ID: u8 = 0x03;
const COMMAND_NS: u8 = 0x00;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct NotificationCode {
    id: u8,
    ns: u8,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct NotificationFrame {
    code: NotificationCode,
    status: u8,
}

impl NotificationFrame {
    fn into_payload(self) -> Vec<u8> {
        vec![
            NOTIFY_PREFIX_LEN,
            NOTIFY_PREFIX_NS,
            self.code.id,
            self.code.ns,
            self.status,
        ]
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum UploadCommand {
    Gif,
    Image,
    Text,
}

impl TryFrom<(u8, u8)> for UploadCommand {
    type Error = ();

    fn try_from(value: (u8, u8)) -> Result<Self, Self::Error> {
        match value {
            (GIF_COMMAND_ID, COMMAND_NS) => Ok(Self::Gif),
            (IMAGE_COMMAND_ID, COMMAND_NS) => Ok(Self::Image),
            (TEXT_COMMAND_ID, COMMAND_NS) => Ok(Self::Text),
            _ => Err(()),
        }
    }
}

impl From<UploadCommand> for TransferFamily {
    fn from(value: UploadCommand) -> Self {
        match value {
            UploadCommand::Gif => TransferFamily::Gif,
            UploadCommand::Image => TransferFamily::Image,
            UploadCommand::Text => TransferFamily::Text,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum HeaderChunkFlag {
    First,
    Continuation,
}

impl TryFrom<u8> for HeaderChunkFlag {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x00 => Ok(Self::First),
            0x02 => Ok(Self::Continuation),
            _ => Err(()),
        }
    }
}

/// Parsed fake scan fixture records.
#[derive(Debug, Clone, derive_more::Into)]
pub(crate) struct ScanFixture {
    devices: Vec<FoundDevice>,
}

impl FromStr for ScanFixture {
    type Err = FixtureError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let devices = parse_scan_fixture(value)?;
        Ok(Self { devices })
    }
}

/// Parsed fake hex payload.
#[derive(Debug, Clone, derive_more::Into)]
pub(crate) struct HexPayload {
    payload: Vec<u8>,
}

impl FromStr for HexPayload {
    type Err = FixtureError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let payload = parse_hex(value)?;
        Ok(Self { payload })
    }
}

/// Parsed fake notification payload fixtures.
#[derive(Debug, Clone, derive_more::Into)]
pub(crate) struct NotificationPayloads {
    payloads: Vec<Vec<u8>>,
}

/// Fake scan behaviour and fixture-backed discovery records.
#[derive(Debug, Clone, Builder)]
pub struct ScanScenario {
    #[builder(with = |value: &str| -> std::result::Result<_, crate::error::FixtureError> { value.parse() })]
    fixture: ScanFixture,
    #[builder(default)]
    discovery_delay: Duration,
}

impl ScanScenario {
    /// Parses a semicolon-delimited fake scan fixture into a scan scenario.
    ///
    /// ```
    /// let scan = idm::ScanScenario::from_fixture("hci0|AA:BB:CC|IDM-Clock|-43")?;
    /// let _ = scan;
    /// # Ok::<(), idm::FixtureError>(())
    /// ```
    pub fn from_fixture(raw_fixture: &str) -> Result<Self, FixtureError> {
        Ok(Self {
            fixture: raw_fixture.parse()?,
            discovery_delay: Duration::ZERO,
        })
    }
}

impl TryFrom<&str> for ScanScenario {
    type Error = FixtureError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::from_fixture(value)
    }
}

impl From<(ScanFixture, Duration)> for ScanScenario {
    fn from((fixture, discovery_delay): (ScanFixture, Duration)) -> Self {
        Self {
            fixture,
            discovery_delay,
        }
    }
}

/// One listen notification fixture item.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ListenNotification {
    /// Encoded from a typed notification event.
    Event(NotifyEvent),
    /// Raw notification payload bytes.
    Raw(Vec<u8>),
}

impl ListenNotification {
    fn payload(self) -> Vec<u8> {
        self.into()
    }
}

impl From<ListenNotification> for Vec<u8> {
    fn from(value: ListenNotification) -> Self {
        match value {
            ListenNotification::Event(event) => encode_notify_event(event),
            ListenNotification::Raw(payload) => payload,
        }
    }
}

/// Fake listen-stream behaviour.
#[derive(Debug, Clone, Builder, Default)]
pub struct ListenScenario {
    #[builder(default)]
    notifications: Vec<ListenNotification>,
    #[builder(default)]
    stream_behaviour: ListenStreamBehaviour,
    auto_advance_interval: Option<Duration>,
}

impl ListenScenario {
    /// Parses comma-delimited notification payload fixtures.
    ///
    /// ```
    /// let listen = idm::ListenScenario::from_payloads("0500010001,0500010003")?;
    /// let _ = listen;
    /// # Ok::<(), idm::FixtureError>(())
    /// ```
    pub fn from_payloads(raw_value: &str) -> Result<Self, FixtureError> {
        raw_value.parse()
    }
}

/// Behaviour of the fake listen stream once subscribed.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
pub enum ListenStreamBehaviour {
    /// Keep the notification stream open for dynamically emitted events.
    #[default]
    KeepOpen,
    /// Close the stream after draining initial queued notifications.
    CloseAfterInitialNotifications,
}

/// Named listen fixtures for fake-backend notification streams.
#[derive(Debug, Clone, Copy, Eq, PartialEq, EnumString)]
#[strum(serialize_all = "snake_case")]
pub enum ListenFixture {
    /// A GIF transfer that ACKs `next package`, then `finished`.
    GifTransferHappyPath,
    /// A text transfer that ACKs `next package`, then `finished`.
    TextTransferHappyPath,
}

impl ListenFixture {
    /// Constant-style alias for [`ListenFixture::GifTransferHappyPath`].
    pub const GIF_TRANSFER_HAPPY_PATH: Self = Self::GifTransferHappyPath;
    /// Constant-style alias for [`ListenFixture::TextTransferHappyPath`].
    pub const TEXT_TRANSFER_HAPPY_PATH: Self = Self::TextTransferHappyPath;

    fn into_scenario(self) -> ListenScenario {
        let family = match self {
            Self::GifTransferHappyPath => TransferFamily::Gif,
            Self::TextTransferHappyPath => TransferFamily::Text,
        };
        ListenScenario {
            notifications: vec![
                ListenNotification::Event(NotifyEvent::NextPackage(family)),
                ListenNotification::Event(NotifyEvent::Finished(family)),
            ],
            stream_behaviour: ListenStreamBehaviour::KeepOpen,
            auto_advance_interval: None,
        }
    }
}

impl From<ListenFixture> for ListenScenario {
    fn from(value: ListenFixture) -> Self {
        value.into_scenario()
    }
}

impl FromStr for ListenScenario {
    type Err = FixtureError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if let Ok(fixture) = value.parse::<ListenFixture>() {
            return Ok(fixture.into());
        }

        let payloads = parse_notifications(value)?;
        Ok(Self::from(payloads))
    }
}

impl TryFrom<&str> for ListenScenario {
    type Error = FixtureError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<Vec<Vec<u8>>> for ListenScenario {
    fn from(payloads: Vec<Vec<u8>>) -> Self {
        Self {
            notifications: payloads.into_iter().map(ListenNotification::Raw).collect(),
            stream_behaviour: ListenStreamBehaviour::KeepOpen,
            auto_advance_interval: None,
        }
    }
}

/// Response action emitted by fake upload acknowledgement logic.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AckAction {
    /// Emit a normal `next package` acknowledgement.
    NextPackage,
    /// Emit a `finished` acknowledgement.
    Finished,
    /// Emit a family-specific error status.
    Error(u8),
    /// Emit nothing (simulates dropped acknowledgement).
    NoAck,
}

impl AckAction {
    fn into_event(self, family: TransferFamily) -> Option<NotifyEvent> {
        match self {
            Self::NextPackage => Some(NotifyEvent::NextPackage(family)),
            Self::Finished => Some(NotifyEvent::Finished(family)),
            Self::Error(status) => Some(NotifyEvent::Error(family, status)),
            Self::NoAck => None,
        }
    }
}

/// Fake GIF acknowledgement behaviour.
#[derive(Debug, Clone, Builder, Default)]
pub struct GifScenario {
    first_chunk: Option<AckAction>,
    non_final_chunk: Option<AckAction>,
    last_chunk: Option<AckAction>,
}

impl GifScenario {
    fn action_for(&self, phase: ChunkPhase) -> AckAction {
        match phase {
            ChunkPhase::First => self.first_chunk,
            ChunkPhase::Single => self.first_chunk.or(self.last_chunk),
            ChunkPhase::NonFinal => self.non_final_chunk,
            ChunkPhase::Last => self.last_chunk,
        }
        .unwrap_or(default_ack_action(phase))
    }
}

/// Fake image acknowledgement behaviour.
#[derive(Debug, Clone, Builder, Default)]
pub struct ImageScenario {
    first_chunk: Option<AckAction>,
    non_final_chunk: Option<AckAction>,
    last_chunk: Option<AckAction>,
}

impl ImageScenario {
    fn action_for(&self, phase: ChunkPhase) -> AckAction {
        match phase {
            ChunkPhase::First => self.first_chunk,
            ChunkPhase::Single => self.first_chunk.or(self.last_chunk),
            ChunkPhase::NonFinal => self.non_final_chunk,
            ChunkPhase::Last => self.last_chunk,
        }
        .unwrap_or(default_ack_action(phase))
    }
}

/// Fake text acknowledgement behaviour.
#[derive(Debug, Clone, Builder, Default)]
pub struct TextScenario {
    first_chunk: Option<AckAction>,
    non_final_chunk: Option<AckAction>,
    last_chunk: Option<AckAction>,
}

impl TextScenario {
    fn action_for(&self, phase: ChunkPhase) -> AckAction {
        match phase {
            ChunkPhase::First => self.first_chunk,
            ChunkPhase::Single => self.first_chunk.or(self.last_chunk),
            ChunkPhase::NonFinal => self.non_final_chunk,
            ChunkPhase::Last => self.last_chunk,
        }
        .unwrap_or(default_ack_action(phase))
    }
}

fn default_ack_action(phase: ChunkPhase) -> AckAction {
    match phase {
        ChunkPhase::Single | ChunkPhase::Last => AckAction::Finished,
        ChunkPhase::First | ChunkPhase::NonFinal => AckAction::NextPackage,
    }
}

impl FromStr for NotificationPayloads {
    type Err = FixtureError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let payloads = parse_notifications(value)?;
        Ok(Self { payloads })
    }
}

impl From<NotificationPayloads> for ListenScenario {
    fn from(value: NotificationPayloads) -> Self {
        Self::from(Vec::<Vec<u8>>::from(value))
    }
}

/// Settings for constructing a fake hardware backend.
#[derive(Debug, Builder)]
pub(crate) struct FakeBackendConfig {
    scan: ScanScenario,
    initial_read: Option<HexPayload>,
    #[builder(default)]
    listen: ListenScenario,
    #[builder(default)]
    gif: GifScenario,
    #[builder(default)]
    image: ImageScenario,
    #[builder(default)]
    text: TextScenario,
    #[builder(default)]
    model_resolution: ModelResolutionConfig,
}

/// Fake backend used in tests and non-hardware environments.
#[derive(Debug)]
pub(crate) struct FakeBackend {
    devices: Vec<FoundDevice>,
    services: Vec<ServiceInfo>,
    initial_read: Option<Vec<u8>>,
    discovery_delay: Duration,
    listen: ListenScenario,
    gif: GifScenario,
    image: ImageScenario,
    text: TextScenario,
    write_without_response_limit: Option<usize>,
    model_resolution: ModelResolutionConfig,
}

impl FakeBackend {
    /// Creates a fake backend from explicit settings.
    pub(crate) fn new(config: FakeBackendConfig) -> Self {
        let ScanScenario {
            fixture,
            discovery_delay,
        } = config.scan;
        let initial_read = config
            .initial_read
            .map(Into::into)
            .or_else(|| Some(DEFAULT_INITIAL_READ.to_vec()));

        Self {
            devices: fixture.into(),
            services: default_services(),
            initial_read,
            discovery_delay,
            listen: config.listen,
            gif: config.gif,
            image: config.image,
            text: config.text,
            write_without_response_limit: DEFAULT_WRITE_WITHOUT_RESPONSE_LIMIT,
            model_resolution: config.model_resolution,
        }
    }

    /// Connects to the first matching fake peripheral and returns a session.
    #[instrument(skip(self), level = "debug", fields(prefix = name_prefix))]
    pub(crate) async fn connect_first_matching_device(
        self,
        name_prefix: &str,
    ) -> Result<FakeDeviceSession, InteractionError> {
        let Self {
            devices,
            services,
            initial_read,
            discovery_delay,
            listen,
            gif,
            image,
            text,
            write_without_response_limit,
            model_resolution,
        } = self;

        let device = first_matching_device(devices, discovery_delay, name_prefix).await?;
        let negotiated_endpoints = negotiate_session_endpoints(&services)?;
        let endpoint_presence = negotiated_endpoints.endpoint_presence();
        let missing = missing_required_endpoints(&endpoint_presence);
        if !missing.is_empty() {
            return Err(InteractionError::MissingRequiredEndpoints {
                missing: format_missing_endpoints(&missing),
            });
        }

        let selected_led_type = select_led_type_override(&device, &model_resolution)?;
        let led_info = initial_read
            .as_deref()
            .and_then(super::LedInfoResponse::parse);
        let device_routing_profile =
            resolve_device_routing_profile(&device, led_info, selected_led_type);
        ensure_ambiguous_shape_is_resolved(&device, device_routing_profile)?;

        let device_profile = resolve_device_profile(
            &device,
            &services,
            write_without_response_limit,
            device_routing_profile,
        );
        let session_metadata =
            SessionMetadata::new(true, write_without_response_limit, device_profile)
                .with_endpoint_resolution(
                    negotiated_endpoints.gatt_profile,
                    negotiated_endpoints.endpoint_uuids.clone(),
                );

        Ok(FakeDeviceSession {
            device,
            services,
            endpoint_presence,
            session_metadata,
            initial_read,
            notification_tx: Mutex::new(None),
            pending_notifications: Mutex::new(
                listen
                    .notifications
                    .into_iter()
                    .map(ListenNotification::payload)
                    .collect(),
            ),
            listen_stream_behaviour: listen.stream_behaviour,
            listen_auto_advance_interval: listen.auto_advance_interval,
            protocol_state: Mutex::new(FakeProtocolState::new(gif, image, text)),
        })
    }
}

/// Active fake session.
pub(crate) struct FakeDeviceSession {
    device: FoundDevice,
    services: Vec<ServiceInfo>,
    endpoint_presence: EndpointPresence,
    session_metadata: SessionMetadata,
    initial_read: Option<Vec<u8>>,
    notification_tx: Mutex<Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>>,
    pending_notifications: Mutex<VecDeque<Vec<u8>>>,
    listen_stream_behaviour: ListenStreamBehaviour,
    listen_auto_advance_interval: Option<Duration>,
    protocol_state: Mutex<FakeProtocolState>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum ChunkPhase {
    First,
    Single,
    NonFinal,
    Last,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct ParsedTransferHeader {
    family: TransferFamily,
    phase: ChunkPhase,
}

#[derive(Debug, Default)]
struct TransferProgress {
    payload_len: u32,
    sent_payload_len: u32,
}

impl TransferProgress {
    fn observe(
        &mut self,
        chunk_flag: HeaderChunkFlag,
        chunk_payload_len: u16,
        payload_len: u32,
    ) -> ChunkPhase {
        if matches!(chunk_flag, HeaderChunkFlag::First) {
            self.payload_len = payload_len;
            self.sent_payload_len = 0;
        }
        self.sent_payload_len = self
            .sent_payload_len
            .saturating_add(u32::from(chunk_payload_len));

        if matches!(chunk_flag, HeaderChunkFlag::First) {
            if self.sent_payload_len >= self.payload_len {
                ChunkPhase::Single
            } else {
                ChunkPhase::First
            }
        } else if self.sent_payload_len >= self.payload_len {
            ChunkPhase::Last
        } else {
            ChunkPhase::NonFinal
        }
    }
}

#[derive(Debug)]
struct FakeProtocolState {
    gif: GifScenario,
    image: ImageScenario,
    text: TextScenario,
    gif_progress: TransferProgress,
    image_progress: TransferProgress,
    text_progress: TransferProgress,
}

impl FakeProtocolState {
    fn new(gif: GifScenario, image: ImageScenario, text: TextScenario) -> Self {
        Self {
            gif,
            image,
            text,
            gif_progress: TransferProgress::default(),
            image_progress: TransferProgress::default(),
            text_progress: TransferProgress::default(),
        }
    }

    fn action_for_header(&mut self, header: ParsedTransferHeader) -> AckAction {
        match header.family {
            TransferFamily::Gif => self.gif.action_for(header.phase),
            TransferFamily::Image => self.image.action_for(header.phase),
            TransferFamily::Text => self.text.action_for(header.phase),
            _ => AckAction::NoAck,
        }
    }
}

#[async_trait]
impl ConnectedBleSession for FakeDeviceSession {
    fn device(&self) -> &FoundDevice {
        &self.device
    }

    fn inspect_report(&self) -> InspectReport {
        InspectReport::new(
            self.device.clone(),
            self.services.clone(),
            self.endpoint_presence.clone(),
            self.session_metadata.clone(),
        )
    }

    fn write_without_response_limit(&self) -> Option<usize> {
        self.session_metadata.write_without_response_limit()
    }

    fn device_profile(&self) -> DeviceProfile {
        self.session_metadata.device_profile()
    }

    #[instrument(skip(self), level = "trace", fields(?endpoint))]
    async fn read_endpoint(&self, endpoint: EndpointId) -> Result<Vec<u8>, InteractionError> {
        self.read_endpoint_optional(endpoint)
            .await?
            .ok_or(InteractionError::MissingEndpoint { endpoint })
    }

    #[instrument(skip(self), level = "trace", fields(?endpoint))]
    async fn read_endpoint_optional(
        &self,
        endpoint: EndpointId,
    ) -> Result<Option<Vec<u8>>, InteractionError> {
        if endpoint != EndpointId::ReadNotifyCharacteristic {
            return Err(InteractionError::MissingEndpoint { endpoint });
        }

        Ok(self.initial_read.clone())
    }

    #[instrument(skip(self, payload), level = "trace", fields(?endpoint, ?mode, payload_len = payload.len()))]
    async fn write_endpoint(
        &self,
        endpoint: EndpointId,
        payload: &[u8],
        mode: WriteMode,
    ) -> Result<(), InteractionError> {
        let _ = (payload, mode);
        if endpoint != EndpointId::WriteCharacteristic {
            return Err(InteractionError::MissingEndpoint { endpoint });
        }

        if let Some(header) = self.parse_transfer_header(payload) {
            let action = {
                let mut protocol_state =
                    self.protocol_state.lock().expect("protocol mutex poisoned");
                protocol_state.action_for_header(header)
            };
            if let Some(event) = action.into_event(header.family) {
                self.emit_notification(encode_notify_event(event));
            }
        }

        Ok(())
    }

    #[instrument(skip(self), level = "trace", fields(?endpoint))]
    async fn subscribe_endpoint(&self, endpoint: EndpointId) -> Result<(), InteractionError> {
        if endpoint != EndpointId::ReadNotifyCharacteristic {
            return Err(InteractionError::MissingEndpoint { endpoint });
        }

        Ok(())
    }

    #[instrument(skip(self), level = "trace", fields(?endpoint))]
    async fn unsubscribe_endpoint(&self, endpoint: EndpointId) -> Result<(), InteractionError> {
        if endpoint != EndpointId::ReadNotifyCharacteristic {
            return Err(InteractionError::MissingEndpoint { endpoint });
        }

        Ok(())
    }

    async fn notification_payloads(
        &self,
        endpoint: EndpointId,
    ) -> Result<PayloadStream, InteractionError> {
        if endpoint != EndpointId::ReadNotifyCharacteristic {
            return Err(InteractionError::MissingEndpoint { endpoint });
        }

        let (sender, rx) = tokio::sync::mpsc::unbounded_channel();
        if let Some(interval) = self
            .listen_auto_advance_interval
            .filter(|value| !value.is_zero())
        {
            let sender_for_clock = sender.clone();
            tokio::spawn(async move {
                while !sender_for_clock.is_closed() {
                    tokio::time::advance(interval).await;
                    tokio::task::yield_now().await;
                }
            });
        }
        match self.listen_stream_behaviour {
            ListenStreamBehaviour::KeepOpen => {
                {
                    let mut tx_guard = self
                        .notification_tx
                        .lock()
                        .expect("notification sender mutex poisoned");
                    *tx_guard = Some(sender.clone());
                }
                {
                    let mut pending = self
                        .pending_notifications
                        .lock()
                        .expect("pending notification mutex poisoned");
                    while let Some(payload) = pending.pop_front() {
                        let _ = sender.send(payload);
                    }
                }
            }
            ListenStreamBehaviour::CloseAfterInitialNotifications => {
                {
                    let mut tx_guard = self
                        .notification_tx
                        .lock()
                        .expect("notification sender mutex poisoned");
                    *tx_guard = None;
                }
                {
                    let mut pending = self
                        .pending_notifications
                        .lock()
                        .expect("pending notification mutex poisoned");
                    while let Some(payload) = pending.pop_front() {
                        let _ = sender.send(payload);
                    }
                }
            }
        }

        Ok(Box::pin(
            tokio_stream::wrappers::UnboundedReceiverStream::new(rx),
        ))
    }

    #[instrument(skip(self), level = "debug")]
    async fn close(self: Arc<Self>) -> Result<(), InteractionError> {
        let _ = self;
        Ok(())
    }
}

impl FakeDeviceSession {
    fn emit_notification(&self, payload: Vec<u8>) {
        if let Some(sender) = self
            .notification_tx
            .lock()
            .expect("notification sender mutex poisoned")
            .as_ref()
            .cloned()
        {
            let _ = sender.send(payload);
            return;
        }

        self.pending_notifications
            .lock()
            .expect("pending notification mutex poisoned")
            .push_back(payload);
    }

    fn parse_transfer_header(&self, payload: &[u8]) -> Option<ParsedTransferHeader> {
        if payload.len() < 16 {
            return None;
        }

        let declared_len = u16::from_le_bytes([payload[0], payload[1]]) as usize;
        if declared_len < 16 {
            return None;
        }

        let command = UploadCommand::try_from((payload[2], payload[3])).ok()?;
        let family = TransferFamily::from(command);

        let chunk_flag = HeaderChunkFlag::try_from(payload[4]).ok()?;
        let chunk_payload_len = u16::try_from(declared_len - 16).ok()?;
        let payload_len = u32::from_le_bytes([payload[5], payload[6], payload[7], payload[8]]);

        let phase = match family {
            TransferFamily::Gif => self
                .protocol_state
                .lock()
                .expect("protocol mutex poisoned")
                .gif_progress
                .observe(chunk_flag, chunk_payload_len, payload_len),
            TransferFamily::Image => self
                .protocol_state
                .lock()
                .expect("protocol mutex poisoned")
                .image_progress
                .observe(chunk_flag, chunk_payload_len, payload_len),
            TransferFamily::Text => self
                .protocol_state
                .lock()
                .expect("protocol mutex poisoned")
                .text_progress
                .observe(chunk_flag, chunk_payload_len, payload_len),
            _ => return None,
        };

        Some(ParsedTransferHeader { family, phase })
    }
}

fn select_led_type_override(
    device: &FoundDevice,
    model_resolution: &ModelResolutionConfig,
) -> Result<Option<u8>, InteractionError> {
    let Some(identity) = device.scan_identity() else {
        return Ok(None);
    };

    if let Some(override_led_type) = model_resolution.led_type_override() {
        if !is_supported_led_type(override_led_type) {
            return Err(InteractionError::InvalidLedTypeOverride {
                value: override_led_type,
            });
        }
        return Ok(Some(override_led_type));
    }

    if super::DeviceProfileResolver::requires_led_type_selection(identity) {
        return Ok(None);
    }

    Ok(None)
}

fn ensure_ambiguous_shape_is_resolved(
    device: &FoundDevice,
    routing_profile: Option<super::DeviceRoutingProfile>,
) -> Result<(), InteractionError> {
    let Some(identity) = device.scan_identity() else {
        return Ok(());
    };

    if !super::DeviceProfileResolver::requires_led_type_selection(identity) {
        return Ok(());
    }
    if routing_profile
        .and_then(|profile| profile.led_type)
        .is_some()
    {
        return Ok(());
    }

    Err(InteractionError::AmbiguousShapeSelectionRequired {
        device_id: device.device_id_display().to_string(),
        shape: identity.shape,
    })
}

fn parse_scan_fixture(raw_fixture: &str) -> Result<Vec<FoundDevice>, FixtureError> {
    if raw_fixture.trim().is_empty() {
        return Err(FixtureError::EmptyFixture);
    }

    raw_fixture
        .split(';')
        .map(parse_scan_record)
        .collect::<Result<Vec<_>, _>>()
}

#[instrument(skip(devices), level = "trace", fields(prefix = name_prefix))]
async fn first_matching_device(
    devices: Vec<FoundDevice>,
    discovery_delay: Duration,
    name_prefix: &str,
) -> Result<FoundDevice, InteractionError> {
    if !discovery_delay.is_zero() {
        sleep(discovery_delay).await;
    }

    devices
        .into_iter()
        .find(|device| device.local_name_starts_with(name_prefix))
        .ok_or_else(|| InteractionError::NoMatchingFixtureDevice {
            prefix: name_prefix.to_string(),
        })
}

fn parse_scan_record(raw_record: &str) -> Result<FoundDevice, FixtureError> {
    let fields: Vec<&str> = raw_record.split('|').map(str::trim).collect();
    if fields.len() != 4 && fields.len() != 5 {
        return Err(FixtureError::InvalidRecordFieldCount);
    }
    if fields[0].is_empty() || fields[1].is_empty() || fields[2].is_empty() || fields[3].is_empty()
    {
        return Err(FixtureError::EmptyRecordField);
    }

    let local_name = if fields[2] == "-" {
        None
    } else {
        Some(fields[2].to_string())
    };
    let rssi = if fields[3] == "-" {
        None
    } else {
        Some(fields[3].parse::<i16>()?)
    };

    let device = FoundDevice::new(
        fields[0].to_string(),
        fields[1].to_string(),
        local_name,
        rssi,
    );

    let scan_model = match fields.get(4).copied().filter(|value| *value != "-") {
        Some(value) => {
            let scan_payload = parse_hex(value)?;
            let scan_identity = ScanModelHandler::parse_identity(&scan_payload)
                .ok_or(FixtureError::InvalidScanModelPayload)?;
            let model_profile = ScanModelHandler::resolve_model(&scan_identity);
            Some((scan_identity, model_profile))
        }
        None => None,
    };

    Ok(match scan_model {
        Some((scan_identity, model_profile)) => {
            device.with_scan_model(scan_identity, model_profile)
        }
        None => device,
    })
}

fn parse_notifications(raw_value: &str) -> Result<Vec<Vec<u8>>, FixtureError> {
    if raw_value.trim().is_empty() {
        return Ok(Vec::new());
    }
    raw_value.split(',').map(parse_hex).collect()
}

fn encode_notify_event(event: NotifyEvent) -> Vec<u8> {
    let frame = match event {
        NotifyEvent::NextPackage(TransferFamily::Text) => Some(NotificationFrame {
            code: NotificationCode {
                id: TEXT_COMMAND_ID,
                ns: COMMAND_NS,
            },
            status: STATUS_NEXT_PACKAGE,
        }),
        NotifyEvent::Finished(TransferFamily::Text) => Some(NotificationFrame {
            code: NotificationCode {
                id: TEXT_COMMAND_ID,
                ns: COMMAND_NS,
            },
            status: STATUS_FINISHED,
        }),
        NotifyEvent::Error(TransferFamily::Text, status) => Some(NotificationFrame {
            code: NotificationCode {
                id: TEXT_COMMAND_ID,
                ns: COMMAND_NS,
            },
            status,
        }),
        NotifyEvent::NextPackage(TransferFamily::Gif) => Some(NotificationFrame {
            code: NotificationCode {
                id: GIF_COMMAND_ID,
                ns: COMMAND_NS,
            },
            status: STATUS_NEXT_PACKAGE,
        }),
        NotifyEvent::Finished(TransferFamily::Gif) => Some(NotificationFrame {
            code: NotificationCode {
                id: GIF_COMMAND_ID,
                ns: COMMAND_NS,
            },
            status: STATUS_FINISHED,
        }),
        NotifyEvent::Error(TransferFamily::Gif, status) => Some(NotificationFrame {
            code: NotificationCode {
                id: GIF_COMMAND_ID,
                ns: COMMAND_NS,
            },
            status,
        }),
        NotifyEvent::NextPackage(TransferFamily::Image) => Some(NotificationFrame {
            code: NotificationCode {
                id: IMAGE_COMMAND_ID,
                ns: COMMAND_NS,
            },
            status: STATUS_NEXT_PACKAGE,
        }),
        NotifyEvent::Finished(TransferFamily::Image) => Some(NotificationFrame {
            code: NotificationCode {
                id: IMAGE_COMMAND_ID,
                ns: COMMAND_NS,
            },
            status: STATUS_FINISHED,
        }),
        NotifyEvent::Error(TransferFamily::Image, status) => Some(NotificationFrame {
            code: NotificationCode {
                id: IMAGE_COMMAND_ID,
                ns: COMMAND_NS,
            },
            status,
        }),
        NotifyEvent::NextPackage(TransferFamily::Diy) => Some(NotificationFrame {
            code: NotificationCode { id: 0x00, ns: 0x00 },
            status: 0x02,
        }),
        NotifyEvent::Finished(TransferFamily::Diy) => Some(NotificationFrame {
            code: NotificationCode { id: 0x00, ns: 0x00 },
            status: 0x00,
        }),
        NotifyEvent::Error(TransferFamily::Diy, status) => Some(NotificationFrame {
            code: NotificationCode { id: 0x00, ns: 0x00 },
            status,
        }),
        NotifyEvent::NextPackage(TransferFamily::Timer) => Some(NotificationFrame {
            code: NotificationCode { id: 0x00, ns: 0x80 },
            status: STATUS_NEXT_PACKAGE,
        }),
        NotifyEvent::Finished(TransferFamily::Timer) => Some(NotificationFrame {
            code: NotificationCode { id: 0x00, ns: 0x80 },
            status: STATUS_FINISHED,
        }),
        NotifyEvent::Error(TransferFamily::Timer, status) => Some(NotificationFrame {
            code: NotificationCode { id: 0x00, ns: 0x80 },
            status,
        }),
        NotifyEvent::NextPackage(TransferFamily::Ota) => Some(NotificationFrame {
            code: NotificationCode { id: 0x01, ns: 0xC0 },
            status: STATUS_NEXT_PACKAGE,
        }),
        NotifyEvent::Finished(TransferFamily::Ota) => Some(NotificationFrame {
            code: NotificationCode { id: 0x01, ns: 0xC0 },
            status: STATUS_FINISHED,
        }),
        NotifyEvent::Error(TransferFamily::Ota, status) => Some(NotificationFrame {
            code: NotificationCode { id: 0x01, ns: 0xC0 },
            status,
        }),
        NotifyEvent::ScheduleSetup(status) => {
            let value = match status {
                crate::ScheduleSetupStatus::Success => 0x01,
                crate::ScheduleSetupStatus::Continue => 0x03,
                crate::ScheduleSetupStatus::Failed(other) => other,
            };
            return NotificationFrame {
                code: NotificationCode {
                    id: SCHEDULE_SETUP_ID,
                    ns: SCHEDULE_NS,
                },
                status: value,
            }
            .into_payload();
        }
        NotifyEvent::ScheduleMasterSwitch(status) => {
            let value = match status {
                crate::ScheduleMasterSwitchStatus::Success => 0x01,
                crate::ScheduleMasterSwitchStatus::Failed(other) => other,
            };
            return NotificationFrame {
                code: NotificationCode {
                    id: SCHEDULE_MASTER_SWITCH_ID,
                    ns: SCHEDULE_NS,
                },
                status: value,
            }
            .into_payload();
        }
        NotifyEvent::LedInfo(response) => {
            return vec![
                0x09,
                0x00,
                0x01,
                0x80,
                response.mcu_major_version,
                response.mcu_minor_version,
                response.status,
                response.screen_type,
                u8::from(response.password_enabled),
            ];
        }
        NotifyEvent::ScreenLightTimeout(value) => {
            return NotificationFrame {
                code: NotificationCode {
                    id: SCREEN_LIGHT_TIMEOUT_ID,
                    ns: SCHEDULE_NS,
                },
                status: value,
            }
            .into_payload();
        }
        NotifyEvent::Unknown(payload) => return payload,
    };

    frame
        .expect("all transfer notifications map to frames")
        .into_payload()
}

fn parse_hex(raw_value: &str) -> Result<Vec<u8>, FixtureError> {
    let cleaned: String = raw_value.chars().filter(|c| !c.is_whitespace()).collect();
    if !cleaned.len().is_multiple_of(2) {
        return Err(FixtureError::InvalidHexLength);
    }
    let mut payload = Vec::with_capacity(cleaned.len() / 2);
    let bytes = cleaned.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        let value = std::str::from_utf8(&bytes[index..index + 2]).map_err(|_| {
            FixtureError::InvalidHexByte {
                value: String::from_utf8_lossy(&bytes[index..index + 2]).to_string(),
            }
        })?;
        let parsed = u8::from_str_radix(value, 16).map_err(|_| FixtureError::InvalidHexByte {
            value: value.to_string(),
        })?;
        payload.push(parsed);
        index += 2;
    }
    Ok(payload)
}

fn default_services() -> Vec<ServiceInfo> {
    vec![ServiceInfo::new(
        FA_SERVICE_UUID.to_string(),
        true,
        vec![
            CharacteristicInfo::new(FA_WRITE_UUID.to_string(), vec!["write".to_string()]),
            CharacteristicInfo::new(
                "0000fa03-0000-1000-8000-00805f9b34fb".to_string(),
                vec!["read".to_string(), "notify".to_string()],
            ),
        ],
    )]
}

fn format_missing_endpoints(endpoints: &[EndpointId]) -> String {
    endpoints
        .iter()
        .map(|endpoint| {
            let metadata = crate::protocol::endpoint_metadata(*endpoint);
            format!("{} ({})", metadata.name(), metadata.uuid())
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case("hci0|AA:BB|IDM-Cube|-43", 1)]
    #[case(
        "hci0|AA:BB|IDM-Cube|-43|0FFF5452007004010200010520002000;hci1|CC:DD|Speaker|-55",
        2
    )]
    fn parse_scan_fixture_parses_records(#[case] fixture: &str, #[case] expected_count: usize) {
        let devices = parse_scan_fixture(fixture).expect("fixture should parse");
        assert_eq!(expected_count, devices.len());
    }

    #[test]
    fn parse_scan_fixture_rejects_invalid_field_count() {
        let result = parse_scan_fixture("hci0|AA:BB|IDM-Cube");
        assert_matches!(result, Err(FixtureError::InvalidRecordFieldCount));
    }

    #[test]
    fn parse_hex_rejects_odd_length() {
        let result = parse_hex("A");
        assert_matches!(result, Err(FixtureError::InvalidHexLength));
    }

    #[test]
    fn parse_scan_fixture_rejects_invalid_scan_model_payload() {
        let result = parse_scan_fixture("hci0|AA:BB|IDM-Cube|-43|DEADBEEF");
        assert_matches!(result, Err(FixtureError::InvalidScanModelPayload));
    }
}
