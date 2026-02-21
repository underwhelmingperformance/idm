use proc_macro::TokenStream;
use proc_macro2::TokenTree;
use quote::quote;
use syn::ItemFn;
use syn::spanned::Spanned;

/// Progress rendering mode inferred from helper-macro usage in the body.
#[derive(Copy, Clone, Eq, PartialEq)]
enum ProgressMode {
    /// A fixed-length bar (`pos/len`) is available.
    Determinate,
    /// No length is known, but a running count (`pos`) is available.
    IndeterminateCount,
    /// Spinner-only mode without counter fields.
    SpinnerOnly,
}

/// Whether elapsed time should be shown in the rendered template.
#[derive(Copy, Clone, Eq, PartialEq)]
enum ElapsedDisplay {
    /// Do not render elapsed time.
    Hidden,
    /// Render elapsed time.
    Shown,
}

/// Whether count-unit text should be shown in the rendered template.
#[derive(Copy, Clone, Eq, PartialEq)]
enum CountUnitDisplay {
    /// Do not render count units.
    Hidden,
    /// Render singular/plural count units.
    Shown,
}

/// Key used to select an output template variant.
#[derive(Copy, Clone, Eq, PartialEq)]
struct ProgressTemplateKey {
    /// Base progress mode.
    mode: ProgressMode,
    /// Elapsed-time display mode.
    elapsed: ElapsedDisplay,
    /// Count-unit display mode.
    count_unit: CountUnitDisplay,
}

/// Errors when resolving a template from a template key.
#[derive(Debug, Copy, Clone, Eq, PartialEq, thiserror::Error)]
enum ProgressTemplateLookupError {
    #[error("no progress template for this configuration")]
    NoTemplate,
    #[error("count units require a counter progress mode")]
    CountUnitWithoutCounter,
}

/// Concrete template variants that can be rendered by indicatif.
#[derive(Copy, Clone, Eq, PartialEq)]
enum ProgressTemplate {
    Determinate,
    DeterminateWithCountUnit,
    DeterminateWithElapsed,
    DeterminateWithCountUnitAndElapsed,
    IndeterminateCount,
    IndeterminateCountWithCountUnit,
    IndeterminateCountWithElapsed,
    IndeterminateCountWithCountUnitAndElapsed,
    SpinnerWithElapsed,
}

impl AsRef<str> for ProgressTemplate {
    fn as_ref(&self) -> &str {
        match self {
            Self::Determinate => "{spinner:.cyan.bold} {msg} [{wide_bar:.cyan/blue}] {pos}/{len}",
            Self::DeterminateWithCountUnit => {
                "{spinner:.cyan.bold} {msg} [{wide_bar:.cyan/blue}] {pos}/{len} {count_unit}"
            }
            Self::DeterminateWithElapsed => {
                "{spinner:.cyan.bold} {msg} [{wide_bar:.cyan/blue}] {pos}/{len} ({elapsed_precise})"
            }
            Self::DeterminateWithCountUnitAndElapsed => {
                "{spinner:.cyan.bold} {msg} [{wide_bar:.cyan/blue}] {pos}/{len} {count_unit} ({elapsed_precise})"
            }
            Self::IndeterminateCount => "{spinner:.cyan.bold} {wide_msg} {pos}",
            Self::IndeterminateCountWithCountUnit => {
                "{spinner:.cyan.bold} {wide_msg} {pos} {count_unit}"
            }
            Self::IndeterminateCountWithElapsed => {
                "{spinner:.cyan.bold} {wide_msg} {pos} ({elapsed_precise})"
            }
            Self::IndeterminateCountWithCountUnitAndElapsed => {
                "{spinner:.cyan.bold} {wide_msg} {pos} {count_unit} ({elapsed_precise})"
            }
            Self::SpinnerWithElapsed => "{spinner:.cyan.bold} {wide_msg} ({elapsed_precise})",
        }
    }
}

impl TryFrom<ProgressTemplateKey> for ProgressTemplate {
    type Error = ProgressTemplateLookupError;

