// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Read access to a store.

use core::fmt;
use core::marker::PhantomData;

use crate::contract::BackendStore;
use crate::engine::Engine;
use crate::internal::{StoreContextInternal, StoreDataInternal};
use crate::store::AsContext;

/// Read access to a store whose host data is a `T`.
pub struct StoreContext<'a, T> {
    store: &'a dyn BackendStore,
    data: PhantomData<fn() -> T>,
}

impl<'a, T: 'static> StoreContext<'a, T> {
    /// The host's data in the store.
    ///
    /// # Panics
    ///
    /// Where [`Store::data`](crate::Store::data) does, for a context made
    /// from a store.
    pub fn data(&self) -> &'a T {
        self.store.data().user::<T>()
    }

    /// The engine of the store.
    pub fn engine(&self) -> &'a Engine {
        self.store.engine()
    }
}

impl<'a, T> StoreContextInternal<'a> for StoreContext<'a, T> {
    fn from_backend(store: &'a dyn BackendStore) -> Self {
        Self {
            store,
            data: PhantomData,
        }
    }

    fn backend(&self) -> &'a dyn BackendStore {
        self.store
    }
}

impl<T: 'static> AsContext for StoreContext<'_, T> {
    type Data = T;

    fn as_context(&self) -> StoreContext<'_, T> {
        StoreContext::from_backend(self.store)
    }
}

impl<T> fmt::Debug for StoreContext<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoreContext")
            .field("id", &self.store.id())
            .finish_non_exhaustive()
    }
}
