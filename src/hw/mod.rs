mod btleplug_backend;
mod device_profile_resolver;
pub(crate) mod diagnostic_value;
pub(crate) mod diagnostics;
mod fake_backend;
mod hardware;
mod model;
mod model_overrides;
mod model_resolution_diagnostics;
mod profile;
mod scan_capabilities;
mod scan_model;
mod session;

pub(crate) use self::device_profile_resolver::{DeviceProfileResolver, DeviceRoutingProfile};
pub use self::device_profile_resolver::{LedInfoResponse, TextPath};
pub use self::fake_backend::{
    AckAction, GifScenario, ImageScenario, ListenFixture, ListenNotification, ListenScenario,
    ListenStreamBehaviour, ScanScenario, TextScenario,
};
pub(crate) use self::fake_backend::{
    FakeBackendConfig, HexPayload, NotificationPayloads, ScanFixture,
};
pub use self::hardware::{
    DeviceSession, HardwareClient, NotificationMessage, NotificationSubscription, WriteMode,
};
pub(crate) use self::hardware::{
    fake_hardware_client, real_hardware_client, real_hardware_client_with_model_resolution,
};
pub use self::model::{
    CharacteristicInfo, EndpointPresence, FoundDevice, InspectReport, ListenStopReason,
    ListenSummary, NotificationRunSummary, ServiceInfo, SessionMetadata,
};
pub use self::model_overrides::ModelResolutionConfig;
pub use self::profile::{
    DeviceProfile, GifHeaderProfile, ImageUploadMode, PanelDimensions, PanelSize,
};
pub use self::scan_model::{AmbiguousShape, ModelProfile, ScanIdentity, ScanModelHandler};
pub use self::session::GattProfile;
