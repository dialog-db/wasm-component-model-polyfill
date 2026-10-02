// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The store, and the contexts that reach it.

mod as_context;
mod as_context_mut;
mod caller;
#[allow(clippy::module_inception)]
mod store;
mod store_context;
mod store_context_mut;
mod store_data;
mod store_id;

pub use as_context::AsContext;
pub use as_context_mut::AsContextMut;
pub use caller::Caller;
pub use store::Store;
pub use store_context::StoreContext;
pub use store_context_mut::StoreContextMut;
pub use store_data::StoreData;
pub use store_id::StoreId;
