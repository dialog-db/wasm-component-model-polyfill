// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What Wasmtime's wast runner reaches in a store and no embedder
//! does.
//!
//! Wasmtime's runner registers a `wasmtime` instance beside the
//! spectest, and one of its items, `set-max-table-capacity`, lowers
//! the cap on the store's live records from inside a guest. The
//! polyfill's conformance harness registers the same item. The
//! harness is an integration test, so it sees the crate only through
//! its public surface, and the cap is set only through the store's
//! internal API.
//!
//! This module bridges the two, and only for the harness. It is
//! compiled only under the `wast-runner` feature, which the crate
//! turns on for its own tests alone, by a dev-dependency on itself.
//! A build without the feature has no item from here, so the public
//! API of the crate carries nothing that changes the cap.

use crate::error::Result;
use crate::store::{StoreContext, StoreContextInternalExt};

/// Set the most records `store` holds live before a new one fails,
/// as Wasmtime's runner sets the capacity of the store's concurrent
/// table. A cap below the records live now removes none of them;
/// only the records created after it fail.
pub fn set_max_table_capacity<T: 'static>(
    store: &mut StoreContext<'_, T>,
    capacity: u32,
) -> Result<()> {
    store.internal().set_max_records(capacity as usize)
}
