// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The derive macros of the polyfill's typed value traits:
//! `ComponentType`, `Lift`, and `Lower`.
//!
//! The attributes and the shape rules are Wasmtime's
//! (`crates/component-macro/src/component.rs` at `v49.0.0-rc.1`): a
//! style attribute, `record`, `variant`, or `enum`, on the type, and
//! `name = "…"` on a field or a case. A crate that re-exports the
//! polyfill names it with `wcmp_crate = path`. Each refusal is a
//! compile error with the meaning of Wasmtime's.
//!
//! The code the derives write lifts and lowers field by field through
//! the polyfill's hidden lift and lower contexts, and builds no `Val`.
//! Its layout constants come from the fields' at compile time.

use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{Data, DeriveInput, Error, Ident, Result, Token};

mod kw {
    syn::custom_keyword!(record);
    syn::custom_keyword!(variant);
    syn::custom_keyword!(flags);
    syn::custom_keyword!(name);
    syn::custom_keyword!(wcmp_crate);
    syn::custom_keyword!(wasmtime_crate);
}

/// Which of the three traits a derive writes.
#[derive(Clone, Copy, Debug)]
pub enum Trait {
    /// `ComponentType`: the layout and the type check.
    ComponentType,
    /// `Lift`: reading a value out of a guest.
    Lift,
    /// `Lower`: writing a value into a guest.
    Lower,
}

/// What WIT type a derived type stands for.
#[derive(Clone, Copy, Debug)]
enum Style {
    Record,
    Enum,
    Variant,
}

impl core::fmt::Display for Style {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Style::Record => "record",
            Style::Enum => "enum",
            Style::Variant => "variant",
        })
    }
}

/// One `component` attribute on a derived type.
enum ComponentAttr {
    Style(Style),
    WcmpCrate(syn::Path),
}

impl Parse for ComponentAttr {
    fn parse(input: ParseStream) -> Result<Self> {
        let lookahead = input.lookahead1();
        if lookahead.peek(kw::record) {
            input.parse::<kw::record>()?;
            Ok(ComponentAttr::Style(Style::Record))
        } else if lookahead.peek(kw::variant) {
            input.parse::<kw::variant>()?;
            Ok(ComponentAttr::Style(Style::Variant))
        } else if lookahead.peek(Token![enum]) {
            input.parse::<Token![enum]>()?;
            Ok(ComponentAttr::Style(Style::Enum))
        } else if lookahead.peek(kw::wcmp_crate) {
            input.parse::<kw::wcmp_crate>()?;
            input.parse::<Token![=]>()?;
            Ok(ComponentAttr::WcmpCrate(input.parse()?))
        } else if input.peek(kw::wasmtime_crate) {
            Err(input.error(
                "`wasmtime_crate` not allowed here; \
                 use `wcmp_crate = path` to name the crate that re-exports the polyfill",
            ))
        } else if input.peek(kw::flags) {
            Err(input.error(
                "`flags` not allowed here; use `wcmp::flags!` macro to define `flags` types",
            ))
        } else {
            Err(lookahead.error())
        }
    }
}

/// The WIT name a field or a case gives with `#[component(name = "…")]`.
/// The path of `#[component(wcmp_crate = path)]`, which `flags!` takes
/// before the type's name.
pub fn parse_crate_path(attribute: &syn::Attribute) -> Result<syn::Path> {
    match attribute.parse_args()? {
        ComponentAttr::WcmpCrate(path) => Ok(path),
        ComponentAttr::Style(_) => Err(Error::new_spanned(
            attribute,
            "only `wcmp_crate = path` may precede the name of a flags type",
        )),
    }
}

