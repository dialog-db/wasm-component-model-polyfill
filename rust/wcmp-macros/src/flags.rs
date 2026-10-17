// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The `flags!` macro, which declares a flags type with Wasmtime's
//! syntax and API (`crates/component-macro/src/component.rs`,
//! `expand_flags`, at `v49.0.0-rc.1`).
//!
//! The type holds its flags as the words the Canonical ABI lays them
//! out in: one `u8` for at most 8 flags, one `u16` for at most 16, and
//! one `u32` for each 32 beyond that. The code it writes names `::wcmp`,
//! as Wasmtime's names `wasmtime`. An outer `#[component(wcmp_crate =
//! path)]` before the type's name moves it, which Wasmtime's `flags!`
//! does not take: `wcmp::bindgen!` writes it for its `wcmp_crate`
//! option.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::{Result, Token, braced};

use super::component::find_rename;

/// One flag: its Rust name, and its WIT name when it renames itself.
struct Flag {
    rename: Option<String>,
    name: syn::Ident,
}

impl Parse for Flag {
    fn parse(input: ParseStream) -> Result<Self> {
        let attributes = syn::Attribute::parse_outer(input)?;
        let rename = find_rename(&attributes)?.map(|literal| literal.value());
        input.parse::<Token![const]>()?;
        let name = input.parse::<syn::Ident>()?;
        Ok(Self { rename, name })
    }
}

/// The input of `flags!`: the crate path, the type's name, and its
/// flags.
pub struct Flags {
    docs: Vec<syn::Attribute>,
    crate_path: Option<syn::Path>,
    name: syn::Ident,
    flags: Vec<Flag>,
}

impl Parse for Flags {
    fn parse(input: ParseStream) -> Result<Self> {
        let mut docs = Vec::new();
        let mut crate_path = None;
        for attribute in syn::Attribute::parse_outer(input)? {
            if attribute.path().is_ident("doc") {
                docs.push(attribute);
            } else if attribute.path().is_ident("component") {
                crate_path = Some(super::component::parse_crate_path(&attribute)?);
            } else {
                return Err(syn::Error::new_spanned(
                    attribute,
                    "only documentation and `#[component(wcmp_crate = path)]` may precede \
                     the type's name",
                ));
            }
        }
        let name = input.parse::<syn::Ident>()?;
        let content;
        braced!(content in input);
        let flags = content
            .parse_terminated(Flag::parse, Token![;])?
            .into_iter()
            .collect();
        Ok(Self {
            docs,
            crate_path,
            name,
            flags,
        })
    }
}

/// How a flags type of `count` flags stores them: the Rust type of one
/// word, its width in bits, and the count of words.
fn words(count: usize) -> (TokenStream, usize, usize) {
    if count == 0 {
        (quote!(u8), 8, 0)
    } else if count <= 8 {
        (quote!(u8), 8, 1)
    } else if count <= 16 {
        (quote!(u16), 16, 1)
    } else {
        (quote!(u32), 32, count.div_ceil(32))
    }
}

