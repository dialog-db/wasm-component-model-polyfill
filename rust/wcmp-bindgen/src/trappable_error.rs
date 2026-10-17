// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One entry of the `trappable_error_type` option.

use proc_macro2::Span;

/// One `trappable_error_type` entry: the WIT type, and the Rust type a
/// trappable function that returns it returns instead.
#[derive(Clone)]
pub struct TrappableError {
    /// The WIT path of the error type, as `wasi:io/streams.stream-error`.
    pub wit_path: String,
    /// The Rust type.
    pub rust_type: syn::Path,
    /// Where the entry was written.
    pub span: Span,
}
