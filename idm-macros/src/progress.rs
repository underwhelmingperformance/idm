use proc_macro::TokenStream;
use proc_macro2::TokenTree;
use quote::quote;
use syn::ItemFn;

struct ProgressAttrs {
    message: syn::Expr,
    finished: syn::Expr,
}

impl syn::parse::Parse for ProgressAttrs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut message = None;
        let mut finished = None;

        while !input.is_empty() {
            let ident: syn::Ident = input.parse()?;
            input.parse::<syn::Token![=]>()?;

            if ident == "message" {
                message = Some(input.parse::<syn::Expr>()?);
            } else if ident == "finished" {
                finished = Some(input.parse::<syn::Expr>()?);
            } else {
                return Err(syn::Error::new(
                    ident.span(),
                    format!("unknown argument `{ident}`"),
                ));
            }

            if !input.is_empty() {
                input.parse::<syn::Token![,]>()?;
            }
        }

        let message = message.ok_or_else(|| input.error("missing `message` argument"))?;
        let finished = finished.ok_or_else(|| input.error("missing `finished` argument"))?;

        Ok(ProgressAttrs { message, finished })
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

    if !inject_progress_into_instrument(&mut func) {
        func.attrs
            .push(syn::parse_quote!(#[tracing::instrument(fields(progress = true))]));
    }

    let original_stmts = std::mem::take(&mut func.block.stmts);
    func.block = syn::parse_quote!({
        {
            use tracing_indicatif::span_ext::IndicatifSpanExt as _;
            let __progress_span = tracing::Span::current();
            __progress_span.pb_set_message(#message);
            __progress_span.pb_set_finish_message(&#finished);
        }
        #(#original_stmts)*
    });

    quote!(#func).into()
}

/// Finds an `#[instrument]` attribute on the function and injects
/// `progress = true` into its `fields(...)` argument. Returns `true` if
/// an `#[instrument]` attribute was found and modified.
fn inject_progress_into_instrument(func: &mut ItemFn) -> bool {
    for attr in &mut func.attrs {
        if !attr.path().is_ident("instrument") {
            continue;
        }

        match &attr.meta {
            syn::Meta::List(meta_list) => {
                let modified = inject_progress_field(meta_list.tokens.clone());
                *attr = syn::parse_quote!(#[instrument(#modified)]);
            }
            syn::Meta::Path(_) => {
                *attr = syn::parse_quote!(#[instrument(fields(progress = true))]);
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
