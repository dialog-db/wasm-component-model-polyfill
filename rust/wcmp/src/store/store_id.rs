// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A process-unique identity for one store.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::internal::StoreIdInternal;

/// A process-unique identity for one [`Store`].
///
/// Every store mints a fresh id at construction. An [`Instance`]
/// records the id of the store it was created in, and a function
/// handle compares that id with the store it is called with. The
/// wrapped integer is opaque: nothing reads it, and only `Debug`
/// prints it.
///
/// A host meets one only as the parameter of
/// [`ComponentValue::check_store`], and can only hand it on: a type of
/// the host's own that holds an [`ErrorContext`] passes it to that
/// field's `check_store`, so a call refuses another store's error
/// context before it starts. No host makes one.
///
/// ```rust
/// use wcmp::StoreId;
/// let _: Option<StoreId> = None;
/// ```
///
/// Minting one is the crate's own:
///
/// ```compile_fail
/// let _ = wcmp::StoreId::fresh();
/// ```
///
/// [`Store`]: crate::Store
/// [`Instance`]: crate::Instance
/// [`ComponentValue::check_store`]: crate::ComponentValue::check_store
/// [`ErrorContext`]: crate::ErrorContext
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct StoreId(u64);

impl StoreIdInternal for StoreId {
    fn fresh() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}
