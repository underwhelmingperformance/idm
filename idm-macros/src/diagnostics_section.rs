use proc_macro::TokenStream;
use proc_macro_crate::{FoundCrate, crate_name};
use quote::quote;
use syn::{Data, DeriveInput, Fields, LitStr};

struct FieldAttrs {
    name: Option<String>,
    skip: bool,
}

fn idm_crate() -> proc_macro2::TokenStream {
    match crate_name("idm").expect("idm crate not found in Cargo.toml") {
        FoundCrate::Itself => quote!(crate),
        FoundCrate::Name(name) => {
            let ident = syn::Ident::new(&name, proc_macro2::Span::call_site());
            quote!(#ident)
        }
    }
}

pub fn expand(input: TokenStream) -> TokenStream {
    let input: DeriveInput = match syn::parse(input) {
        Ok(input) => input,
        Err(err) => return err.to_compile_error().into(),
    };

    let name = &input.ident;
    let krate = idm_crate();
    let section_id = parse_section_attr(&input, "id").unwrap_or_else(|| name.to_string());
    let section_name = parse_section_attr(&input, "section").unwrap_or_else(|| name.to_string());

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => {
                return syn::Error::new_spanned(
                    &input,
                    "DiagnosticsSection can only be derived for structs with named fields",
                )
                .to_compile_error()
                .into();
            }
        },
        _ => {
            return syn::Error::new_spanned(
                &input,
                "DiagnosticsSection can only be derived for structs",
            )
            .to_compile_error()
            .into();
        }
    };

    let mut row_exprs = Vec::new();
    for field in fields {
        let field_name = field.ident.as_ref().expect("named field required");
        let field_attrs = match parse_field_attrs(field) {
            Ok(attrs) => attrs,
            Err(err) => return err.to_compile_error().into(),
        };

        if field_attrs.skip {
            continue;
        }

        let display_name = field_attrs
            .name
            .unwrap_or_else(|| field_name_to_display(field_name));

        row_exprs.push(quote! {
            #krate::hw::diagnostics::DiagnosticRow::new(#display_name, &self.#field_name)
        });
    }

    quote! {
        impl #krate::hw::diagnostics::DiagnosticsSection for #name {
            fn section_id(&self) -> &'static str {
                #section_id
            }

            fn section_name(&self) -> &'static str {
                #section_name
            }

            fn rows(&self) -> Vec<#krate::hw::diagnostics::DiagnosticRow> {
                vec![#(#row_exprs),*]
            }
        }
    }
    .into()
}

fn parse_section_attr(input: &DeriveInput, key: &str) -> Option<String> {
    for attr in &input.attrs {
        if !attr.path().is_ident("diagnostics") {
            continue;
        }

        let mut parsed = None;
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident(key) {
                let value: LitStr = meta.value()?.parse()?;
                parsed = Some(value.value());
            }
            Ok(())
        });

        if parsed.is_some() {
            return parsed;
        }
    }

    None
}

fn parse_field_attrs(field: &syn::Field) -> Result<FieldAttrs, syn::Error> {
    let mut attrs = FieldAttrs {
        name: None,
        skip: false,
    };

    for attr in &field.attrs {
        if !attr.path().is_ident("diagnostic") {
            continue;
        }

        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                let value: LitStr = meta.value()?.parse()?;
                attrs.name = Some(value.value());
            } else if meta.path.is_ident("skip") {
                attrs.skip = true;
            }
            Ok(())
        })?;
    }

    Ok(attrs)
}

fn field_name_to_display(ident: &syn::Ident) -> String {
    ident
        .to_string()
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
