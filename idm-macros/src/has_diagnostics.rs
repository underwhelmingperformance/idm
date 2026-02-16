use proc_macro::TokenStream;
use proc_macro_crate::{FoundCrate, crate_name};
use quote::quote;
use syn::{Data, DeriveInput, Fields};

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

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => {
                return syn::Error::new_spanned(
                    &input,
                    "HasDiagnostics can only be derived for structs with named fields",
                )
                .to_compile_error()
                .into();
            }
        },
        _ => {
            return syn::Error::new_spanned(&input, "HasDiagnostics can only be derived for structs")
                .to_compile_error()
                .into();
        }
    };

    let diagnostic_fields: Vec<_> = fields
        .iter()
        .filter(|field| {
            field
                .attrs
                .iter()
                .any(|attr| attr.path().is_ident("diagnostic"))
        })
        .filter_map(|field| field.ident.as_ref())
        .collect();

    if diagnostic_fields.is_empty() {
        return syn::Error::new_spanned(
            &input,
            "HasDiagnostics requires at least one field marked with #[diagnostic]",
        )
        .to_compile_error()
        .into();
    }

    let section_refs = diagnostic_fields.iter().map(|field| {
        quote! {
            &self.#field as &dyn #krate::hw::diagnostics::DiagnosticsSection
        }
    });

    quote! {
        impl #krate::hw::diagnostics::HasDiagnostics for #name {
            fn diagnostics(&self) -> Vec<&dyn #krate::hw::diagnostics::DiagnosticsSection> {
                vec![#(#section_refs),*]
            }
        }
    }
    .into()
}