    fn try_from(key: ProgressTemplateKey) -> Result<Self, Self::Error> {
        use CountUnitDisplay::{Hidden as UnitHidden, Shown as UnitShown};
        use ElapsedDisplay::{Hidden as ElapsedHidden, Shown as ElapsedShown};
        use ProgressMode::{Determinate, IndeterminateCount, SpinnerOnly};

        match (key.mode, key.elapsed, key.count_unit) {
            (Determinate, ElapsedHidden, UnitHidden) => Ok(Self::Determinate),
            (Determinate, ElapsedHidden, UnitShown) => Ok(Self::DeterminateWithCountUnit),
            (Determinate, ElapsedShown, UnitHidden) => Ok(Self::DeterminateWithElapsed),
            (Determinate, ElapsedShown, UnitShown) => Ok(Self::DeterminateWithCountUnitAndElapsed),
            (IndeterminateCount, ElapsedHidden, UnitHidden) => Ok(Self::IndeterminateCount),
            (IndeterminateCount, ElapsedHidden, UnitShown) => {
                Ok(Self::IndeterminateCountWithCountUnit)
            }
            (IndeterminateCount, ElapsedShown, UnitHidden) => {
                Ok(Self::IndeterminateCountWithElapsed)
            }
            (IndeterminateCount, ElapsedShown, UnitShown) => {
                Ok(Self::IndeterminateCountWithCountUnitAndElapsed)
            }
            (SpinnerOnly, ElapsedShown, UnitHidden) => Ok(Self::SpinnerWithElapsed),
            (SpinnerOnly, ElapsedHidden, UnitHidden) => {
                Err(ProgressTemplateLookupError::NoTemplate)
            }
            (SpinnerOnly, _, UnitShown) => {
                Err(ProgressTemplateLookupError::CountUnitWithoutCounter)
            }
        }
    }
}

/// Singular/plural unit labels for counter output (for example
/// `"command"`/`"commands"`).
struct CountUnit {
    /// Singular form used when position equals `1`.
    singular: syn::LitStr,
    /// Plural form used for all other positions.
    plural: syn::LitStr,
    /// Source span used for compile-time error reporting.
    span: proc_macro2::Span,
}

/// Parsed `#[progress(...)]` attribute options.
struct ProgressAttrs {
    /// Initial progress message.
    message: syn::Expr,
    /// Completion message expression evaluated against `result`.
    finished: syn::Expr,
    /// Elapsed-time display setting.
    elapsed_display: ElapsedDisplay,
    /// Count-unit display setting.
    count_unit_display: CountUnitDisplay,
    /// Optional singular/plural count units.
    count_unit: Option<CountUnit>,
    /// Forwarded `#[instrument(...)]` arguments.
    instrument_args: proc_macro2::TokenStream,
}

/// Static analysis results derived from the attributed function body.
struct ProgressUsage {
    /// Whether `progress_set_length!` is used.
    uses_progress_set_length: bool,
    /// Whether `progress_inc_length!` is used.
    uses_progress_inc_length: bool,
    /// Whether `progress_inc!` is used.
    uses_progress_inc: bool,
    /// Whether `progress_trace!` is used.
    uses_progress_trace: bool,
    /// Inferred base progress mode.
    mode: ProgressMode,
    /// Selected indicatif style template.
    style_template: Option<ProgressTemplate>,
}

/// Token groups for helper macros injected into the wrapped function body.
struct ProgressHelperMacroTokens {
    /// `progress_set_length!` macro definition (or empty).
    set_length: proc_macro2::TokenStream,
    /// `progress_inc_length!` macro definition (or empty).
    inc_length: proc_macro2::TokenStream,
    /// `progress_inc!` macro definition (or empty).
    inc: proc_macro2::TokenStream,
    /// `progress_trace!` macro definition (or empty).
    trace: proc_macro2::TokenStream,
}

