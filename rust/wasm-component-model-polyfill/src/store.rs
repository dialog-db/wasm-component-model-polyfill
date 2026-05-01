//! The polyfill's owner of guest state.
//!
//! `Store<T>` carries host data of type `T` and is the unit of
//! isolation between independent component instances: the
//! polyfill's analogue to `wasmtime::Store`. The store also owns the
//! per-resource-type handle tables the canonical-ABI runtime-state
//! rules require.

use std::sync::{Arc, Mutex};

use crate::backend::Backend;
use crate::engine::Engine;
use crate::error::{Error, Result};
use crate::resource::{HandleTables, ResourceHandle, ResourceTypeId};

/// The polyfill's owner of guest state.
///
/// `T` is host data that travels with the store and is reachable from
/// every host function the polyfill later lets contributors define.
/// `Store` is constructed from an [`Engine`] and a host-data value via
/// [`Store::new`], and exposes [`data`][Store::data] /
/// [`data_mut`][Store::data_mut] accessors so host code can read and
/// mutate its host data without leaving the polyfill's API.
pub struct Store<T: 'static> {
    inner: wasm_runtime_layer::Store<T, Backend>,
    /// Per-resource-type handle tables. The `Arc<Mutex<...>>` shape
    /// lets resource trampolines and lift/lower contexts reach the
    /// tables from inside runtime-layer closures, where the
    /// polyfill's wrapper struct is otherwise unreachable.
    /// Workspace-internal; not re-exported by `lib.rs`.
    pub tables: Arc<Mutex<HandleTables>>,
}

impl<T: 'static> Store<T> {
    /// Construct a `Store` against an [`Engine`] and an initial value
    /// for the host-data slot.
    ///
    /// The return type is [`Result`] for forward compatibility with
    /// later work that surfaces backend errors at store construction
    /// time; today, the supported backends construct a store
    /// infallibly.
    #[allow(clippy::unnecessary_wraps)]
    pub fn new(engine: &Engine, data: T) -> Result<Self> {
        Ok(Self {
            inner: wasm_runtime_layer::Store::new(engine.inner(), data),
            tables: Arc::new(Mutex::new(HandleTables::new())),
        })
    }

    /// Borrow the host data carried by this store.
    pub fn data(&self) -> &T {
        self.inner.data()
    }

    /// Mutably borrow the host data carried by this store.
    pub fn data_mut(&mut self) -> &mut T {
        self.inner.data_mut()
    }

    /// Clone the handle for the per-store handle-tables ledger.
    /// Workspace-internal; not re-exported by `lib.rs`.
    pub fn tables_handle(&self) -> Arc<Mutex<HandleTables>> {
        self.tables.clone()
    }

    /// Mint a fresh `own<T>` handle in this store's resource table
    /// for the given registered resource type.
    ///
    /// The `rep` is a host-supplied 32-bit representation that
    /// identifies the resource's host-side state — typically an
    /// index into a host-managed table. The polyfill allocates a
    /// table entry, records the rep, and returns a [`ResourceHandle`]
    /// the caller can hand to a guest export through [`Val::Own`].
    ///
    /// [`Val::Own`]: crate::Val::Own
    pub fn resource_new(&self, type_id: ResourceTypeId, rep: u32) -> Result<ResourceHandle> {
        let mut guard = self.tables.lock().map_err(|_| Error::Internal {
            message: "resource handle tables lock poisoned".to_owned(),
        })?;
        let table = guard.for_type_mut(type_id);
        let index = table.insert(rep);
        Ok(ResourceHandle { type_id, index })
    }

    /// Borrow the wrapped runtime-layer store.
    ///
    /// Workspace-internal; not re-exported by `lib.rs`.
    pub fn inner(&self) -> &wasm_runtime_layer::Store<T, Backend> {
        &self.inner
    }

    /// Mutably borrow the wrapped runtime-layer store.
    ///
    /// Workspace-internal; not re-exported by `lib.rs`.
    pub fn inner_mut(&mut self) -> &mut wasm_runtime_layer::Store<T, Backend> {
        &mut self.inner
    }
}