/// Expand `flags!`.
pub fn expand(flags: &Flags) -> Result<TokenStream> {
    let c = match &flags.crate_path {
        Some(path) => quote!(#path),
        None => quote!(::wcmp),
    };
    let name = &flags.name;
    let docs = &flags.docs;
    let count = flags.flags.len();
    let (word, width, word_count) = words(count);
    let fields: Vec<_> = (0..word_count)
        .map(|index| format_ident!("__inner{}", index))
        .collect();

    // The bits of each word that hold a flag. Padding bits take no part
    // in equality, as Wasmtime masks them, and `!` leaves them clear, so
    // no value carries one into a guest: the Canonical ABI's
    // `pack_flags_into_int` packs the labelled bits alone.
    let masks: Vec<_> = (0..word_count)
        .map(|index| {
            let used = (count - index * width).min(width);
            let mask: u64 = if used == 64 {
                u64::MAX
            } else {
                (1u64 << used) - 1
            };
            quote!((#mask as #word))
        })
        .collect();
    let eq = fields
        .iter()
        .zip(masks.iter())
        .map(|(field, mask)| quote!((self.#field & #mask) == (rhs.#field & #mask)));
    let constants = flags.flags.iter().enumerate().map(|(index, flag)| {
        let flag_name = &flag.name;
        let inits = fields.iter().enumerate().map(|(word_index, field)| {
            if index / width == word_index {
                let bit = index % width;
                quote!(#field: (1 as #word) << #bit)
            } else {
                quote!(#field: 0)
            }
        });
        quote!(pub const #flag_name: Self = Self { #(#inits),* };)
    });
    let rust_names = flags.flags.iter().map(|flag| flag.name.to_string());
    let wit_names = flags
        .flags
        .iter()
        .map(|flag| flag.rename.clone().unwrap_or_else(|| flag.name.to_string()));
    let as_array = fields.iter().map(|field| quote!(self.#field as u32));
    let ops = |method: TokenStream| {
        let entries = fields
            .iter()
            .map(|field| quote!(#field: self.#field.#method(rhs.#field)));
        quote!(Self { #(#entries),* })
    };
    let assign_ops = |method: TokenStream| {
        let entries = fields
            .iter()
            .map(|field| quote!(self.#field.#method(rhs.#field);));
        quote!(#(#entries)*)
    };
    let bitor = ops(quote!(bitor));
    let bitand = ops(quote!(bitand));
    let bitxor = ops(quote!(bitxor));
    let bitor_assign = assign_ops(quote!(bitor_assign));
    let bitand_assign = assign_ops(quote!(bitand_assign));
    let bitxor_assign = assign_ops(quote!(bitxor_assign));
    let not = {
        let entries = fields
            .iter()
            .zip(masks.iter())
            .map(|(field, mask)| quote!(#field: !self.#field & #mask));
        quote!(Self { #(#entries),* })
    };
    let word_size = width / 8;
    let field_decls = fields
        .iter()
        .map(|field| quote!(#[doc(hidden)] #field: #word,));

    Ok(quote! {
        #(#docs)*
        #[derive(Copy, Clone, Default)]
        pub struct #name { #(#field_decls)* }

        impl #name {
            #(#constants)*

            /// The flags as the `u32` words the Canonical ABI lays them
            /// out in.
            pub fn as_array(&self) -> [u32; #word_count] {
                [#(#as_array),*]
            }

            /// No flag set.
            pub fn empty() -> Self {
                Self::default()
            }

            /// Every flag set.
            pub fn all() -> Self {
                !Self::default()
            }

            /// Whether every flag of `other` is set.
            pub fn contains(&self, other: Self) -> bool {
                *self & other == other
            }

            /// Whether a flag of `other` is set.
            pub fn intersects(&self, other: Self) -> bool {
                *self & other != Self::empty()
            }
        }

        impl ::core::cmp::PartialEq for #name {
            fn eq(&self, rhs: &#name) -> bool {
                true #(&& #eq)*
            }
        }

        impl ::core::cmp::Eq for #name {}

        impl ::core::fmt::Debug for #name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                #c::__private::format_flags(&self.as_array(), &[#(#rust_names),*], f)
            }
        }

        impl ::core::ops::BitOr for #name {
            type Output = #name;
            #[allow(unused_imports)]
            fn bitor(self, rhs: #name) -> #name {
                use ::core::ops::BitOr;
                #bitor
            }
        }

        impl ::core::ops::BitOrAssign for #name {
            #[allow(unused_imports, unused_variables)]
            fn bitor_assign(&mut self, rhs: #name) {
                use ::core::ops::BitOrAssign;
                #bitor_assign
            }
        }

        impl ::core::ops::BitAnd for #name {
            type Output = #name;
            #[allow(unused_imports)]
            fn bitand(self, rhs: #name) -> #name {
                use ::core::ops::BitAnd;
                #bitand
            }
        }

        impl ::core::ops::BitAndAssign for #name {
            #[allow(unused_imports, unused_variables)]
            fn bitand_assign(&mut self, rhs: #name) {
                use ::core::ops::BitAndAssign;
                #bitand_assign
            }
        }

        impl ::core::ops::BitXor for #name {
            type Output = #name;
            #[allow(unused_imports)]
            fn bitxor(self, rhs: #name) -> #name {
                use ::core::ops::BitXor;
                #bitxor
            }
        }

        impl ::core::ops::BitXorAssign for #name {
            #[allow(unused_imports, unused_variables)]
            fn bitxor_assign(&mut self, rhs: #name) {
                use ::core::ops::BitXorAssign;
                #bitxor_assign
            }
        }

        impl ::core::ops::Not for #name {
            type Output = #name;
            fn not(self) -> #name {
                #not
            }
        }

        impl #c::ComponentType for #name {
            #[doc(hidden)]
            const ABI: #c::__private::CanonicalAbiInfo =
                #c::__private::CanonicalAbiInfo::flags(#count);

            #[doc(hidden)]
            fn typecheck(
                ty: &#c::ValueType,
                _types: &#c::__private::TypeChecker<'_>,
            ) -> ::core::result::Result<(), #c::__private::TypeCheckError> {
                #c::__private::typecheck_flags::<Self>(ty, &[#(#wit_names),*])
            }
        }

        impl #c::Lower for #name {
            #[doc(hidden)]
            #[allow(unused_variables)]
            fn lower_flat<D: 'static>(
                &self,
                cx: &mut #c::__private::LowerContext<'_, '_, D>,
                ty: &#c::ValueType,
                out: &mut ::std::vec::Vec<#c::__private::CoreVal>,
            ) -> #c::Result<()> {
                #(#c::Lower::lower_flat(&self.#fields, cx, ty, out)?;)*
                ::core::result::Result::Ok(())
            }

            #[doc(hidden)]
            #[allow(unused_variables, unused_mut)]
            fn lower_memory<D: 'static>(
                &self,
                cx: &mut #c::__private::LowerContext<'_, '_, D>,
                ty: &#c::ValueType,
                mut offset: usize,
            ) -> #c::Result<()> {
                #(
                    #c::Lower::lower_memory(&self.#fields, cx, ty, offset)?;
                    offset += #word_size;
                )*
                ::core::result::Result::Ok(())
            }
        }

        impl #c::Lift for #name {
            #[doc(hidden)]
            #[allow(unused_variables)]
            fn lift_flat<D: 'static>(
                cx: &mut #c::__private::LiftContext<'_, '_, D>,
                ty: &#c::ValueType,
                slots: &[#c::__private::CoreVal],
                cursor: &mut usize,
            ) -> #c::Result<Self> {
                // A guest's padding bits are not flags, as the
                // Canonical ABI's `unpack_flags_from_int` reads them.
                ::core::result::Result::Ok(Self {
                    #(#fields: <#word as #c::Lift>::lift_flat(cx, ty, slots, cursor)?,)*
                } & Self::all())
            }

            #[doc(hidden)]
            #[allow(unused_variables, unused_mut)]
            fn lift_memory<D: 'static>(
                cx: &mut #c::__private::LiftContext<'_, '_, D>,
                ty: &#c::ValueType,
                mut offset: usize,
            ) -> #c::Result<Self> {
                ::core::result::Result::Ok(Self {
                    #(#fields: {
                        let word = <#word as #c::Lift>::lift_memory(cx, ty, offset)?;
                        offset += #word_size;
                        word
                    },)*
                } & Self::all())
            }
        }
    })
}
