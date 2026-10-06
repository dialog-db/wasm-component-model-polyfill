// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The owner of guest state.

use core::fmt;
use core::marker::PhantomData;

use crate::contract::{BackendStore, MaybeSend};
use crate::engine::Engine;
use crate::error::Result;
use crate::internal::{
    EngineInternal, StoreContextInternal, StoreContextMutInternal, StoreDataInternal,
};
use crate::store::{AsContext, AsContextMut, StoreContext, StoreContextMut, StoreData};

/// The owner of every instance and every object an instance makes, and of
/// the host's data, a `T`.
///
/// Every handle names one store, and means nothing to another. When a store
/// drops, everything it owns drops with it, including each call that waits
/// in it for a resumption.
pub struct Store<T> {
    inner: Box<dyn BackendStore>,
    data: PhantomData<fn() -> T>,
}

impl<T: MaybeSend + 'static> Store<T> {
    /// A store of `engine` that owns `data`.
    ///
    /// Natively `T` must be `Send`, as it must be for Wasmtime's
    /// asynchronous API, because every compile and instantiation is
    /// asynchronous.
    pub fn new(engine: &Engine, data: T) -> Result<Self> {
        let data = StoreData::new(engine.clone(), data);
        let inner = engine.backend().new_store(data)?;
        Ok(Self {
            inner,
            data: PhantomData,
        })
    }
}

impl<T: 'static> Store<T> {
    /// The host's data in the store.
    ///
    /// # Panics
    ///
    /// Where the backend refuses the store because another reference to
    /// it lives. The browser backend refuses it to a host function that
    /// reaches its store past its caller, for example through a global,
    /// while a guest call that runs on its own calls that host function:
    /// the resumed stack of a resumable call, or the start function of an
    /// instantiation, after the host forgot the future that waited for it.
    /// The caller is the one way to the store then. Every method of the
    /// store and its handles that returns a [`Result`] returns
    /// [`Error::Backend`](crate::Error::Backend) there, and only this
    /// method and [`data_mut`](Store::data_mut), which cannot fail, panic.
    pub fn data(&self) -> &T {
        self.inner.data().user::<T>()
    }

    /// The host's data in the store, mutably.
    ///
    /// # Panics
    ///
    /// Where [`data`](Store::data) does.
    pub fn data_mut(&mut self) -> &mut T {
        self.inner.data_mut().user_mut::<T>()
    }

    /// The host's data in the store, mutably, or `None` where
    /// [`data_mut`](Store::data_mut) would panic: inside a host function
    /// that a guest call running on its own called, after the host forgot
    /// the future that waited for that call. A caller that must not
    /// panic, such as the `Drop` of a store that wraps this one, reaches
    /// the data through this.
    pub fn try_data_mut(&mut self) -> Option<&mut T> {
        self.inner.try_data_mut().map(|data| data.user_mut::<T>())
    }

    /// The engine of the store.
    pub fn engine(&self) -> &Engine {
        self.inner.engine()
    }
}

impl<T: 'static> AsContext for Store<T> {
    type Data = T;

    fn as_context(&self) -> StoreContext<'_, T> {
        StoreContext::from_backend(&*self.inner)
    }
}

impl<T: 'static> AsContextMut for Store<T> {
    fn as_context_mut(&mut self) -> StoreContextMut<'_, T> {
        StoreContextMut::from_backend(&mut *self.inner)
    }
}

impl<T> fmt::Debug for Store<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Store")
            .field("id", &self.inner.id())
            .finish_non_exhaustive()
    }
}
