use proc_macro::TokenStream;

mod diagnostics_section;
mod has_diagnostics;

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
