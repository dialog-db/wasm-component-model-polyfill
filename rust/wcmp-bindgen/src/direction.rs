// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Whether an interface is imported or exported.

/// Whether an interface is imported or exported.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// The world imports it.
    Import,
    /// The world exports it.
    Export,
}
