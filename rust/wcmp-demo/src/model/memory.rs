// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A storage in memory, for the tests.

use super::{Storage, TodoList};

/// A storage in memory, for the tests.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
pub struct Memory(std::sync::Mutex<TodoList>);

#[cfg(not(target_arch = "wasm32"))]
impl Storage for Memory {
    fn load(&self) -> impl Future<Output = Result<TodoList, String>> + Send {
        let list = self
            .0
            .lock()
            .map(|list| list.clone())
            .map_err(|error| error.to_string());
        async move { list }
    }

    fn save(&self, list: &TodoList) -> impl Future<Output = Result<(), String>> + Send {
        let saved = self
            .0
            .lock()
            .map(|mut kept| *kept = list.clone())
            .map_err(|error| error.to_string());
        async move { saved }
    }
}
