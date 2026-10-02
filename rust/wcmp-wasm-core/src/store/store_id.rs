// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The identity of a store.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::internal::StoreIdInternal;

/// The identity of a store, unique in the process.
///
/// Every handle carries the identity of the store that owns its object, so
/// the engine can refuse a handle used with another store before a backend
/// sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StoreId {
    id: u64,
}

impl StoreIdInternal for StoreId {
    fn allocate() -> StoreId {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        StoreId {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
        }
    }
}
