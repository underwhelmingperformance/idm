use std::io::{self, IsTerminal};
use std::sync::OnceLock;

use opentelemetry::global;
use opentelemetry::trace::TracerProvider as _;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{Layer, fmt};

use crate::error::TelemetryError;

static TRACING_INITIALISED: OnceLock<Result<(), TelemetryError>> = OnceLock::new();

/// Initialises structured logging and OpenTelemetry tracing support.
pub(crate) fn initialise_tracing(service_name: &str) -> Result<(), &'static TelemetryError> {
    TRACING_INITIALISED
        .get_or_init(|| initialise_tracing_once(service_name))
        .as_ref()
        .copied()
}

fn initialise_tracing_once(service_name: &str) -> Result<(), TelemetryError> {
    let tracer_provider = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
    let tracer = tracer_provider.tracer(service_name.to_owned());
    global::set_tracer_provider(tracer_provider);

    let otel_layer = OpenTelemetryLayer::new(tracer);
    let formatting_layer = terminal_aware_formatting_layer();
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));

    tracing_subscriber::registry()
        .with(env_filter)
        .with(formatting_layer)
        .with(otel_layer)
        .try_init()?;
    Ok(())
}

fn terminal_aware_formatting_layer<S>() -> Box<dyn Layer<S> + Send + Sync>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    if io::stdout().is_terminal() {
        fmt::layer().pretty().with_target(false).boxed()
    } else {
        fmt::layer().json().with_target(false).boxed()
    }
}
