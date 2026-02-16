use std::io::{self, IsTerminal};
use std::sync::OnceLock;

use indicatif::ProgressStyle;
use opentelemetry::global;
use opentelemetry::trace::TracerProvider as _;
use tracing::Metadata;
use tracing_indicatif::{IndicatifLayer, TickSettings};
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::filter;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::error::TelemetryError;

static TRACING_INITIALISED: OnceLock<Result<(), TelemetryError>> = OnceLock::new();

/// Initialises structured logging and OpenTelemetry tracing support.
pub(crate) fn initialise_tracing(
    service_name: &str,
    interactive_terminal: bool,
) -> Result<(), &'static TelemetryError> {
    TRACING_INITIALISED
        .get_or_init(|| initialise_tracing_once(service_name, interactive_terminal))
        .as_ref()
        .copied()
}

fn initialise_tracing_once(
    service_name: &str,
    interactive_terminal: bool,
) -> Result<(), TelemetryError> {
    let tracer_provider = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
    let tracer = tracer_provider.tracer(service_name.to_owned());
    global::set_tracer_provider(tracer_provider);

    let log_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    let is_interactive = interactive_terminal && io::stderr().is_terminal();

    if is_interactive {
        let indicatif_layer = IndicatifLayer::new()
            .with_progress_style(progress_style())
            .with_tick_settings(progress_tick_settings());
        let formatting_layer = fmt::layer()
            .pretty()
            .with_target(false)
            .with_writer(indicatif_layer.get_stderr_writer());
        let progress_layer = indicatif_layer.with_filter(filter::filter_fn(progress_span_filter));

        tracing_subscriber::registry()
            .with(formatting_layer.with_filter(log_filter.clone()))
            .with(progress_layer)
            .with(OpenTelemetryLayer::new(tracer.clone()))
            .try_init()?;
    } else {
        tracing_subscriber::registry()
            .with(
                fmt::layer()
                    .json()
                    .with_target(false)
                    .with_filter(log_filter),
            )
            .with(OpenTelemetryLayer::new(tracer))
            .try_init()?;
    }

    Ok(())
}

fn progress_style() -> ProgressStyle {
    ProgressStyle::with_template("{spinner:.cyan.bold} {msg}")
        .unwrap_or_else(|_error| ProgressStyle::default_spinner())
}

fn progress_tick_settings() -> TickSettings {
    TickSettings {
        default_tick_interval: Some(std::time::Duration::from_millis(90)),
        ..TickSettings::default()
    }
}

fn progress_span_filter(metadata: &Metadata<'_>) -> bool {
    metadata.is_span()
        && metadata.target().starts_with("idm::")
        && matches!(
            *metadata.level(),
            tracing::Level::INFO | tracing::Level::WARN | tracing::Level::ERROR
        )
}
