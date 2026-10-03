// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A diagnostic of the compiler, for an editor.

use super::Severity;

/// A diagnostic of the compiler, for an editor: where it is, how grave,
/// and what it says. Offsets count UTF-8 bytes from the start of the
/// file, and lines and columns count from one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// The file it is in.
    pub file: String,
    /// The byte offset where it starts.
    pub start: u32,
    /// Its length in bytes.
    pub length: u32,
    /// Its line.
    pub line: u32,
    /// Its column.
    pub column: u32,
    /// How grave it is.
    pub severity: Severity,
    /// What it says.
    pub message: String,
}
