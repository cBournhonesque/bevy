use bevy_macro_utils::{get_lit_str, Symbol};
use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{quote, ToTokens};
use syn::{parse_macro_input, parse_quote, DeriveInput, Error, Ident, LitBool, Path, Result};

pub fn derive_resource(input: TokenStream) -> TokenStream {
    let mut ast = parse_macro_input!(input as DeriveInput);
    let bevy_ecs_path: Path = crate::bevy_ecs_path();

    let attrs = match parse_resource_attrs(&ast) {
        Ok(attrs) => attrs,
        Err(e) => return e.into_compile_error().into(),
    };

    ast.generics
        .make_where_clause()
        .predicates
        .push(parse_quote! { Self: Send + Sync + 'static });

    let change_detection_enabled = LitBool::new(attrs.change_detection_enabled, Span::call_site());

    let struct_name = &ast.ident;
    let (impl_generics, type_generics, where_clause) = &ast.generics.split_for_impl();

    TokenStream::from(quote! {
        impl #impl_generics #bevy_ecs_path::system::Resource for #struct_name #type_generics #where_clause {
            const CHANGE_DETECTION_ENABLED: bool = #change_detection_enabled;
        }
    })
}

pub fn derive_component(input: TokenStream) -> TokenStream {
    let mut ast = parse_macro_input!(input as DeriveInput);
    let bevy_ecs_path: Path = crate::bevy_ecs_path();

    let attrs = match parse_component_attrs(&ast) {
        Ok(attrs) => attrs,
        Err(e) => return e.into_compile_error().into(),
    };

    let change_detection_enabled = LitBool::new(attrs.change_detection_enabled, Span::call_site());
    let storage = storage_path(&bevy_ecs_path, attrs.storage);

    ast.generics
        .make_where_clause()
        .predicates
        .push(parse_quote! { Self: Send + Sync + 'static });

    let struct_name = &ast.ident;
    let (impl_generics, type_generics, where_clause) = &ast.generics.split_for_impl();

    TokenStream::from(quote! {
        impl #impl_generics #bevy_ecs_path::component::Component for #struct_name #type_generics #where_clause {
            const CHANGE_DETECTION_ENABLED: bool = #change_detection_enabled;
            type Storage = #storage;
        }
    })
}

pub const COMPONENT: Symbol = Symbol("component");
pub const RESOURCE: Symbol = Symbol("resource");
pub const CHANGED_DETECTION: Symbol = Symbol("change_detection");
pub const STORAGE: Symbol = Symbol("storage");

struct ComponentAttrs {
    change_detection_enabled: bool,
    storage: StorageTy,
}

struct ResourceAttrs {
    change_detection_enabled: bool,
}

#[derive(Clone, Copy)]
enum StorageTy {
    Table,
    SparseSet,
}

// values for `storage` attribute
const TABLE: &str = "Table";
const SPARSE_SET: &str = "SparseSet";

fn parse_component_attrs(ast: &DeriveInput) -> Result<ComponentAttrs> {
    let meta_items = bevy_macro_utils::parse_attrs(ast, COMPONENT)?;

    let mut attrs = ComponentAttrs {
        change_detection_enabled: true,
        storage: StorageTy::Table,
    };

    for meta in meta_items {
        use syn::{
            Meta::NameValue,
            NestedMeta::{Lit, Meta},
        };
        match meta {
            Meta(NameValue(m)) if m.path == STORAGE => {
                attrs.storage = match get_lit_str(STORAGE, &m.lit)?.value().as_str() {
                    TABLE => StorageTy::Table,
                    SPARSE_SET => StorageTy::SparseSet,
                    s => {
                        return Err(Error::new_spanned(
                            m.lit,
                            format!(
                                "Invalid storage type `{}`, expected '{}' or '{}'.",
                                s, TABLE, SPARSE_SET
                            ),
                        ))
                    }
                };
            }
            Meta(NameValue(m)) if m.path == CHANGED_DETECTION => {
                attrs.change_detection_enabled = match m.lit {
                    syn::Lit::Bool(value) => value.value,
                    s => {
                        return Err(Error::new_spanned(
                            s,
                            "Change detection must be a bool, expected 'true' or 'false'.",
                        ))
                    }
                };
            }
            Meta(meta_item) => {
                return Err(Error::new_spanned(
                    meta_item.path(),
                    format!(
                        "unknown component attribute `{}`",
                        meta_item.path().into_token_stream()
                    ),
                ));
            }
            Lit(lit) => {
                return Err(Error::new_spanned(
                    lit,
                    "unexpected literal in component attribute",
                ))
            }
        }
    }

    Ok(attrs)
}

fn parse_resource_attrs(ast: &DeriveInput) -> Result<ResourceAttrs> {
    let meta_items = bevy_macro_utils::parse_attrs(ast, RESOURCE)?;

    let mut attrs = ResourceAttrs {
        change_detection_enabled: true,
    };

    for meta in meta_items {
        use syn::{
            Meta::NameValue,
            NestedMeta::{Lit, Meta},
        };
        match meta {
            Meta(NameValue(m)) if m.path == CHANGED_DETECTION => {
                attrs.change_detection_enabled = match m.lit {
                    syn::Lit::Bool(value) => value.value,
                    s => {
                        return Err(Error::new_spanned(
                            s,
                            "Change detection must be a bool, expected 'true' or 'false'.",
                        ))
                    }
                };
            }
            Meta(meta_item) => {
                return Err(Error::new_spanned(
                    meta_item.path(),
                    format!(
                        "unknown resource attribute `{}`",
                        meta_item.path().into_token_stream()
                    ),
                ));
            }
            Lit(lit) => {
                return Err(Error::new_spanned(
                    lit,
                    "unexpected literal in resource attribute",
                ))
            }
        }
    }

    Ok(attrs)
}

fn storage_path(bevy_ecs_path: &Path, ty: StorageTy) -> TokenStream2 {
    let typename = match ty {
        StorageTy::Table => Ident::new("TableStorage", Span::call_site()),
        StorageTy::SparseSet => Ident::new("SparseStorage", Span::call_site()),
    };

    quote! { #bevy_ecs_path::component::#typename }
}