pub fn find_rename(attributes: &[syn::Attribute]) -> Result<Option<syn::LitStr>> {
    let mut name = None;
    for attribute in attributes {
        if !attribute.path().is_ident("component") {
            continue;
        }
        let literal = attribute.parse_args_with(|parser: ParseStream<'_>| {
            parser.parse::<kw::name>()?;
            parser.parse::<Token![=]>()?;
            parser.parse::<syn::LitStr>()
        })?;
        if name.is_some() {
            return Err(Error::new_spanned(
                attribute,
                "duplicate field rename attribute",
            ));
        }
        name = Some(literal);
    }
    Ok(name)
}

/// The WIT name of a field or a case: its rename, or its Rust name as
/// written.
fn wit_name(attributes: &[syn::Attribute], ident: &Ident) -> Result<String> {
    Ok(find_rename(attributes)?
        .map(|literal| literal.value())
        .unwrap_or_else(|| ident.to_string()))
}

/// `generics` with `bound` added to each type parameter.
fn add_trait_bounds(generics: &syn::Generics, bound: syn::TypeParamBound) -> syn::Generics {
    let mut generics = generics.clone();
    for param in &mut generics.params {
        if let syn::GenericParam::Type(type_param) = param {
            type_param.bounds.push(bound.clone());
        }
    }
    generics
}

/// One case of a derived variant or enum.
struct Case<'a> {
    name: String,
    ident: &'a Ident,
    ty: Option<&'a syn::Type>,
}

/// One field of a derived record.
struct Field<'a> {
    name: String,
    ident: &'a Ident,
    ty: &'a syn::Type,
}

/// Expand the derive of `which` for `input`.
pub fn expand(which: Trait, input: &DeriveInput) -> Result<TokenStream> {
    let mut crate_path = None;
    let mut style = None;
    for attribute in &input.attrs {
        if !attribute.path().is_ident("component") {
            continue;
        }
        match attribute.parse_args()? {
            ComponentAttr::WcmpCrate(path) => crate_path = Some(path),
            ComponentAttr::Style(found) => {
                if style.is_some() {
                    return Err(Error::new_spanned(
                        attribute,
                        "duplicate `component` attribute",
                    ));
                }
                style = Some(found);
            }
        }
    }
    let style = style.ok_or_else(|| Error::new_spanned(input, "missing `component` attribute"))?;
    let crate_path = crate_path.unwrap_or_else(|| {
        let mut path: syn::Path = Ident::new("wcmp", Span::call_site()).into();
        path.leading_colon = Some(Default::default());
        path
    });
    match style {
        Style::Record => {
            let fields = record_fields(input)?;
            Ok(match which {
                Trait::ComponentType => record_component_type(input, &fields, &crate_path),
                Trait::Lift => record_lift(input, &fields, &crate_path),
                Trait::Lower => record_lower(input, &fields, &crate_path),
            })
        }
        Style::Variant | Style::Enum => {
            let cases = cases(input, style)?;
            if let Style::Enum = style {
                validate_enum(input, cases.len())?;
            }
            Ok(match which {
                Trait::ComponentType => cases_component_type(input, style, &cases, &crate_path),
                Trait::Lift => cases_lift(input, &cases, &crate_path),
                Trait::Lower => cases_lower(input, &cases, &crate_path),
            })
        }
    }
}

/// The named fields of a derived record.
fn record_fields(input: &DeriveInput) -> Result<Vec<Field<'_>>> {
    let name = &input.ident;
    let Data::Struct(body) = &input.data else {
        return Err(Error::new(
            name.span(),
            "`record` component types can only be derived for Rust `struct`s",
        ));
    };
    match &body.fields {
        syn::Fields::Named(fields) => fields
            .named
            .iter()
            .map(|field| {
                let ident = field.ident.as_ref().expect("a named field has a name");
                Ok(Field {
                    name: wit_name(&field.attrs, ident)?,
                    ident,
                    ty: &field.ty,
                })
            })
            .collect(),
        syn::Fields::Unnamed(_) | syn::Fields::Unit => Err(Error::new(
            name.span(),
            "`record` component types can only be derived for `struct`s with named fields",
        )),
    }
}

