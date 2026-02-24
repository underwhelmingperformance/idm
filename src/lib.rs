mod app;
mod cli;
pub mod diy;
mod error;
mod handlers;
mod hw;
mod media;
mod notification;
mod protocol;
mod telemetry;
mod terminal;
mod utils;

// ── Public API ───────────────────────────────────────────────────────

pub use app::{
    SessionHandler, fake_hardware_client, real_hardware_client,
    real_hardware_client_with_model_resolution, run, run_with_clients,
    run_with_clients_and_log_level, run_with_log_level,
};
pub use cli::{
    Args, BrightnessArgs, ColourArgs, Command, ControlAction, ControlArgs, FakeArgs, ImageArgs,
    ListenArgs, LogLevel, OutputFormat, PowerArgs, PowerState, SyncTimeArgs, TextArgs,
};
pub use error::{FixtureError, InteractionError, ProtocolError};
pub use handlers::{
    Brightness, BrightnessError, BrightnessHandler, FrameCodecError, FullscreenColourHandler,
    GifUploadError, GifUploadHandler, GifUploadReceipt, GifUploadRequest, ImageUploadError,
    ImageUploadHandler, ImageUploadReceipt, ImageUploadRequest, MaterialSlot, MaterialTimeSign,
    MediaHeaderTail, PowerHandler, Rgb, ScreenLightTimeoutHandler, ScreenLightTimeoutProbe,
    ScreenLightTimeoutProbeOutcome, ScreenPower, TextOptions, TextUploadError, TextUploadHandler,
    TextUploadRequest, TimeSyncHandler, TimedMaterialSlot, UploadAckError, UploadReceipt,
};
pub use hw::{
    AckAction, AmbiguousShape, CharacteristicInfo, DeviceProfile, DeviceSession, EndpointPresence,
    FoundDevice, GattProfile, GifHeaderProfile, GifScenario, HardwareClient, ImageScenario,
    ImageUploadMode, InspectReport, LedInfoResponse, ListenFixture, ListenNotification,
    ListenScenario, ListenStopReason, ListenStreamBehaviour, ListenSummary, ModelProfile,
    ModelResolutionConfig, NotificationMessage, NotificationRunSummary, NotificationSubscription,
    PanelDimensions, PanelSize, ScanIdentity, ScanModelHandler, ScanScenario, ServiceInfo,
    SessionMetadata, TextPath, TextScenario, WriteMode,
};
pub use media::{
    GifAnimation, GifAnimationError, ImagePreparationError, ImagePreprocessor, PreparedImageUpload,
    PreparedStillImage, Rgb888Frame, Rgb888FrameError,
};
pub use notification::{
    NotificationDecodeError, NotifyEvent, ScheduleMasterSwitchStatus, ScheduleSetupStatus,
    TransferFamily,
};
pub use protocol::EndpointId;
pub use terminal::TerminalClient;

// ── Crate-internal re-exports ────────────────────────────────────────

pub(crate) use handlers::{
    DiyPrefixFields, FrameCodec, GifChunkFlag, GifHeaderFields, ImageHeaderFields, TextHeaderFields,
};
