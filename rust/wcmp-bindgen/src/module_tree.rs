// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The tree of modules the macro writes.

use std::collections::BTreeMap;

use proc_macro2::TokenStream;
use quote::quote;

use crate::names::ident;

/// The tree of modules the macro writes, by name, each with the items
/// it holds.
///
/// The interface `example:app/log` is the module `example::app::log`,
/// and an exported interface sits under `exports`. Two interfaces of a
/// package share the package's module, so the tree merges the paths
/// before it writes them.
#[derive(Default)]
pub struct ModuleTree {
    items: TokenStream,
    children: BTreeMap<String, ModuleTree>,
}

impl ModuleTree {
    /// Add `items` to the module at `path` from the root.
    pub fn insert(&mut self, path: &[String], items: TokenStream) {
        match path.split_first() {
            None => self.items.extend(items),
            Some((first, rest)) => self
                .children
                .entry(first.clone())
                .or_default()
                .insert(rest, items),
        }
    }

    /// The modules, as Rust items at the root.
    pub fn into_tokens(self) -> TokenStream {
        let mut tokens = self.items;
        for (name, child) in self.children {
            let name = ident(&name);
            let child = child.into_tokens();
            tokens.extend(quote! {
                #[allow(clippy::all, unused_imports, dead_code)]
                pub mod #name {
                    #child
                }
            });
        }
        tokens
    }
}
