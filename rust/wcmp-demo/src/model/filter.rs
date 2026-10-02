// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Which todos a query returns.

/// Which todos a query returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    /// Every todo.
    All,
    /// Todos not done.
    Active,
    /// Todos done.
    Completed,
}
