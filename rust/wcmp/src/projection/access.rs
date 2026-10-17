// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A short-lived view of the store and a projection of its host data.

use crate::store::StoreContext;

use super::has_data::HasData;
use super::has_self::HasSelf;

/// A context for the store, paired with a projection of its host
/// data, as Wasmtime's `Access` is.
///
/// A host function reaches the store through one for the length of
/// one call or one reach: [`Accessor::with`] hands its closure one,
/// and a synchronous host function builds one from the context of its
/// call with [`Access::new`] and [`HostCall::into_store`]. What it
/// gives:
///
/// - [`get`](Self::get), the view the projection reaches.
/// - [`data`](Self::data) and [`data_mut`](Self::data_mut), the whole
///   host data.
/// - [`store`](Self::store), the context for the store, which is what
///   the polyfill's own calls take, as [`HostCall::store`] gives it.
///   Wasmtime's `Access` is itself a store context through
///   `AsContextMut`; the polyfill has no such trait, so the context is
///   one call away.
///
/// [`Accessor::with`]: crate::Accessor::with
/// [`HostCall::into_store`]: crate::HostCall::into_store
/// [`HostCall::store`]: crate::HostCall::store
pub struct Access<'a, T: 'static, D: HasData + ?Sized = HasSelf<T>> {
    store: StoreContext<'a, T>,
    get_data: fn(&mut T) -> D::Data<'_>,
}

impl<'a, T: 'static, D: HasData + ?Sized> Access<'a, T, D> {
    /// Pair `store` with the projection `get_data`. The name is
    /// Wasmtime's.
    pub fn new(store: StoreContext<'a, T>, get_data: fn(&mut T) -> D::Data<'_>) -> Self {
        Self { store, get_data }
    }

    /// The store's host data.
    pub fn data(&self) -> &T {
        self.store.data()
    }

    /// The store's host data, mutably.
    pub fn data_mut(&mut self) -> &mut T {
        self.store.data_mut()
    }

    /// The view the projection reaches from the host data. The name is
    /// Wasmtime's.
    pub fn get(&mut self) -> D::Data<'_> {
        (self.get_data)(self.store.data_mut())
    }

    /// The projection. The name is Wasmtime's.
    pub fn getter(&self) -> fn(&mut T) -> D::Data<'_> {
        self.get_data
    }

    /// The context for the store.
    pub fn store(&mut self) -> &mut StoreContext<'a, T> {
        &mut self.store
    }
}
