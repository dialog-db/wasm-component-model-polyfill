//! The polyfill's owner of guest state.
//!
//! `Store<T>` carries host data of type `T` and is the unit of
//! isolation between independent component instances: the
//! polyfill's analogue to `wasmtime::Store`. The store also owns the
//! per-resource-type handle tables the canonical-ABI runtime-state
//! rules require, and carries a process-unique identity so that an
//! instance can refuse a call made through a different store.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use wasm_runtime_layer::Val as RuntimeVal;

use crate::backend::Backend;
use crate::engine::Engine;
use crate::error::{AbiCause, AbiError, AbiPosition, Error, Result};
use crate::executor::ResourceDestructor;
use crate::resource::{HandleEntry, HandleTables, ResourceHandle, ResourceTypeId};
use crate::types::{ResourceType, ValueType};

/// A process-unique identity for one [`Store`].
///
/// Every store mints a fresh id at construction. An [`Instance`]
/// records the id of the store it was created in, and a function
/// handle compares that id with the store it is called with. The
/// wrapped integer is opaque and not exposed.
///
/// [`Instance`]: crate::Instance
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct StoreId(u64);

impl StoreId {
    fn fresh() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

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
    /// The store's process-unique identity. Workspace-internal; not
    /// re-exported by `lib.rs`.
    pub id: StoreId,
    /// Per-resource-type handle tables. The `Arc<Mutex<...>>` shape
    /// lets resource trampolines and lift/lower contexts reach the
    /// tables from inside runtime-layer closures, where the
    /// polyfill's wrapper struct is otherwise unreachable.
    /// Workspace-internal; not re-exported by `lib.rs`.
    pub tables: Arc<Mutex<HandleTables>>,
    /// The destructor of every resource type an instance in this
    /// store introduced, keyed by identity. Recorded at instantiation
    /// so that [`Self::resource_drop`] can run the right destructor
    /// for a handle the host holds. Workspace-internal.
    pub destructors: HashMap<ResourceTypeId, ResourceDestructor<T>>,
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
            id: StoreId::fresh(),
            tables: Arc::new(Mutex::new(HandleTables::new())),
            destructors: HashMap::new(),
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
        let mut guard = self
            .tables
            .lock()
            .map_err(|_| Error::internal("resource handle tables lock poisoned"))?;
        let table = guard.for_type_mut(type_id);
        let index = table.insert_own(rep);
        Ok(ResourceHandle {
            type_id,
            index,
            rep,
        })
    }

    /// Record the destructor of a resource type an instance
    /// introduced. Workspace-internal.
    pub fn register_destructor(
        &mut self,
        type_id: ResourceTypeId,
        destructor: ResourceDestructor<T>,
    ) {
        self.destructors.entry(type_id).or_insert(destructor);
    }

    /// Release a handle the host holds. The handle's entry leaves the
    /// host's table for its resource type, and the resource's
    /// destructor runs once: the registered closure for a host
    /// resource, or the defining component's own destructor for a
    /// locally-defined one. A handle that is not live, because it was
    /// released or handed to a guest already, fails with the
    /// invalid-handle ABI cause; a handle lent out as a borrow cannot
    /// be released until the call that borrowed it ends.
    ///
    /// Dropping the store instead runs no destructor: a handle the
    /// host never released is leaked, as in Wasmtime.
    pub fn resource_drop(&mut self, handle: ResourceHandle) -> Result<()> {
        let invalid = |reason: String| {
            Error::from(AbiError {
                position: AbiPosition::Argument(0),
                valtype: ValueType::Own(ResourceType::new("resource")),
                cause: AbiCause::InvalidHandle { reason },
            })
        };
        let rep = {
            let mut guard = self
                .tables
                .lock()
                .map_err(|_| Error::internal("resource handle tables lock poisoned"))?;
            let table = guard.host_table(handle.type_id);
            let entry = guard
                .for_table(table)
                .and_then(|t| t.entry(handle.index))
                .copied()
                .ok_or_else(|| {
                    invalid(format!(
                        "handle index {} is not live in the host's resource table",
                        handle.index
                    ))
                })?;
            match entry {
                HandleEntry::Own { rep, lend_count: 0 } => {
                    guard.for_table_mut(table).remove(handle.index);
                    rep
                }
                HandleEntry::Own { .. } => {
                    return Err(invalid(
                        "cannot remove owned resource while borrowed".to_owned(),
                    ));
                }
                HandleEntry::Borrow { .. } => {
                    return Err(invalid(format!(
                        "handle index {} is a borrow, which returns with its call",
                        handle.index
                    )));
                }
            }
        };
        let Some(destructor) = self.destructors.get(&handle.type_id).cloned() else {
            return Ok(());
        };
        match destructor {
            ResourceDestructor::Host(body) => body(self.inner.data_mut(), rep),
            ResourceDestructor::Local(slot) => {
                let function = slot
                    .lock()
                    .map_err(|_| Error::internal("resource destructor slot poisoned"))?
                    .clone();
                if let Some(function) = function {
                    function
                        .call(&mut self.inner, &[RuntimeVal::I32(rep as i32)], &mut [])
                        .map_err(|err| {
                            Error::from(AbiError {
                                position: AbiPosition::Argument(0),
                                valtype: ValueType::Own(ResourceType::new("resource")),
                                cause: AbiCause::SubstrateFailure(err),
                            })
                        })?;
                }
                Ok(())
            }
        }
    }

    /// Open a call scope on the handle tables. Workspace-internal.
    pub fn enter_call(&self) -> Result<()> {
        self.tables
            .lock()
            .map_err(|_| Error::internal("resource handle tables lock poisoned"))?
            .enter_call();
        Ok(())
    }

    /// Close the innermost call scope on its success path. The inner
    /// `Err` carries the count of borrows the guest did not drop.
    /// Workspace-internal.
    pub fn exit_call(&self) -> Result<core::result::Result<(), u32>> {
        Ok(self
            .tables
            .lock()
            .map_err(|_| Error::internal("resource handle tables lock poisoned"))?
            .exit_call())
    }

    /// Close the innermost call scope on its failure path.
    /// Workspace-internal.
    pub fn abandon_call(&self) -> Result<()> {
        self.tables
            .lock()
            .map_err(|_| Error::internal("resource handle tables lock poisoned"))?
            .abandon_call();
        Ok(())
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
