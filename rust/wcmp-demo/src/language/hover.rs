// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What a symbol under a position is.

/// What a symbol under a position is: its declaration, its type, and its
/// doc comment, each possibly empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hover {
    /// The declaration, such as `let count: i32`.
    pub label: String,
    /// The type.
    pub detail: String,
    /// The doc comment.
    pub doc: String,
}
