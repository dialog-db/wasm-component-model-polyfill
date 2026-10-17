// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The `ownership` option.

use proc_macro2::Span;

/// `ownership`: how generated types hold what they hold.
#[derive(Clone, Copy, Debug, Default)]
pub enum Ownership {
    /// Every field owns its value, the default.
    #[default]
    Owning,
    /// The types an export takes borrow, with Wasmtime's
    /// `duplicate_if_necessary`. Parsed, and refused.
    Borrowing {
        /// Where the option was written.
        span: Span,
    },
}
