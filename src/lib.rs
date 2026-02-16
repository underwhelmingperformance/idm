mod app;
mod cli;
mod error;
mod handlers;
mod hw;
mod notification;
mod protocol;
mod telemetry;
mod terminal;
mod utils;

pub use app::{
    SessionHandler, fake_hardware_client, real_hardware_client,
    real_hardware_client_with_model_resolution, run, run_with_clients,
};
pub use cli::{
    Args, BrightnessArgs, ColourArgs, Command, ControlAction, ControlArgs, FakeArgs, ListenArgs,
    PowerArgs, PowerState, SyncTimeArgs, TextArgs,
};
pub use error::{FixtureError, InteractionError, ProtocolError};
pub use handlers::{
    Brightness, BrightnessError, BrightnessHandler, FrameCodec, FrameCodecError,
    FullscreenColourHandler, GifChunkFlag, GifHeaderFields, PowerHandler, Rgb, ScreenPower,
    ShortFrame, TextHeaderFields, TextOptions, TextUploadError, TextUploadHandler,
    TextUploadRequest, TimeSyncHandler, UploadPacing, UploadReceipt,
};
pub use hw::{
    AmbiguousShape, CharacteristicInfo, DeviceProfile, DeviceProfileResolver, DeviceRoutingProfile,
    DeviceSession, EndpointPresence, FoundDevice, GattProfile, GifHeaderProfile, HardwareClient,
    ImageUploadMode, InspectReport, LedInfoResponse, ListenStopReason, ListenSummary, ModelProfile,
    ModelResolutionConfig, NotificationRunSummary, PanelSize, ScanIdentity, ScanModelHandler,
    ServiceInfo, SessionMetadata, TextPath, WriteMode,
};
pub use notification::{NotificationDecodeError, NotificationHandler, NotifyEvent};
pub use protocol::EndpointId;
pub use terminal::TerminalClient;
