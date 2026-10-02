// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Why the model refused a change.

/// Why the model refused a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelError {
    /// No todo has the id.
    NotFound,
    /// The title is empty once trimmed.
    EmptyTitle,
}