/// The cases of a derived variant or enum.
fn cases(input: &DeriveInput, style: Style) -> Result<Vec<Case<'_>>> {
    let name = &input.ident;
    let Data::Enum(body) = &input.data else {
        return Err(Error::new(
            name.span(),
            format!("`{style}` component types can only be derived for Rust `enum`s"),
        ));
    };
    if body.variants.is_empty() {
        return Err(Error::new(
            name.span(),
            format!(
                "`{style}` component types can only be derived for Rust `enum`s with at least \
                 one variant"
            ),
        ));
    }
    if u32::try_from(body.variants.len()).is_err() {
        return Err(Error::new(
            name.span(),
            "`enum`s with more than 2^32 variants are not supported",
        ));
    }
    body.variants
        .iter()
        .map(|variant| {
            let ty = match &variant.fields {
                syn::Fields::Unnamed(fields)
                    if fields.unnamed.len() == 1 && matches!(style, Style::Variant) =>
                {
                    Some(&fields.unnamed[0].ty)
                }
                syn::Fields::Unit => None,
                _ => {
                    return Err(Error::new(
                        name.span(),
                        format!(
                            "`{style}` component types can only be derived for Rust `enum`s \
                             containing variants with {}",
                            match style {
                                Style::Variant => "at most one unnamed field each",
                                Style::Enum | Style::Record => "no fields",
                            }
                        ),
                    ));
                }
            };
            Ok(Case {
                name: wit_name(&variant.attrs, &variant.ident)?,
                ident: &variant.ident,
                ty,
            })
        })
        .collect()
}

/// The `#[repr]` a derived enum of `count` cases needs, as Wasmtime's
/// discriminant size gives it.
fn discriminant(count: usize) -> &'static str {
    if count <= 1 << 8 {
        "u8"
    } else if count <= 1 << 16 {
        "u16"
    } else {
        "u32"
    }
}

/// Refuse a derived enum without the `#[repr]` its count of cases
/// needs, with generics, or with an explicit discriminant, as
/// Wasmtime's `validate_enum` does.
fn validate_enum(input: &DeriveInput, count: usize) -> Result<()> {
    let Data::Enum(body) = &input.data else {
        unreachable!("the cases of an enum were read")
    };
    if !input.generics.params.is_empty() {
        return Err(Error::new_spanned(
            &input.generics.params,
            "cannot have generics on an `enum`",
        ));
    }
    if let Some(clause) = &input.generics.where_clause {
        return Err(Error::new_spanned(
            clause,
            "cannot have a where clause on an `enum`",
        ));
    }
    let expected = discriminant(count);
    let mut found = false;
    for attribute in &input.attrs {
        if !attribute.meta.path().is_ident("repr") {
            continue;
        }
        let list = attribute.meta.require_list()?;
        found = true;
        if list.tokens.to_string() != expected {
            return Err(Error::new_spanned(
                &list.tokens,
                format!("expected `repr({expected})`, found `repr({})`", list.tokens),
            ));
        }
    }
    if !found {
        return Err(Error::new_spanned(
            body.enum_token,
            format!("missing required `#[repr({expected})]`"),
        ));
    }
    for case in &body.variants {
        if let Some((_, expr)) = &case.discriminant {
            return Err(Error::new_spanned(
                expr,
                "cannot have an explicit discriminant",
            ));
        }
    }
    Ok(())
}

