// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! How grave a diagnostic is.

/// How grave a diagnostic is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// The program does not compile.
    Error,
    /// The program compiles, but something in it is likely wrong.
    Warning,
    /// A note.
    Information,
}
