// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The `with` option.

use std::cell::Cell;

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use wit_parser::WorldKey;

use crate::generator::Generator;
use crate::lookup::{LookupItem, lookup_keys};
use crate::names::ident;

/// One `with` entry, as the generated code names it: the alias the
/// root brings its path in as, and whether an item matched it.
pub struct WithName {
    /// The WIT name the entry maps.
    pub key: String,
    /// The alias at the root, as `__with_name0`.
    pub alias: syn::Ident,
    /// Where the entry was written.
    pub span: Span,
    /// Whether an item of the WIT matched the entry.
    pub used: Cell<bool>,
}

impl Generator<'_> {
    /// Bring each `with` path in at the root under an alias, so that a
    /// module at any depth names it from the root, as Wasmtime's macro
    /// does. The entries are sorted by their keys, so the aliases do
    /// not depend on the order they were written in.
    pub fn with_aliases(&mut self) -> TokenStream {
        let mut entries = self.config.options.with.iter().collect::<Vec<_>>();
        entries.sort_by(|a, b| a.key.cmp(&b.key));
        let mut tokens = TokenStream::new();
        for (index, entry) in entries.into_iter().enumerate() {
            let alias = format_ident!("__with_name{index}");
            let path = &entry.path;
            tokens.extend(quote! {
                #[doc(hidden)]
                pub use #path as #alias;
            });
            self.with_names.push(WithName {
                key: entry.key.clone(),
                alias,
                span: entry.span,
                used: Cell::new(false),
            });
        }
        tokens
    }

    /// The path from the root that `with` maps the interface `key` to,
    /// or its item `item`, by the most specific key that matches, with
    /// the projection from what that key maps to down to the item.
    pub fn lookup_replacement(
        &self,
        key: Option<&WorldKey>,
        item: Option<&str>,
    ) -> Option<TokenStream> {
        let keys = match key {
            Some(key) => lookup_keys(
                self.resolve,
                key,
                match item {
                    Some(item) => LookupItem::Name(item),
                    None => LookupItem::None,
                },
            ),
            // An item of the world is looked up by its name alone.
            None => vec![(item?.to_owned(), Vec::new())],
        };
        keys.into_iter().find_map(|(lookup, projection)| {
            let entry = self.with_names.iter().find(|entry| entry.key == lookup)?;
            entry.used.set(true);
            let alias = &entry.alias;
            let projection = projection.iter().rev().map(|segment| ident(segment));
            Some(quote!(#alias #(::#projection)*))
        })
    }

    /// Fail when a `with` entry matched no item of the WIT, naming it,
    /// as Wasmtime does.
    pub fn check_with_used(&self) -> syn::Result<()> {
        let unused = self
            .with_names
            .iter()
            .filter(|entry| !entry.used.get())
            .collect::<Vec<_>>();
        let Some(first) = unused.first() else {
            return Ok(());
        };
        let keys = unused
            .iter()
            .map(|entry| format!("{:?}", entry.key))
            .collect::<Vec<_>>()
            .join(", ");
        Err(syn::Error::new(
            first.span,
            format!("unused `with` mappings found: {keys} match no item of the WIT"),
        ))
    }
}