/// The `impl` header pieces of a derive of `bound` for `input`.
fn header(input: &DeriveInput, bound: TokenStream) -> (TokenStream, TokenStream, TokenStream) {
    let generics = add_trait_bounds(&input.generics, syn::parse_quote!(#bound));
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    (
        quote!(#impl_generics),
        quote!(#ty_generics),
        quote!(#where_clause),
    )
}

fn record_component_type(input: &DeriveInput, fields: &[Field<'_>], c: &syn::Path) -> TokenStream {
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = header(input, quote!(#c::ComponentType));
    let types = fields.iter().map(|field| field.ty);
    let checks = fields.iter().map(|field| {
        let wit = &field.name;
        let ty = field.ty;
        quote!((#wit, <#ty as #c::ComponentType>::typecheck as #c::__private::TypeCheckFn))
    });
    quote! {
        impl #impl_generics #c::ComponentType for #name #ty_generics #where_clause {
            #[doc(hidden)]
            const ABI: #c::__private::CanonicalAbiInfo =
                #c::__private::CanonicalAbiInfo::record_static(&[
                    #(<#types as #c::ComponentType>::ABI),*
                ]);

            #[doc(hidden)]
            fn typecheck(
                ty: &#c::ValueType,
                types: &#c::__private::TypeChecker<'_>,
            ) -> ::core::result::Result<(), #c::__private::TypeCheckError> {
                #c::__private::typecheck_record::<Self>(ty, types, &[#(#checks),*])
            }
        }
    }
}

fn record_lower(input: &DeriveInput, fields: &[Field<'_>], c: &syn::Path) -> TokenStream {
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = header(input, quote!(#c::Lower));
    let count = fields.len();
    let indices = 0..count;
    let idents: Vec<_> = fields.iter().map(|field| field.ident).collect();
    let types: Vec<_> = fields.iter().map(|field| field.ty).collect();
    let flat = idents.iter().zip(indices.clone()).map(|(ident, index)| {
        quote!(#c::Lower::lower_flat(&self.#ident, cx, fields[#index].ty(), out)?;)
    });
    let memory = idents
        .iter()
        .zip(&types)
        .zip(indices)
        .map(|((ident, ty), index)| {
            quote! {
                field = #c::__private::align_to(field, <#ty as #c::ComponentType>::ALIGN32);
                #c::Lower::lower_memory(&self.#ident, cx, fields[#index].ty(), offset + field)?;
                field += <#ty as #c::ComponentType>::SIZE32;
            }
        });
    quote! {
        impl #impl_generics #c::Lower for #name #ty_generics #where_clause {
            #[doc(hidden)]
            fn lower_flat<D: 'static>(
                &self,
                cx: &mut #c::__private::LowerContext<'_, '_, D>,
                ty: &#c::ValueType,
                out: &mut ::std::vec::Vec<#c::__private::CoreVal>,
            ) -> #c::Result<()> {
                let fields = match ty {
                    #c::ValueType::Record(record) if record.fields().len() == #count => {
                        record.fields()
                    }
                    _ => return ::core::result::Result::Err(cx.mismatch(ty)),
                };
                #(#flat)*
                ::core::result::Result::Ok(())
            }

            #[doc(hidden)]
            #[allow(unused_assignments, unused_mut, unused_variables)]
            fn lower_memory<D: 'static>(
                &self,
                cx: &mut #c::__private::LowerContext<'_, '_, D>,
                ty: &#c::ValueType,
                offset: usize,
            ) -> #c::Result<()> {
                let fields = match ty {
                    #c::ValueType::Record(record) if record.fields().len() == #count => {
                        record.fields()
                    }
                    _ => return ::core::result::Result::Err(cx.mismatch(ty)),
                };
                let mut field = 0usize;
                #(#memory)*
                ::core::result::Result::Ok(())
            }

            #[doc(hidden)]
            #[allow(unused_variables)]
            fn check_store(&self, store: #c::StoreId) -> #c::Result<()> {
                #(#c::Lower::check_store(&self.#idents, store)?;)*
                ::core::result::Result::Ok(())
            }
        }
    }
}

fn record_lift(input: &DeriveInput, fields: &[Field<'_>], c: &syn::Path) -> TokenStream {
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = header(input, quote!(#c::Lift));
    let count = fields.len();
    let idents: Vec<_> = fields.iter().map(|field| field.ident).collect();
    let types: Vec<_> = fields.iter().map(|field| field.ty).collect();
    let flat = idents.iter().zip(&types).enumerate().map(|(index, (ident, ty))| {
        quote!(#ident: <#ty as #c::Lift>::lift_flat(cx, fields[#index].ty(), slots, cursor)?,)
    });
    let memory = idents.iter().zip(&types).enumerate().map(|(index, (ident, ty))| {
        quote! {
            #ident: {
                field = #c::__private::align_to(field, <#ty as #c::ComponentType>::ALIGN32);
                let value = <#ty as #c::Lift>::lift_memory(cx, fields[#index].ty(), offset + field)?;
                field += <#ty as #c::ComponentType>::SIZE32;
                value
            },
        }
    });
    quote! {
        impl #impl_generics #c::Lift for #name #ty_generics #where_clause {
            #[doc(hidden)]
            #[allow(unused_variables)]
            fn lift_flat<D: 'static>(
                cx: &mut #c::__private::LiftContext<'_, '_, D>,
                ty: &#c::ValueType,
                slots: &[#c::__private::CoreVal],
                cursor: &mut usize,
            ) -> #c::Result<Self> {
                let fields = match ty {
                    #c::ValueType::Record(record) if record.fields().len() == #count => {
                        record.fields()
                    }
                    _ => return ::core::result::Result::Err(cx.mismatch(ty)),
                };
                ::core::result::Result::Ok(Self { #(#flat)* })
            }

            #[doc(hidden)]
            #[allow(unused_assignments, unused_mut, unused_variables)]
            fn lift_memory<D: 'static>(
                cx: &mut #c::__private::LiftContext<'_, '_, D>,
                ty: &#c::ValueType,
                offset: usize,
            ) -> #c::Result<Self> {
                let fields = match ty {
                    #c::ValueType::Record(record) if record.fields().len() == #count => {
                        record.fields()
                    }
                    _ => return ::core::result::Result::Err(cx.mismatch(ty)),
                };
                let mut field = 0usize;
                ::core::result::Result::Ok(Self { #(#memory)* })
            }
        }
    }
}

fn cases_component_type(
    input: &DeriveInput,
    style: Style,
    cases: &[Case<'_>],
    c: &syn::Path,
) -> TokenStream {
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = header(input, quote!(#c::ComponentType));
    let (abi, check) = match style {
        Style::Enum => {
            let count = cases.len();
            let names = cases.iter().map(|case| &case.name);
            (
                quote!(#c::__private::CanonicalAbiInfo::enum_(#count)),
                quote!(#c::__private::typecheck_enum::<Self>(ty, &[#(#names),*])),
            )
        }
        Style::Variant | Style::Record => {
            let payloads = cases.iter().map(|case| match case.ty {
                Some(ty) => quote!(::core::option::Option::Some(<#ty as #c::ComponentType>::ABI)),
                None => quote!(::core::option::Option::None),
            });
            let checks = cases.iter().map(|case| {
                let wit = &case.name;
                match case.ty {
                    Some(ty) => quote!((#wit, ::core::option::Option::Some(
                        <#ty as #c::ComponentType>::typecheck
                            as #c::__private::TypeCheckFn
                    ))),
                    None => quote!((#wit, ::core::option::Option::None)),
                }
            });
            (
                quote!(#c::__private::CanonicalAbiInfo::variant_static(&[#(#payloads),*])),
                quote!(#c::__private::typecheck_variant::<Self>(ty, types, &[#(#checks),*])),
            )
        }
    };
    quote! {
        impl #impl_generics #c::ComponentType for #name #ty_generics #where_clause {
            #[doc(hidden)]
            const ABI: #c::__private::CanonicalAbiInfo = #abi;

            #[doc(hidden)]
            #[allow(unused_variables)]
            fn typecheck(
                ty: &#c::ValueType,
                types: &#c::__private::TypeChecker<'_>,
            ) -> ::core::result::Result<(), #c::__private::TypeCheckError> {
                #check
            }
        }
    }
}

fn cases_lower(input: &DeriveInput, cases: &[Case<'_>], c: &syn::Path) -> TokenStream {
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = header(input, quote!(#c::Lower));
    let flat = cases.iter().enumerate().map(|(index, case)| {
        let ident = case.ident;
        match case.ty {
            Some(_) => quote! {
                Self::#ident(value) => #c::__private::lower_case_flat(
                    cx, ty, #index, out,
                    |cx, payload, out| #c::Lower::lower_flat(value, cx, payload, out),
                ),
            },
            None => quote! {
                Self::#ident => #c::__private::lower_case_flat(cx, ty, #index, out, |_, _, _| {
                    ::core::result::Result::Ok(())
                }),
            },
        }
    });
    let memory = cases.iter().enumerate().map(|(index, case)| {
        let ident = case.ident;
        match case.ty {
            Some(_) => quote! {
                Self::#ident(value) => {
                    let (payload, payload_ty) =
                        #c::__private::lower_case_memory(cx, ty, #index, offset)?;
                    match payload_ty {
                        ::core::option::Option::Some(payload_ty) => {
                            #c::Lower::lower_memory(value, cx, payload_ty, payload)
                        }
                        ::core::option::Option::None => ::core::result::Result::Err(cx.mismatch(ty)),
                    }
                }
            },
            None => quote! {
                Self::#ident => {
                    #c::__private::lower_case_memory(cx, ty, #index, offset)?;
                    ::core::result::Result::Ok(())
                }
            },
        }
    });
    let stores = cases.iter().map(|case| {
        let ident = case.ident;
        match case.ty {
            Some(_) => quote!(Self::#ident(value) => #c::Lower::check_store(value, store),),
            None => quote!(Self::#ident => ::core::result::Result::Ok(()),),
        }
    });
    quote! {
        impl #impl_generics #c::Lower for #name #ty_generics #where_clause {
            #[doc(hidden)]
            fn lower_flat<D: 'static>(
                &self,
                cx: &mut #c::__private::LowerContext<'_, '_, D>,
                ty: &#c::ValueType,
                out: &mut ::std::vec::Vec<#c::__private::CoreVal>,
            ) -> #c::Result<()> {
                match self { #(#flat)* }
            }

            #[doc(hidden)]
            fn lower_memory<D: 'static>(
                &self,
                cx: &mut #c::__private::LowerContext<'_, '_, D>,
                ty: &#c::ValueType,
                offset: usize,
            ) -> #c::Result<()> {
                match self { #(#memory)* }
            }

            #[doc(hidden)]
            #[allow(unused_variables)]
            fn check_store(&self, store: #c::StoreId) -> #c::Result<()> {
                match self { #(#stores)* }
            }
        }
    }
}

fn cases_lift(input: &DeriveInput, cases: &[Case<'_>], c: &syn::Path) -> TokenStream {
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = header(input, quote!(#c::Lift));
    let flat = cases.iter().enumerate().map(|(index, case)| {
        let ident = case.ident;
        match case.ty {
            Some(ty) => quote! {
                (#index, ::core::option::Option::Some((payload_ty, payload))) => {
                    let mut cursor = 0usize;
                    ::core::result::Result::Ok(Self::#ident(
                        <#ty as #c::Lift>::lift_flat(cx, payload_ty, payload, &mut cursor)?,
                    ))
                }
            },
            None => quote! {
                (#index, ::core::option::Option::None) => ::core::result::Result::Ok(Self::#ident),
            },
        }
    });
    let memory = cases.iter().enumerate().map(|(index, case)| {
        let ident = case.ident;
        match case.ty {
            Some(ty) => quote! {
                (#index, payload, ::core::option::Option::Some(payload_ty)) => {
                    ::core::result::Result::Ok(Self::#ident(
                        <#ty as #c::Lift>::lift_memory(cx, payload_ty, payload)?,
                    ))
                }
            },
            None => quote! {
                (#index, _, ::core::option::Option::None) => ::core::result::Result::Ok(Self::#ident),
            },
        }
    });
    quote! {
        impl #impl_generics #c::Lift for #name #ty_generics #where_clause {
            #[doc(hidden)]
            fn lift_flat<D: 'static>(
                cx: &mut #c::__private::LiftContext<'_, '_, D>,
                ty: &#c::ValueType,
                slots: &[#c::__private::CoreVal],
                cursor: &mut usize,
            ) -> #c::Result<Self> {
                #c::__private::lift_case_flat(cx, ty, slots, cursor, |cx, case, payload| {
                    match (case, payload) {
                        #(#flat)*
                        _ => ::core::result::Result::Err(cx.mismatch(ty)),
                    }
                })
            }

            #[doc(hidden)]
            fn lift_memory<D: 'static>(
                cx: &mut #c::__private::LiftContext<'_, '_, D>,
                ty: &#c::ValueType,
                offset: usize,
            ) -> #c::Result<Self> {
                match #c::__private::lift_case_memory(cx, ty, offset)? {
                    #(#memory)*
                    _ => ::core::result::Result::Err(cx.mismatch(ty)),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The refusal the derive of `ComponentType` gives for `source`.
    fn refusal(source: &str) -> String {
        let input: DeriveInput = syn::parse_str(source).expect("the input parses");
        match expand(Trait::ComponentType, &input) {
            Ok(_) => panic!("the derive accepted {source}"),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn it_refuses_a_record_on_a_tuple_struct() {
        assert!(
            refusal("#[component(record)] struct Pair(u32, u32);")
                .contains("can only be derived for `struct`s with named fields")
        );
    }

    #[test]
    fn it_refuses_a_variant_with_no_cases() {
        assert!(
            refusal("#[component(variant)] enum Never {}").contains("with at least one variant")
        );
    }

    #[test]
    fn it_refuses_a_variant_with_a_case_of_two_payloads() {
        assert!(
            refusal("#[component(variant)] enum Shape { Pair(u32, u32) }")
                .contains("at most one unnamed field each")
        );
    }

    #[test]
    fn it_refuses_an_enum_without_a_repr() {
        assert!(
            refusal("#[component(enum)] enum Level { Info, Error }")
                .contains("missing required `#[repr(u8)]`")
        );
    }

    #[test]
    fn it_refuses_an_enum_whose_repr_does_not_match_its_count_of_cases() {
        assert!(
            refusal("#[component(enum)] #[repr(u32)] enum Level { Info, Error }")
                .contains("expected `repr(u8)`, found `repr(u32)`")
        );
    }

    #[test]
    fn it_refuses_an_enum_with_an_explicit_discriminant() {
        assert!(
            refusal("#[component(enum)] #[repr(u8)] enum Level { Info = 1, Error }")
                .contains("cannot have an explicit discriminant")
        );
    }

    #[test]
    fn it_refuses_a_type_with_no_style_attribute() {
        assert!(refusal("struct Point { x: u32 }").contains("missing `component` attribute"));
    }

    #[test]
    fn it_refuses_a_type_with_two_style_attributes() {
        assert!(
            refusal("#[component(record)] #[component(record)] struct Point { x: u32 }")
                .contains("duplicate `component` attribute")
        );
    }

    #[test]
    fn it_refuses_flags_and_names_the_flags_macro() {
        assert!(
            refusal("#[component(flags)] struct Permissions { read: bool }")
                .contains("use `wcmp::flags!` macro")
        );
    }

    #[test]
    fn it_refuses_wasmtime_crate_and_names_wcmp_crate() {
        assert!(
            refusal("#[component(record)] #[component(wasmtime_crate = wasmtime)] struct Point { x: u32 }")
                .contains("use `wcmp_crate = path`")
        );
    }
}
