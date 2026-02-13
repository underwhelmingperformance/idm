use std::future::Future;
use std::time::Duration;

use indicatif::{ProgressBar, ProgressStyle};

/// Async progress spinner for long-running operations.
#[derive(Debug)]
pub(crate) struct Spinner {
    enabled: bool,
}

impl Spinner {
    /// Creates a spinner with explicit enable/disable control.
    pub(crate) fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    /// Executes an operation while rendering an indefinite spinner when enabled.
    pub(crate) async fn with_spinner<F, Fut, T>(&self, message: &str, operation: F) -> T
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        if !self.enabled {
            return operation().await;
        }

        let spinner = new_spinner(message);
        let result = operation().await;
        spinner.finish_and_clear();
        result
    }
}

fn new_spinner(message: &str) -> ProgressBar {
    let spinner = ProgressBar::new_spinner();
    spinner.set_style(spinner_style());
    spinner.set_message(message.to_string());
    spinner.enable_steady_tick(Duration::from_millis(90));
    spinner
}

fn spinner_style() -> ProgressStyle {
    let base_style = ProgressStyle::default_spinner();
    let templated =
        ProgressStyle::with_template("{spinner:.cyan.bold} {msg}").unwrap_or(base_style);
    templated.tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::disabled(false)]
    #[case::enabled(true)]
    #[tokio::test]
    async fn with_spinner_returns_operation_result(#[case] enabled: bool) {
        let spinner = Spinner::new(enabled);
        let result = spinner.with_spinner("working...", || async { 42 }).await;
        assert_eq!(42, result);
    }
}
