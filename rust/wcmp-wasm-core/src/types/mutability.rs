// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Whether a global can change.

/// Whether a global can be set after it is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mutability {
    /// The global holds one value for its life.
    Const,
    /// The global can be set.
    Var,
}
