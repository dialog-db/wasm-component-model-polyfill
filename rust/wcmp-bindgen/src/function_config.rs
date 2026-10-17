// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The rules of the `imports` or the `exports` option.

use std::cell::Cell;

use proc_macro2::Span;
use wit_parser::{Function, FunctionKind, Resolve, WorldKey};

use crate::function_flags::FunctionFlags;
use crate::lookup::{LookupItem, lookup_keys};

/// One rule: the name it matches, the flags it gives, and whether a
/// function matched it.
#[derive(Debug)]
struct Rule {
    filter: String,
    flags: FunctionFlags,
    span: Span,
    used: Cell<bool>,
}

/// The rules of the `imports` or the `exports` option, with Wasmtime's
/// matching (`crates/wit-bindgen/src/config.rs` at `v49.0.0-rc.1`).
///
/// The first rule that matches a function applies, and `default`
/// applies when none does. A rule names a function by its WIT path, as
/// `wasi:io/streams.[method]input-stream.read` or `my-func`, and a
/// resource's destructor as `[drop]r`. A rule without `exact` matches
/// a name with or without its version, and an interface's name or its
/// package's for every function in it, from the most specific name to
/// the least. A rule that matches no function fails the compile.
#[derive(Debug, Default)]
pub struct FunctionConfig {
    rules: Vec<Rule>,
    default: FunctionFlags,
    default_span: Option<Span>,
}

impl FunctionConfig {
    /// Add a rule for the functions `filter` names. Only the first
    /// matching rule applies, so the order rules are added in matters.
    pub fn push(&mut self, filter: String, flags: FunctionFlags, span: Span) {
        self.rules.push(Rule {
            filter,
            flags,
            span,
            used: Cell::new(false),
        });
    }

    /// Set the flags of every function no rule names.
    pub fn set_default(&mut self, flags: FunctionFlags, span: Span) {
        self.default = flags;
        self.default_span = Some(span);
    }

    /// The flags of `default`.
    pub fn default_flags(&self) -> FunctionFlags {
        self.default
    }

    /// Every rule, with the flags it gives and where it was written,
    /// and `default` last when it was written.
    pub fn rules(&self) -> impl Iterator<Item = (&str, FunctionFlags, Span)> {
        self.rules
            .iter()
            .map(|rule| (rule.filter.as_str(), rule.flags, rule.span))
            .chain(
                self.default_span
                    .map(|span| ("default", self.default, span)),
            )
    }

    /// The flags of `func`, which `key` names the interface of, or
    /// `None` for a function of the world.
    ///
    /// An async function always gets the `async` and `store` flags, as
    /// in Wasmtime, whatever the rules say.
    pub fn flags(
        &self,
        resolve: &Resolve,
        key: Option<&WorldKey>,
        func: &Function,
    ) -> FunctionFlags {
        let mut flags = self.lookup(resolve, key, &func.name);
        match func.kind {
            FunctionKind::AsyncFreestanding
            | FunctionKind::AsyncMethod(_)
            | FunctionKind::AsyncStatic(_) => flags |= FunctionFlags::ASYNC | FunctionFlags::STORE,
            FunctionKind::Freestanding
            | FunctionKind::Method(_)
            | FunctionKind::Static(_)
            | FunctionKind::Constructor(_) => {}
        }
        flags
    }

    /// The flags of the destructor of the resource `resource`.
    pub fn resource_drop_flags(
        &self,
        resolve: &Resolve,
        key: Option<&WorldKey>,
        resource: &str,
    ) -> FunctionFlags {
        self.lookup(resolve, key, &format!("[drop]{resource}"))
    }

    /// The flags of the first rule that matches `name`, or `default`.
    fn lookup(&self, resolve: &Resolve, key: Option<&WorldKey>, name: &str) -> FunctionFlags {
        let apply = |name: &str, exact: bool| -> Option<FunctionFlags> {
            let rule = self.rules.iter().find(|rule| {
                rule.filter == name && (exact || !rule.flags.contains(FunctionFlags::EXACT))
            })?;
            rule.used.set(true);
            Some(rule.flags)
        };
        let found = match key {
            Some(key) => lookup_keys(resolve, key, LookupItem::Name(name))
                .into_iter()
                .find_map(|(lookup, projection)| apply(&lookup, projection.is_empty())),
            None => apply(name, true),
        };
        found.unwrap_or(self.default)
    }

    /// Fail when a rule matched no function, naming the rule, as
    /// Wasmtime does. `option` is `imports` or `exports`.
    pub fn check_all_used(&self, option: &str) -> syn::Result<()> {
        let mut unused = self.rules.iter().filter(|rule| !rule.used.get());
        let Some(first) = unused.next() else {
            return Ok(());
        };
        let names = core::iter::once(first)
            .chain(unused)
            .map(|rule| format!("{:?}", rule.filter))
            .collect::<Vec<_>>()
            .join(", ");
        Err(syn::Error::new(
            first.span,
            format!("unused `{option}` rules found: {names} match no function of the world"),
        ))
    }
}
