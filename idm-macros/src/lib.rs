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
/// `message` and `finished` configure progress-bar text. Any additional
/// arguments are forwarded to `#[instrument(...)]`, and `progress = true` is
/// injected into instrument fields automatically. If no `#[instrument]`
/// attribute is present, one is added automatically.
///
/// `finished` is evaluated when the function body completes. During evaluation,
/// `result` is available as a reference to the function return value.
///
/// Inside the attributed function body, this macro also injects helper
/// macros for chunked-transfer progress:
/// - `progress_set_length!(<usize>)`
/// - `progress_inc_length!(<usize>)`
/// - `progress_inc!()` or `progress_inc!(<usize>)`
/// - `progress_trace!(<completed>, <total>)`
///
/// Progress-bar length is initialised to `0` when the function starts.
/// The macro applies a per-span bar template only when it detects
/// `progress_set_length!`, `progress_inc_length!`, or `progress_inc!` in the
/// body. Otherwise, span rendering falls back to the global spinner-style
/// template.
///
/// ```ignore
/// #[progress(
///     message = "Scanning for devices",
///     finished = format!("{} Connected", "✓".green()),
///     skip(self),
///     level = "info",
/// )]
/// async fn connect(&self) -> Result<()> { /* ... */ }
/// ```
#[proc_macro_attribute]
pub fn progress(attr: TokenStream, item: TokenStream) -> TokenStream {
    progress::expand(attr, item)
}
