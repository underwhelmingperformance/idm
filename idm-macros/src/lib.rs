use proc_macro::TokenStream;

mod diagnostics_section;
mod has_diagnostics;
mod progress;

/// Derives `crate::hw::diagnostics::DiagnosticsSection` for a named struct.
///
/// Supported attributes:
/// - Container: `#[diagnostics(id = "...", section = "...")]`
/// - Field: `#[diagnostic(name = "...")]`, `#[diagnostic(skip)]`
#[proc_macro_derive(DiagnosticsSection, attributes(diagnostics, diagnostic))]
pub fn derive_diagnostics_section(input: TokenStream) -> TokenStream {
    diagnostics_section::expand(input)
}

/// Derives `crate::hw::diagnostics::HasDiagnostics` for a container struct.
///
/// Mark each diagnostics section field with `#[diagnostic]`.
#[proc_macro_derive(HasDiagnostics, attributes(diagnostic))]
pub fn derive_has_diagnostics(input: TokenStream) -> TokenStream {
    has_diagnostics::expand(input)
}

/// Wraps a function with an indicatif progress bar tied to its tracing span.
///
/// Place this above `#[instrument]` to automatically inject `progress = true`
/// into the span's fields and set both the in-progress and finished messages.
/// If no `#[instrument]` attribute is present, one is added automatically.
///
/// ```ignore
/// #[progress(
///     message = "Scanning for devices",
///     finished = format!("{} Connected", "✓".green()),
/// )]
/// #[instrument(skip(self), level = "info")]
/// async fn connect(&self) -> Result<()> { /* ... */ }
/// ```
#[proc_macro_attribute]
pub fn progress(attr: TokenStream, item: TokenStream) -> TokenStream {
    progress::expand(attr, item)
}
