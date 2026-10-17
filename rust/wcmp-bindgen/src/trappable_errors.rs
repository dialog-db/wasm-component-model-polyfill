// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The `trappable_error_type` option.

use heck::{ToSnakeCase, ToUpperCamelCase};
use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use wit_parser::{Function, InterfaceId, Type, TypeDefKind, TypeId, TypeOwner, WorldKey};

use crate::generator::Generator;
use crate::lookup::{LookupItem, lookup_keys};
use crate::names::{ident, resolve_type_definition_id};

/// What a trappable function that returns a mapped error returns
/// instead of its WIT `result`, as Wasmtime's
/// `special_case_trappable_error` finds it.
pub struct TrappableReturn {
    /// The `ok` type of the WIT `result`.
    pub ok: Option<Type>,
    /// The WIT error type the mapped Rust type converts to.
    pub error: TypeId,
    /// The Rust type the method returns as its error, from the module
    /// the method is written in.
    pub rust: TokenStream,
}

impl Generator<'_> {
    /// Find the WIT type of each `trappable_error_type` entry, and write
    /// the alias the generated code names it by at the root, as
    /// `_TrappableError0`. An entry whose WIT path names no type fails
    /// the compile, as in Wasmtime, and so does an entry for a type an
    /// earlier entry maps.
    pub fn resolve_trappable_errors(&mut self) -> syn::Result<TokenStream> {
        let mut aliases = TokenStream::new();
        for (index, entry) in self.config.options.trappable_error_type.iter().enumerate() {
            let alias = format_ident!("_TrappableError{index}");
            let found = self.resolve.interfaces.iter().find_map(|(id, interface)| {
                lookup_keys(
                    self.resolve,
                    &WorldKey::Interface(id),
                    LookupItem::InterfaceNoPop,
                )
                .into_iter()
                .find_map(|(key, _)| {
                    let suffix = entry.wit_path.strip_prefix(&key)?.strip_prefix('.')?;
                    interface.types.get(suffix).copied()
                })
            });
            let Some(ty) = found else {
                return Err(syn::Error::new(
                    entry.span,
                    format!(
                        "failed to locate a WIT error type corresponding to the \
                         `trappable_error_type` name `{}` provided",
                        entry.wit_path
                    ),
                ));
            };
            if self.trappable_errors.insert(ty, alias.clone()).is_some() {
                // Wasmtime asserts that no earlier entry mapped the type.
                return Err(syn::Error::new(
                    entry.span,
                    format!(
                        "the `trappable_error_type` name `{}` names a WIT type \
                         an earlier entry maps",
                        entry.wit_path
                    ),
                ));
            }
            let rust = &entry.rust_type;
            aliases.extend(quote!(type #alias = #rust;));
        }
        Ok(aliases)
    }

    /// The mapped error `func` returns, when it returns a WIT `result`
    /// whose error type a `trappable_error_type` entry maps.
    pub fn trappable_return(&self, scope: &[String], func: &Function) -> Option<TrappableReturn> {
        let Some(Type::Id(id)) = func.result else {
            return None;
        };
        let TypeDefKind::Result(result) = &self.resolve.types[id].kind else {
            return None;
        };
        let Some(Type::Id(error)) = result.err else {
            return None;
        };
        let error = resolve_type_definition_id(self.resolve, error);
        let alias = self.trappable_errors.get(&error)?;
        let root = Self::path_to_root(scope);
        Some(TrappableReturn {
            ok: result.ok,
            error,
            rust: quote!(#root #alias),
        })
    }

    /// The error types of the interface `id` that a
    /// `trappable_error_type` entry maps, by their WIT names: each gives
    /// the interface's `Host` a `convert_*` method.
    pub fn interface_error_types(&self, id: InterfaceId) -> Vec<(String, TypeId)> {
        self.resolve.interfaces[id]
            .types
            .iter()
            .filter(|(_, ty)| self.trappable_errors.contains_key(*ty))
            .map(|(name, ty)| (name.clone(), *ty))
            .collect()
    }

    /// The `convert_*` method an error type `name` gives a `Host`
    /// trait, without its body: the mapped Rust error to the WIT
    /// error, or a trap.
    pub fn convert_signature(&self, scope: &[String], name: &str, ty: TypeId) -> TokenStream {
        let method = format_ident!("convert_{}", name.to_snake_case());
        let wit = ident(&name.to_upper_camel_case());
        let root = Self::path_to_root(scope);
        let alias = &self.trappable_errors[&ty];
        let result = self.result_path();
        quote! {
            fn #method(&mut self, err: #root #alias) -> #result<#wit>
        }
    }

    /// The `convert_*` call of the error type `error` on `host`, a
    /// `&mut` of the projected view, from the module `scope`.
    pub fn convert_call(
        &self,
        scope: &[String],
        error: TypeId,
        host: TokenStream,
    ) -> syn::Result<TokenStream> {
        let def = &self.resolve.types[error];
        let name = def.name.as_deref().unwrap_or_default();
        let method = format_ident!("convert_{}", name.to_snake_case());
        let TypeOwner::Interface(owner) = def.owner else {
            return Err(syn::Error::new(
                Span::call_site(),
                format!("the trappable error type `{name}` is not defined in an interface"),
            ));
        };
        let host_trait = match self.path_to_interface(scope, owner) {
            Some(path) => quote!(#path::Host),
            None => quote!(Host),
        };
        Ok(quote!(#host_trait::#method(#host, e)))
    }

    /// The `Host` traits of other interfaces whose `convert_*` methods
    /// the functions of the interface `id` call, which its
    /// `add_to_linker` asks of the projected view as well.
    pub fn conversion_traits(&self, scope: &[String], id: InterfaceId) -> Vec<TokenStream> {
        let mut owners = Vec::new();
        for func in self.resolve.interfaces[id].functions.values() {
            let Some(trappable) = self.trappable_return(scope, func) else {
                continue;
            };
            if let TypeOwner::Interface(owner) = self.resolve.types[trappable.error].owner
                && owner != id
                && !owners.contains(&owner)
            {
                owners.push(owner);
            }
        }
        owners
            .into_iter()
            .filter_map(|owner| self.path_to_interface(scope, owner))
            .map(|path| quote!(#path::Host))
            .collect()
    }
}
