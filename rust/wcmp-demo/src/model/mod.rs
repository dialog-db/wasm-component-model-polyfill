// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The todo model: the business logic of the todo list, in Rust.
//!
//! Route components import it as `demo:todo/model`. It trims titles,
//! refuses an empty one, assigns ids, filters, and counts. The list
//! itself lives in a [`Storage`]: IndexedDB in the service worker, and
//! memory in the tests. Each call reads the list, changes it, and writes
//! it back, so a model that the browser stops and starts again reads
//! the list it left.

mod counts;
mod filter;
#[cfg(not(target_arch = "wasm32"))]
mod memory;
mod model_error;
mod storage;
mod todo;
mod todo_list;

pub use counts::Counts;
pub use filter::Filter;
#[cfg(not(target_arch = "wasm32"))]
pub use memory::Memory;
pub use model_error::ModelError;
pub use storage::Storage;
pub use todo::Todo;
pub use todo_list::TodoList;
