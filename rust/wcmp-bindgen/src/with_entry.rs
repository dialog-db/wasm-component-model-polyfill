// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One entry of the `with` option.

use proc_macro2::Span;

/// One `with` entry: the WIT name, and the Rust path it maps to.
#[derive(Clone)]
pub struct WithEntry {
    /// The WIT name the entry maps: an interface, a package, a
    /// namespace, a version of a package, or one type.
    pub key: String,
    /// The Rust path, with no generic parameters.
    pub path: syn::Path,
    /// Where the entry was written.
    pub span: Span,
}
