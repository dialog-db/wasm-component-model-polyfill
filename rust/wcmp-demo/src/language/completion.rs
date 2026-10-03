// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A completion at a position.

/// A completion at a position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    /// The text it inserts.
    pub label: String,
    /// The Language Server Protocol's kind of completion: 2 a method,
    /// 3 a function, 6 a variable, 7 a class, and 14 a keyword.
    pub kind: u32,
    /// Its type or signature.
    pub detail: String,
    /// Its doc comment.
    pub doc: String,
}
