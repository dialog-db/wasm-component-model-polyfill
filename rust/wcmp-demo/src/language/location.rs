// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Where something is declared.

/// Where something is declared: a span of a file. Offsets count UTF-8
/// bytes, and lines and columns count from one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    /// The file.
    pub file: String,
    /// The byte offset where it starts.
    pub start: u32,
    /// Its length in bytes.
    pub length: u32,
    /// Its line.
    pub line: u32,
    /// Its column.
    pub column: u32,
}
