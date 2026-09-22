//! The workspace-internal face of [`Store`].
//!
//! [`Store`] is re-exported by `lib.rs`, so every `pub` item on it
//! is public API even when the type it hands back cannot be named
//! outside the crate: a method is reachable by method syntax
//! whatever its return type is. The entries a driver, an
//! instantiation, and the crate's own tests need therefore live
//! here instead, on a wrapper the crate builds over a borrow of the
//! store. The wrapper is never re-exported, and no `pub` method on
//! [`Store`] returns one, so safe host code cannot reach the
//! scheduler, the handle tables, or the runtime-layer store through
//! a store it owns.
//!
//! Each entry keeps the signature it had on the store. The wrapper
//! holds the borrow, so a borrow an entry returns is tied to that
//! borrow exactly as it was tied to `&mut self` before; the entries
//! take `self` by value for that reason.
//!
//! Two entries of the public API — [`CoreInstance::get_export`] and
//! [`CoreExtern::ty`] — read the runtime-layer store through a
//! shared borrow of the store the caller owns. [`StoreRefInternal`]
//! is the same wrapper over that shared borrow, carrying the
//! entries that need no mutable access.
//!
//! [`CoreInstance::get_export`]: crate::CoreInstance::get_export
//! [`CoreExtern::ty`]: crate::CoreExtern::ty

use core::task::Waker;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::backend::Backend;
use crate::concurrency::{Outcome, Scheduler};
use crate::error::Result;
use crate::resource::HandleTables;

use super::super::store_context::StoreContext;
use super::super::store_data::StoreData;
use super::super::store_id::StoreId;
use super::Store;

/// The workspace-internal entries of a [`Store`], over a mutable
/// borrow of one.
pub struct StoreInternal<'a, T: 'static> {
    store: &'a mut Store<T>,
}

impl<'a, T: 'static> StoreInternal<'a, T> {
    /// The internal face of `store`.
    pub fn of(store: &'a mut Store<T>) -> Self {
        Self { store }
    }

    /// The store as a turn, an item, or a trampoline reaches it: a
    /// borrow of the core store, which carries everything else the
    /// store holds.
    pub fn context(self) -> StoreContext<'a, T> {
        self.store.context()
    }

    /// The store's process-unique identity.
    pub fn id(self) -> StoreId {
        self.store.id()
    }

    /// The store's handle tables.
    pub fn tables(self) -> &'a Arc<Mutex<HandleTables>> {
        self.store.tables()
    }

    /// Clone the handle for the per-store handle-tables ledger.
    pub fn tables_handle(self) -> Arc<Mutex<HandleTables>> {
        self.store.tables_handle()
    }

    /// Lock the store's handle tables and record state.
    pub fn lock_tables(self) -> Result<MutexGuard<'a, HandleTables>> {
        self.store.lock_tables()
    }

    /// The store's cooperative scheduler.
    pub fn scheduler(self) -> &'a Scheduler<T> {
        self.store.scheduler()
    }

    /// The store's cooperative scheduler, mutably.
    pub fn scheduler_mut(self) -> &'a mut Scheduler<T> {
        self.store.scheduler_mut()
    }

    /// Whether a turn of this store is running.
    pub fn turn_in_flight(self) -> bool {
        self.store.turn_in_flight()
    }

    /// Run one turn of the store's scheduler.
    pub fn turn(self, waker: &Waker) -> Result<Outcome> {
        self.store.turn(waker)
    }

    /// Mutably borrow the wrapped runtime-layer store.
    pub fn inner_mut(self) -> &'a mut wasm_runtime_layer::Store<StoreData<T>, Backend> {
        self.store.inner_mut()
    }
}

/// The workspace-internal entries of a [`Store`] that need no
/// mutable access, over a shared borrow of one.
pub struct StoreRefInternal<'a, T: 'static> {
    store: &'a Store<T>,
}

impl<'a, T: 'static> StoreRefInternal<'a, T> {
    /// The internal face of `store`.
    pub fn of(store: &'a Store<T>) -> Self {
        Self { store }
    }

    /// Borrow the wrapped runtime-layer store.
    pub fn inner(self) -> &'a wasm_runtime_layer::Store<StoreData<T>, Backend> {
        self.store.inner()
    }

    /// The store's handle tables.
    pub fn tables(self) -> &'a Arc<Mutex<HandleTables>> {
        self.store.tables()
    }

    /// Clone the handle for the per-store handle-tables ledger.
    pub fn tables_handle(self) -> Arc<Mutex<HandleTables>> {
        self.store.tables_handle()
    }

    /// Lock the store's handle tables and record state.
    pub fn lock_tables(self) -> Result<MutexGuard<'a, HandleTables>> {
        self.store.lock_tables()
    }

    /// The store's cooperative scheduler.
    pub fn scheduler(self) -> &'a Scheduler<T> {
        self.store.scheduler()
    }
}

/// The seam crate code reaches [`StoreInternal`] and
/// [`StoreRefInternal`] through.
///
/// The trait lives in a private module, so it cannot be imported
/// outside the crate and `store.internal()` resolves only inside it.
pub trait StoreInternalExt<T: 'static> {
    /// The workspace-internal entries of this store.
    fn internal(&mut self) -> StoreInternal<'_, T>;

    /// The workspace-internal entries of this store that need no
    /// mutable access.
    fn internal_ref(&self) -> StoreRefInternal<'_, T>;
}

impl<T: 'static> StoreInternalExt<T> for Store<T> {
    fn internal(&mut self) -> StoreInternal<'_, T> {
        StoreInternal::of(self)
    }

    fn internal_ref(&self) -> StoreRefInternal<'_, T> {
        StoreRefInternal::of(self)
    }
}
