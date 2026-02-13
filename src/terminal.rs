use std::io::{self, IsTerminal};

/// Provides terminal capability checks used by CLI rendering.
pub trait TerminalClient: Send + Sync {
    /// Returns whether standard output should be treated as a terminal.
    fn stdout_is_terminal(&self) -> bool;

    /// Returns whether standard error should be treated as a terminal.
    fn stderr_is_terminal(&self) -> bool;
}

/// Terminal capability provider backed by the host process stdio streams.
#[derive(Debug, Default)]
pub struct SystemTerminalClient;

impl TerminalClient for SystemTerminalClient {
    fn stdout_is_terminal(&self) -> bool {
        io::stdout().is_terminal()
    }

    fn stderr_is_terminal(&self) -> bool {
        io::stderr().is_terminal()
    }
}