impl syn::parse::Parse for ProgressAttrs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut message = None;
        let mut finished = None;
        let mut elapsed_display = ElapsedDisplay::Hidden;
        let mut show_elapsed_set = false;
        let mut count_unit = None;
        let mut instrument_args = Vec::new();

        let metas =
            syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated(input)?;
        for meta in metas {
            match &meta {
                syn::Meta::Path(path) if path.is_ident("show_elapsed") => {
                    if show_elapsed_set {
                        return Err(syn::Error::new(
                            path.span(),
                            "`show_elapsed` provided more than once",
                        ));
                    }
                    show_elapsed_set = true;
                    elapsed_display = ElapsedDisplay::Shown;
                    continue;
                }
                syn::Meta::NameValue(name_value) if name_value.path.is_ident("message") => {
                    if message.is_some() {
                        return Err(syn::Error::new(
                            name_value.path.span(),
                            "`message` provided more than once",
                        ));
                    }
                    message = Some(name_value.value.clone());
                    continue;
                }
                syn::Meta::NameValue(name_value) if name_value.path.is_ident("finished") => {
                    if finished.is_some() {
                        return Err(syn::Error::new(
                            name_value.path.span(),
                            "`finished` provided more than once",
                        ));
                    }
                    finished = Some(name_value.value.clone());
                    continue;
                }
                syn::Meta::NameValue(name_value) if name_value.path.is_ident("count_unit") => {
                    if count_unit.is_some() {
                        return Err(syn::Error::new(
                            name_value.path.span(),
                            "`count_unit` provided more than once",
                        ));
                    }
                    count_unit = Some(parse_count_unit_name_value(name_value)?);
                    continue;
                }
                syn::Meta::NameValue(name_value) if name_value.path.is_ident("show_elapsed") => {
                    return Err(syn::Error::new(
                        name_value.path.span(),
                        "`show_elapsed` is a bare attribute",
                    ));
                }
                syn::Meta::Path(path)
                    if path.is_ident("message")
                        || path.is_ident("finished")
                        || path.is_ident("count_unit") =>
                {
                    return Err(syn::Error::new(
                        path.span(),
                        "`message`, `finished`, and `count_unit` must use `name = value` syntax",
                    ));
                }
                syn::Meta::NameValue(name_value) if name_value.path.is_ident("elapsed") => {
                    return Err(syn::Error::new(
                        name_value.path.span(),
                        "`elapsed = true` is no longer supported; use bare `show_elapsed`",
                    ));
                }
                syn::Meta::Path(path) if path.is_ident("elapsed") => {
                    return Err(syn::Error::new(
                        path.span(),
                        "`elapsed` is no longer supported; use bare `show_elapsed`",
                    ));
                }
                syn::Meta::Path(path) if path.is_ident("show_count_unit") => {
                    return Err(syn::Error::new(
                        path.span(),
                        "`show_count_unit` is not supported; `count_unit = (...)` implies display",
                    ));
                }
                syn::Meta::NameValue(name_value) if name_value.path.is_ident("show_count_unit") => {
                    return Err(syn::Error::new(
                        name_value.path.span(),
                        "`show_count_unit` is not supported; `count_unit = (...)` implies display",
                    ));
                }
                _ => {}
            }
            instrument_args.push(meta);
        }

        let message = message.ok_or_else(|| input.error("missing `message` argument"))?;
        let finished = finished.ok_or_else(|| input.error("missing `finished` argument"))?;
        let count_unit_display = match count_unit.as_ref() {
            Some(_unit) => CountUnitDisplay::Shown,
            None => CountUnitDisplay::Hidden,
        };
        let instrument_args = quote!(#(#instrument_args),*);

        Ok(ProgressAttrs {
            message,
            finished,
            elapsed_display,
            count_unit_display,
            count_unit,
            instrument_args,
        })
    }
}

