// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A process-unique identity for one store.

use std::sync::atomic::{AtomicU64, Ordering};

/// A process-unique identity for one [`Store`].
///
/// Every store mints a fresh id at construction. An [`Instance`]
/// records the id of the store it was created in, and a function
/// handle compares that id with the store it is called with. The
/// wrapped integer is opaque and not exposed.
///
/// [`Store`]: crate::Store
/// [`Instance`]: crate::Instance
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct StoreId(u64);

impl StoreId {
    /// Mint an identity no other store in this process carries.
    /// Workspace-internal; not re-exported by `lib.rs`.
    pub fn fresh() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}
