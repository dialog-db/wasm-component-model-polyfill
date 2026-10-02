// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One todo.

/// One todo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Todo {
    /// The todo's id, which the model assigns.
    pub id: String,
    /// The title, trimmed and never empty.
    pub title: String,
    /// Whether the todo is done.
    pub completed: bool,
}
