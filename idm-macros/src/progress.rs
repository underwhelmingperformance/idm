use proc_macro::TokenStream;
use proc_macro2::TokenTree;
use quote::quote;
use syn::spanned::Spanned;
use syn::ItemFn;

struct ProgressAttrs {
    message: syn::Expr,
    finished: syn::Expr,
    instrument_args: proc_macro2::TokenStream,
}

impl syn::parse::Parse for ProgressAttrs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut message = None;
        let mut finished = None;
        let mut instrument_args = Vec::new();

        let metas =
            syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated(input)?;
        for meta in metas {
            if let syn::Meta::NameValue(name_value) = &meta {
                if name_value.path.is_ident("message") {
                    if message.is_some() {
                        return Err(syn::Error::new(
                            name_value.path.span(),
                            "`message` provided more than once",
                        ));
                    }
                    message = Some(name_value.value.clone());
                    continue;
                }
                if name_value.path.is_ident("finished") {
                    if finished.is_some() {
                        return Err(syn::Error::new(
                            name_value.path.span(),
                            "`finished` provided more than once",
                        ));
                    }
                    finished = Some(name_value.value.clone());
                    continue;
                }
            } else if meta.path().is_ident("message") || meta.path().is_ident("finished") {
                return Err(syn::Error::new(
                    meta.path().span(),
                    "`message` and `finished` must use `name = value` syntax",
                ));
            }
            instrument_args.push(meta);
        }

        let message = message.ok_or_else(|| input.error("missing `message` argument"))?;
        let finished = finished.ok_or_else(|| input.error("missing `finished` argument"))?;
        let instrument_args = quote!(#(#instrument_args),*);

        Ok(ProgressAttrs {
            message,
            finished,
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
    let uses_progress_set_length = contains_macro_call(&original_stmts, "progress_set_length");
    let uses_progress_inc_length = contains_macro_call(&original_stmts, "progress_inc_length");
    let uses_progress_inc = contains_macro_call(&original_stmts, "progress_inc");
    let uses_progress_trace = contains_macro_call(&original_stmts, "progress_trace");
    let body_eval = if func.sig.asyncness.is_some() {
        quote!((async { #(#original_stmts)* }).await)
    } else {
        quote!((|| { #(#original_stmts)* })())
    };

    let progress_set_length_macro = if uses_progress_set_length {
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
    let progress_inc_length_macro = if uses_progress_inc_length {
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
    let progress_inc_macro = if uses_progress_inc {
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
    let progress_trace_macro = if uses_progress_trace {
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

    func.block = syn::parse_quote!({
        use tracing_indicatif::span_ext::IndicatifSpanExt as _;
        let __progress_span = tracing::Span::current();
        __progress_span.pb_set_message(#message);
        __progress_span.pb_set_length(0);
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
