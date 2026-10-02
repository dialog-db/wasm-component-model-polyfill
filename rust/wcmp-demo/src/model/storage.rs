// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Where the todo list lives between calls.

use super::TodoList;

/// Where the todo list lives between calls.
///
/// Natively, where the host framework's tests run, a host function's
/// future must be `Send`, so a storage's futures are too. The browser
/// runs on one thread, and IndexedDB's futures are not `Send`.
#[cfg(not(target_arch = "wasm32"))]
pub trait Storage: Send + Sync {
    /// The list as last written, or an empty one.
    fn load(&self) -> impl Future<Output = Result<TodoList, String>> + Send;
    /// Write `list` in place of the last one.
    fn save(&self, list: &TodoList) -> impl Future<Output = Result<(), String>> + Send;
}

/// Where the todo list lives between calls.
#[cfg(target_arch = "wasm32")]
pub trait Storage {
    /// The list as last written, or an empty one.
    fn load(&self) -> impl Future<Output = Result<TodoList, String>>;
    /// Write `list` in place of the last one.
    fn save(&self, list: &TodoList) -> impl Future<Output = Result<(), String>>;
}
