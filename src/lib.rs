mod app;
mod cli;
mod error;
mod hw;
mod protocol;
mod telemetry;
mod terminal;
mod utils;

pub use app::{run, run_with_terminal_client};
pub use cli::{Args, Command, FakeArgs, ListenArgs};
pub use error::{FixtureError, InteractionError};
pub use hw::{
    CharacteristicInfo, EndpointPresence, FoundDevice, HardwareClient, InspectReport,
    ListenStopReason, ListenSummary, PreparedListenSession, ServiceInfo,
};
pub use protocol::EndpointId;
pub use terminal::{SystemTerminalClient, TerminalClient};
