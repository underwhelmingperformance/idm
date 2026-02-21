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
/// Set bare `show_elapsed` to include elapsed time in the rendered progress
/// template.
///
/// Set `count_unit = ("singular", "plural")` to suffix counters with a unit
/// label (for example `1 command`, `2 commands`).
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
/// Rendering mode is inferred from usage:
/// - Determinate when `progress_set_length!` or `progress_inc_length!` is used.
/// - Indeterminate when no length API is used.
///
/// If `progress_inc!` is used without a length API, the span remains
/// indeterminate and displays a running command count.
///
/// `count_unit` requires `progress_inc!` so the displayed position can change.
///
/// ```no_run
/// use idm_macros::progress;
///
/// #[progress(
///     message = "Executing transfer",
///     finished = "done".to_string(),
///     show_elapsed,
///     count_unit = ("command", "commands"),
///     skip_all,
///     level = "info",
/// )]
/// fn transfer() -> Result<(), Box<dyn std::error::Error>> {
///     progress_inc!();
///     Ok(())
/// }
/// # let _ = transfer;
/// ```
///
/// ```no_run
/// use idm_macros::progress;
///
/// #[progress(
///     message = "Uploading payload",
///     finished = "uploaded".to_string(),
///     count_unit = ("chunk", "chunks"),
///     skip_all,
///     level = "debug",
/// )]
/// fn upload() -> Result<(), Box<dyn std::error::Error>> {
///     progress_set_length!(8usize);
///     progress_inc!();
///     Ok(())
/// }
/// # let _ = upload;
/// ```
#[proc_macro_attribute]
pub fn progress(attr: TokenStream, item: TokenStream) -> TokenStream {
    progress::expand(attr, item)
}
