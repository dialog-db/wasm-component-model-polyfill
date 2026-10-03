// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A token of Zena source.

use super::Kind;

/// A token of Zena source: its byte range and its kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    /// The byte offset where the token starts.
    pub start: usize,
    /// The byte offset just past the token.
    pub end: usize,
    /// What the token is.
    pub kind: Kind,
}
