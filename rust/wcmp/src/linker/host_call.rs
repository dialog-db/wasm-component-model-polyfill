// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The host's view of one guest call into a registered host function.

use crate::internal::HostCallInternal;
use crate::store::StoreContext;

/// The context a registered host function runs against, scoped to
/// one call from the guest.
///
/// Two surfaces are reachable: the host data of the [`Store<T>`]
/// through [`Self::data`] and [`Self::data_mut`], which mirror
/// [`Store::data`] and [`Store::data_mut`]; and the borrow of the
/// store the call runs against, through [`Self::store`], which is the
/// value the polyfill itself blocks a call on and starts a host
/// `async` function's task from, and which the conversions of a
/// [`ResourceAny`](crate::ResourceAny) take.
///
/// The store surface is a [`StoreContext`], the polyfill's own view
/// of the store. The whole store sits behind that view — the
/// scheduler, the queues, the host tasks, the handle tables, and the
/// core store the runtime layer gave it — because of how a
/// trampoline is served: a suspended guest thread resumes outside
/// any poll of a driver, so the scheduler's state has to be
/// reachable from a trampoline that holds nothing but the context
/// the runtime layer handed it, and the polyfill's state therefore
/// rides in the core store's data. None of it is reachable from
/// here. What a [`StoreContext`] offers a host function is its host
/// data; the bookkeeping behind it is reached through a seam that
/// only the crate can name. The host data goes through:
///
/// ```rust
/// # use wcmp::{Engine, HostCall, Linker, Result};
/// let engine = Engine::with_backend(wcmp_wasm_core_wasmtime::Wasmtime::new().unwrap()).unwrap();
/// let mut linker: Linker<u32> = Linker::new(&engine);
/// linker
///     .root()
///     .func_wrap("f", |mut call: HostCall<'_, u32>, (): ()| -> Result<()> {
///         *call.store().data_mut() += 1;
///         Ok(())
///     });
/// ```
///
/// The core store behind it does not:
///
/// ```compile_fail
/// # use wcmp::{Engine, HostCall, Linker, Result};
/// let engine = Engine::with_backend(wcmp_wasm_core_wasmtime::Wasmtime::new().unwrap()).unwrap();
/// let mut linker: Linker<u32> = Linker::new(&engine);
/// linker
///     .root()
///     .func_wrap("f", |mut call: HostCall<'_, u32>, (): ()| -> Result<()> {
///         let _ = call.store().runtime_mut();
///         Ok(())
///     });
/// ```
///
/// [`Store<T>`]: crate::Store
/// [`Store::data`]: crate::Store::data
/// [`Store::data_mut`]: crate::Store::data_mut
pub struct HostCall<'a, T: 'static> {
    store: StoreContext<'a, T>,
}

impl<'a, T: 'static> HostCallInternal<'a, T> for HostCall<'a, T> {
    fn new(store: StoreContext<'a, T>) -> Self {
        HostCall { store }
    }
}

impl<'a, T: 'static> HostCall<'a, T> {
    /// The store's host data.
    pub fn data(&self) -> &T {
        self.store.data()
    }

    /// The store's host data, mutably.
    pub fn data_mut(&mut self) -> &mut T {
        self.store.data_mut()
    }

    /// The store this call runs against.
    ///
    /// A host function that only reads or writes the host data
    /// reaches it through [`Self::data`] and [`Self::data_mut`] and
    /// never needs this. What this hands back is the same borrow the
    /// polyfill blocks a call on and starts a host `async` function's
    /// task from: the runtime layer hands a trampoline the core
    /// store's context and nothing else, so this is the store as the
    /// call reaches it, built from that context alone, because
    /// everything the polyfill carries rides in the core store's
    /// data. The bookkeeping that rides there is the crate's own and
    /// is not reachable through this value.
    pub fn store(&mut self) -> &mut StoreContext<'a, T> {
        &mut self.store
    }

    /// The store this call runs against, by value, for the length of
    /// the call: what [`Access::new`](crate::Access::new) pairs with
    /// a projection of the host data.
    pub fn into_store(self) -> StoreContext<'a, T> {
        self.store
    }
}
