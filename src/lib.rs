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

pub use app::{SessionHandler, fake_hardware_client, real_hardware_client, run_with_clients};
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
    CharacteristicInfo, DeviceProfile, DeviceSession, EndpointPresence, FoundDevice, GattProfile,
    GifHeaderProfile, HardwareClient, ImageUploadMode, InspectReport, ListenStopReason,
    ListenSummary, NotificationRunSummary, PanelSize, ServiceInfo, SessionMetadata, WriteMode,
};
pub use notification::{NotificationDecodeError, NotificationHandler, NotifyEvent};
pub use protocol::EndpointId;
pub use terminal::{SystemTerminalClient, TerminalClient};