pub fn expand(attr: TokenStream, item: TokenStream) -> TokenStream {
    let progress_attrs = match syn::parse::<ProgressAttrs>(attr) {
        Ok(attrs) => attrs,
        Err(err) => return err.to_compile_error().into(),
    };
    let mut func = match syn::parse::<ItemFn>(item) {
        Ok(func) => func,
        Err(err) => return err.to_compile_error().into(),
    };

    let message = &progress_attrs.message;
    let finished = &progress_attrs.finished;

    if !inject_progress_into_instrument(&mut func, progress_attrs.instrument_args.clone()) {
        let instrument_args = inject_progress_field(progress_attrs.instrument_args.clone());
        func.attrs
            .push(syn::parse_quote!(#[tracing::instrument(#instrument_args)]));
    }

    let original_stmts = std::mem::take(&mut func.block.stmts);
    let progress_usage = analyse_progress_usage(&original_stmts, &progress_attrs);
    if let Err(error) = validate_count_unit_usage(&progress_attrs, &progress_usage) {
        return error.to_compile_error().into();
    }
    let helper_macros = build_progress_helper_macros(&progress_usage);
    let body_eval = build_body_eval(&original_stmts, func.sig.asyncness.is_some());
    let progress_set_style = build_progress_set_style_tokens(
        progress_usage.style_template,
        progress_attrs.count_unit.as_ref(),
    );
    let ProgressHelperMacroTokens {
        set_length: progress_set_length_macro,
        inc_length: progress_inc_length_macro,
        inc: progress_inc_macro,
        trace: progress_trace_macro,
    } = helper_macros;

    func.block = syn::parse_quote!({
        use tracing_indicatif::span_ext::IndicatifSpanExt as _;
        let __progress_span = tracing::Span::current();
        #progress_set_style
        __progress_span.pb_set_message(#message);
        #progress_set_length_macro
        #progress_inc_length_macro
        #progress_inc_macro
        #progress_trace_macro
        let __progress_result = #body_eval;
        let __progress_finished = {
            let result = &__progress_result;
            (#finished).to_string()
        };
        __progress_span.pb_set_finish_message(&__progress_finished);
        tracing::info!(finished_message = %__progress_finished, "progress finished");
        __progress_result
    });

    quote!(#func).into()
}

/// Analyses the function body and resolves progress behaviour and template.
fn analyse_progress_usage(stmts: &[syn::Stmt], attrs: &ProgressAttrs) -> ProgressUsage {
    let uses_progress_set_length = contains_macro_call(stmts, "progress_set_length");
    let uses_progress_inc_length = contains_macro_call(stmts, "progress_inc_length");
    let uses_progress_inc = contains_macro_call(stmts, "progress_inc");
    let uses_progress_trace = contains_macro_call(stmts, "progress_trace");
    let mode = detect_progress_mode(
        uses_progress_set_length || uses_progress_inc_length,
        uses_progress_inc,
    );
    let style_template =
        progress_style_template(mode, attrs.elapsed_display, attrs.count_unit_display);

    ProgressUsage {
        uses_progress_set_length,
        uses_progress_inc_length,
        uses_progress_inc,
        uses_progress_trace,
        mode,
        style_template,
    }
}

/// Validates combinations that require counter semantics.
fn validate_count_unit_usage(attrs: &ProgressAttrs, usage: &ProgressUsage) -> syn::Result<()> {
    if let Some(count_unit) = &attrs.count_unit
        && !usage.uses_progress_inc
    {
        return Err(syn::Error::new(
            count_unit.span,
            "`count_unit` requires `progress_inc!` so the position can advance",
        ));
    }

    if usage.mode == ProgressMode::SpinnerOnly
        && attrs.count_unit_display == CountUnitDisplay::Shown
    {
        return Err(syn::Error::new(
            count_unit_span(attrs),
            "`count_unit` requires a counter progress mode",
        ));
    }

    Ok(())
}

/// Returns the source span for count-unit related diagnostics.
fn count_unit_span(attrs: &ProgressAttrs) -> proc_macro2::Span {
    attrs
        .count_unit
        .as_ref()
        .map_or(proc_macro2::Span::call_site(), |count_unit| count_unit.span)
}

/// Wraps the original function statements for uniform sync/async evaluation.
fn build_body_eval(stmts: &[syn::Stmt], is_async: bool) -> proc_macro2::TokenStream {
    if is_async {
        return quote!((async { #(#stmts)* }).await);
    }

    quote!((|| { #(#stmts)* })())
}

/// Builds helper macros injected into the function body based on usage.
fn build_progress_helper_macros(usage: &ProgressUsage) -> ProgressHelperMacroTokens {
    let set_length = if usage.uses_progress_set_length {
        quote! {
            macro_rules! progress_set_length {
                ($len:expr) => {{
                    let len = match u64::try_from($len) {
                        Ok(value) => value,
                        Err(_overflow) => u64::MAX,
                    };
                    __progress_span.pb_set_length(len);
                }};
            }
        }
    } else {
        quote! {}
    };
    let inc_length = if usage.uses_progress_inc_length {
        quote! {
            macro_rules! progress_inc_length {
                ($delta:expr) => {{
                    let delta = match u64::try_from($delta) {
                        Ok(value) => value,
                        Err(_overflow) => u64::MAX,
                    };
                    __progress_span.pb_inc_length(delta);
                }};
            }
        }
    } else {
        quote! {}
    };
    let inc = if usage.uses_progress_inc {
        quote! {
            macro_rules! progress_inc {
                () => {
                    progress_inc!(1usize);
                };
                ($delta:expr) => {{
                    let delta = match u64::try_from($delta) {
                        Ok(value) => value,
                        Err(_overflow) => u64::MAX,
                    };
                    __progress_span.pb_inc(delta);
                }};
            }
        }
    } else {
        quote! {}
    };
    let trace = if usage.uses_progress_trace {
        quote! {
            macro_rules! progress_trace {
                ($completed:expr, $total:expr) => {
                    tracing::trace!(
                        completed = $completed,
                        total = $total,
                        "chunked upload progress"
                    );
                };
            }
        }
    } else {
        quote! {}
    };

    ProgressHelperMacroTokens {
        set_length,
        inc_length,
        inc,
        trace,
    }
}

/// Parses `count_unit = ("singular", "plural")`.
fn parse_count_unit_name_value(name_value: &syn::MetaNameValue) -> syn::Result<CountUnit> {
    let syn::Expr::Tuple(tuple) = &name_value.value else {
        return Err(syn::Error::new(
            name_value.value.span(),
            "`count_unit` must be a tuple: (\"singular\", \"plural\")",
        ));
    };

    if tuple.elems.len() != 2 {
        return Err(syn::Error::new(
            tuple.span(),
            "`count_unit` must contain exactly two string literals",
        ));
    }

    let parse_lit = |expr: &syn::Expr| -> syn::Result<syn::LitStr> {
        let syn::Expr::Lit(expr_lit) = expr else {
            return Err(syn::Error::new(
                expr.span(),
                "`count_unit` values must be string literals",
            ));
        };
        let syn::Lit::Str(lit_str) = &expr_lit.lit else {
            return Err(syn::Error::new(
                expr_lit.lit.span(),
                "`count_unit` values must be string literals",
            ));
        };
        if lit_str.value().is_empty() {
            return Err(syn::Error::new(
                lit_str.span(),
                "`count_unit` values must not be empty",
            ));
        }
        Ok(lit_str.clone())
    };

    let singular = parse_lit(&tuple.elems[0])?;
    let plural = parse_lit(&tuple.elems[1])?;

    Ok(CountUnit {
        singular,
        plural,
        span: name_value.path.span(),
    })
}

/// Infers base progress mode from helper-macro usage in the body.
fn detect_progress_mode(uses_progress_length_api: bool, uses_progress_inc: bool) -> ProgressMode {
    if uses_progress_length_api {
        return ProgressMode::Determinate;
    }
    if uses_progress_inc {
        return ProgressMode::IndeterminateCount;
    }
    ProgressMode::SpinnerOnly
}

/// Builds style initialisation tokens for the current progress template.
fn build_progress_set_style_tokens(
    template: Option<ProgressTemplate>,
    count_unit: Option<&CountUnit>,
) -> proc_macro2::TokenStream {
    let Some(template) = template else {
        return quote! {};
    };
    let template = template.as_ref();

    if let Some(count_unit) = count_unit {
        let singular = &count_unit.singular;
        let plural = &count_unit.plural;
        return quote! {
            if let Ok(style) = tracing_indicatif::style::ProgressStyle::with_template(#template) {
                let style = style.with_key(
                    "count_unit",
                    move |state: &indicatif::ProgressState, writer: &mut dyn std::fmt::Write| {
                        let count_unit = if state.pos() == 1 {
                            #singular
                        } else {
                            #plural
                        };
                        let _ = writer.write_str(count_unit);
                    },
                );
                __progress_span.pb_set_style(&style);
            }
        };
    }

    quote! {
        if let Ok(style) = tracing_indicatif::style::ProgressStyle::with_template(#template) {
            __progress_span.pb_set_style(&style);
        }
    }
}

/// Resolves the template variant for the current display configuration.
fn progress_style_template(
    mode: ProgressMode,
    elapsed: ElapsedDisplay,
    count_unit: CountUnitDisplay,
) -> Option<ProgressTemplate> {
    let key = ProgressTemplateKey {
        mode,
        elapsed,
        count_unit,
    };

    match ProgressTemplate::try_from(key) {
        Ok(template) => Some(template),
        Err(ProgressTemplateLookupError::NoTemplate) => None,
        Err(ProgressTemplateLookupError::CountUnitWithoutCounter) => None,
    }
}

/// Finds an `#[instrument]` attribute on the function and injects
/// `progress = true` into its `fields(...)` argument, while appending forwarded
/// `#[progress(...)]` instrument arguments. Returns `true` if an `#[instrument]`
/// attribute was found and modified.
fn inject_progress_into_instrument(
    func: &mut ItemFn,
    forwarded_args: proc_macro2::TokenStream,
) -> bool {
    for attr in &mut func.attrs {
        if !attr.path().is_ident("instrument") {
            continue;
        }

        match &attr.meta {
            syn::Meta::List(meta_list) => {
                let mut combined_tokens = meta_list.tokens.clone();
                if !forwarded_args.is_empty() {
                    if !combined_tokens.is_empty() {
                        combined_tokens.extend(quote!(,));
                    }
                    combined_tokens.extend(forwarded_args.clone());
                }
                let modified = inject_progress_field(combined_tokens);
                *attr = syn::parse_quote!(#[instrument(#modified)]);
            }
            syn::Meta::Path(_) => {
                let modified = inject_progress_field(forwarded_args.clone());
                *attr = syn::parse_quote!(#[instrument(#modified)]);
            }
            _ => continue,
        }

        return true;
    }

    false
}

/// Walks the raw token stream of an `#[instrument(...)]` attribute and
/// injects `progress = true` into the `fields(...)` group. If no
/// `fields(...)` is present, appends `fields(progress = true)`.
fn inject_progress_field(tokens: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
    let trees: Vec<TokenTree> = tokens.into_iter().collect();
    let mut result = Vec::new();
    let mut found_fields = false;
    let mut i = 0;

    while i < trees.len() {
        if !is_fields_group(&trees, i) {
            result.push(trees[i].clone());
            i += 1;
            continue;
        }

        let ident = &trees[i];
        let TokenTree::Group(group) = &trees[i + 1] else {
            unreachable!();
        };

        found_fields = true;
        let inner = group.stream();
        let new_inner = if inner.is_empty() {
            quote!(progress = true)
        } else {
            quote!(#inner, progress = true)
        };
        result.push(ident.clone());
        result.push(TokenTree::Group(proc_macro2::Group::new(
            proc_macro2::Delimiter::Parenthesis,
            new_inner,
        )));
        i += 2;
    }

    let result_stream: proc_macro2::TokenStream = result.into_iter().collect();

    if found_fields {
        return result_stream;
    }
    if result_stream.is_empty() {
        return quote!(fields(progress = true));
    }
    quote!(#result_stream, fields(progress = true))
}

fn is_fields_group(trees: &[TokenTree], i: usize) -> bool {
    let TokenTree::Ident(ident) = &trees[i] else {
        return false;
    };
    if ident != "fields" {
        return false;
    }
    let Some(TokenTree::Group(group)) = trees.get(i + 1) else {
        return false;
    };
    group.delimiter() == proc_macro2::Delimiter::Parenthesis
}

fn contains_macro_call(stmts: &[syn::Stmt], macro_name: &str) -> bool {
    let stream = quote!(#(#stmts)*);
    contains_macro_call_tokens(stream, macro_name)
}

fn contains_macro_call_tokens(stream: proc_macro2::TokenStream, macro_name: &str) -> bool {
    let trees: Vec<TokenTree> = stream.into_iter().collect();
    let mut i = 0usize;
    while i < trees.len() {
        if let TokenTree::Ident(ident) = &trees[i]
            && ident == macro_name
            && matches!(trees.get(i + 1), Some(TokenTree::Punct(punct)) if punct.as_char() == '!')
        {
            return true;
        }

        if let TokenTree::Group(group) = &trees[i]
            && contains_macro_call_tokens(group.stream(), macro_name)
        {
            return true;
        }

        i += 1;
    }

    false
}
