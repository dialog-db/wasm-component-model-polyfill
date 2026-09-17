//! The polyfill's owner of guest state.

use core::task::Waker;
use std::sync::{Arc, Mutex, MutexGuard};

use wasm_runtime_layer::AsContextMut;

use crate::backend::Backend;
use crate::concurrency::{Accessor, Outcome, Scheduler};
use crate::engine::Engine;
use crate::error::Result;
use crate::executor::ResourceDestructor;
use crate::resource::{HandleTables, ResourceHandle, ResourceTypeId};

use super::store_context::StoreContext;
use super::store_data::StoreData;
use super::store_id::StoreId;

/// The polyfill's owner of guest state.
///
/// `T` is host data that travels with the store and is reachable from
/// every host function the polyfill later lets contributors define.
/// `Store` is constructed from an [`Engine`] and a host-data value via
/// [`Store::new`], and exposes [`data`][Store::data] /
/// [`data_mut`][Store::data_mut] accessors so host code can read and
/// mutate its host data without leaving the polyfill's API.
///
/// The store is the unit of isolation between independent component
/// instances: the polyfill's analogue to `wasmtime::Store`. It owns
/// the core store the runtime layer gives it, and that store carries
/// [`StoreData`]: the host's data, the handle tables the
/// canonical-ABI runtime-state rules require — one per component
/// instance, shared by every handle kind that instance uses, plus
/// one per resource type for the host's own handles — the scheduler,
/// the destructors its instances registered, and a process-unique
/// identity so that an instance can refuse a call made through a
/// different store.
///
/// Everything the store does to guest state it does through
/// [`StoreContext`], which is a borrow of that core store. A host
/// trampoline cannot reach this type — the runtime layer hands it a
/// context and nothing else — but it can build a context of its own,
/// which is what makes the scheduler and its suspend seam reachable
/// from inside a guest call.
pub struct Store<T: 'static> {
    inner: wasm_runtime_layer::Store<StoreData<T>, Backend>,
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
            inner: wasm_runtime_layer::Store::new(engine.inner(), StoreData::new(data)),
        })
    }

    /// Borrow the host data carried by this store.
    pub fn data(&self) -> &T {
        self.inner.data().host()
    }

    /// Mutably borrow the host data carried by this store.
    pub fn data_mut(&mut self) -> &mut T {
        self.inner.data_mut().host_mut()
    }

    /// The store as a turn, an item, or a trampoline reaches it: a
    /// borrow of the core store, which carries everything else the
    /// store holds. Every entry that touches guest state lives
    /// there. Workspace-internal; not re-exported by `lib.rs`.
    pub fn context(&mut self) -> StoreContext<'_, T> {
        StoreContext::new(self.inner.as_context_mut())
    }

    /// Everything the store carries: the host's data and the
    /// polyfill's own state. Workspace-internal.
    pub fn store_data(&self) -> &StoreData<T> {
        self.inner.data()
    }

    /// Everything the store carries, mutably. Workspace-internal.
    pub fn store_data_mut(&mut self) -> &mut StoreData<T> {
        self.inner.data_mut()
    }

    /// The store's process-unique identity. Workspace-internal.
    pub fn id(&self) -> StoreId {
        self.store_data().id()
    }

    /// The store's handle tables. Workspace-internal.
    pub fn tables(&self) -> &Arc<Mutex<HandleTables>> {
        self.store_data().tables()
    }

    /// Clone the handle for the per-store handle-tables ledger.
    /// Workspace-internal; not re-exported by `lib.rs`.
    pub fn tables_handle(&self) -> Arc<Mutex<HandleTables>> {
        self.store_data().tables_handle()
    }

    /// Lock the store's handle tables and record state.
    /// Workspace-internal.
    pub fn lock_tables(&self) -> Result<MutexGuard<'_, HandleTables>> {
        self.store_data().lock_tables()
    }

    /// The store's cooperative scheduler: the ready queues, the host
    /// tasks, the entry gate, and the suspend seam.
    /// Workspace-internal.
    pub fn scheduler(&self) -> &Scheduler<T> {
        self.store_data().scheduler()
    }

    /// The store's cooperative scheduler, mutably.
    /// Workspace-internal.
    pub fn scheduler_mut(&mut self) -> &mut Scheduler<T> {
        self.store_data_mut().scheduler_mut()
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
        self.store_data().resource_new(type_id, rep)
    }

    /// Record the destructor of a resource type an instance
    /// introduced. Workspace-internal.
    pub fn register_destructor(
        &mut self,
        type_id: ResourceTypeId,
        destructor: ResourceDestructor<T>,
    ) {
        self.store_data_mut()
            .register_destructor(type_id, destructor);
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
        self.context().resource_drop(handle)
    }

    /// Whether a turn of this store is running. A driver entered
    /// while another driver of the same store is inside a turn fails
    /// with the recursive-driver cause. Workspace-internal.
    pub fn turn_in_flight(&self) -> Result<bool> {
        self.store_data().turn_in_flight()
    }

    /// Whether the store holds work only a turn can carry forward:
    /// an item in one of the ready queues, deferred work included,
    /// or a host task that has not resolved. Workspace-internal.
    pub fn has_pending_work(&self) -> bool {
        self.store_data().has_pending_work()
    }

    /// Run one turn of the store's scheduler. Workspace-internal;
    /// see [`StoreContext::turn`], which is where a turn lives.
    pub fn turn(&mut self, waker: &Waker) -> Result<Outcome> {
        self.context().turn(waker)
    }

    /// Run `body` with an accessor to this store, driving the
    /// store's scheduler until the future `body` returns completes,
    /// and return what that future resolved to.
    ///
    /// This is a driver: one poll of the returned future is a turn,
    /// and guest code runs only inside a turn. A host uses it to let
    /// a task finish after the call that started it returned, and to
    /// run host tasks that no call owns.
    ///
    /// `body` does not borrow the store. It reaches the store's host
    /// data only inside a closure the [`Accessor`] runs, through
    /// [`Accessor::with`], and a value taken from the host data must
    /// be cloned out of that closure.
    ///
    /// Entering this entry while another driver of the same store is
    /// inside a turn fails with the recursive-driver cause. Dropping
    /// the returned future cancels nothing: whatever the driver
    /// queued stays in the store and runs in the next turn of any
    /// driver.
    ///
    /// A turn that finds nothing ready and no host task pending
    /// leaves this entry pending rather than failing with the
    /// deadlock cause, which is the one rule where it differs from
    /// the other drivers: `body`'s future can wait on something
    /// outside the store, and the waker it was polled with is the
    /// one that brings the entry back.
    pub async fn run_concurrent<'a, R, F>(&'a mut self, body: F) -> Result<R>
    where
        F: AsyncFnOnce(&Accessor<'a, T>) -> R,
    {
        self.context().run_concurrent(body).await
    }

    /// Borrow the wrapped runtime-layer store.
    ///
    /// Workspace-internal; not re-exported by `lib.rs`.
    pub fn inner(&self) -> &wasm_runtime_layer::Store<StoreData<T>, Backend> {
        &self.inner
    }

    /// Mutably borrow the wrapped runtime-layer store.
    ///
    /// Workspace-internal; not re-exported by `lib.rs`.
    pub fn inner_mut(&mut self) -> &mut wasm_runtime_layer::Store<StoreData<T>, Backend> {
        &mut self.inner
    }
}

/// A store is `Send` on the native target, and everything it holds
/// keeps it so: the actions of its queued items, the bodies of its
/// host tasks, and the lowerings those bodies' results cross
/// through. The browser drops the bound, because a body that awaits
/// a JavaScript promise is not `Send` and the whole polyfill runs on
/// one thread there. This is the assertion that fails the native
/// build the moment something not `Send` joins a store.
#[cfg(not(target_arch = "wasm32"))]
const _: fn() = || {
    fn assert_send<S: Send>() {}
    assert_send::<Store<u32>>();
};

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    use crate::concurrency::{HostTask, Item, ItemKind};
    use crate::engine::Engine;
    use crate::value::Val;

    use super::*;

    #[test]
    fn it_runs_no_destructor_when_the_store_is_dropped() {
        let engine = Engine::new().expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");

        // A resource the host holds, whose destructor would run if
        // anything released the handle.
        let destructor_runs = Arc::new(AtomicUsize::new(0));
        let counted = destructor_runs.clone();
        let type_id = ResourceTypeId::fresh();
        store.register_destructor(
            type_id,
            ResourceDestructor::Host(Arc::new(move |_data: &mut (), _rep: u32| {
                counted.fetch_add(1, AtomicOrdering::Relaxed);
                Ok(())
            })),
        );
        let _handle = store.resource_new(type_id, 7).expect("mint a handle");

        // A queued item and a host task, neither of which has run.
        let item_runs = Arc::new(AtomicUsize::new(0));
        let counted = item_runs.clone();
        store.scheduler_mut().push_high_priority(Item::new(
            ItemKind::TaskStart,
            move |_store: &mut StoreContext<'_, ()>| {
                counted.fetch_add(1, AtomicOrdering::Relaxed);
                Ok(())
            },
        ));
        let subtask = store.lock_tables().expect("tables").tasks.push_subtask();
        store.scheduler_mut().push_host_task(HostTask::from_future(
            subtask,
            |_store: &mut StoreContext<'_, ()>, _outcome: Result<Vec<Val>>| Ok(()),
            core::future::pending::<Result<Vec<Val>>>(),
        ));
        assert_eq!(store.scheduler().queued_items(), 1);
        assert_eq!(store.scheduler().host_task_count(), 1);

        drop(store);

        assert_eq!(
            destructor_runs.load(AtomicOrdering::Relaxed),
            0,
            "dropping the store runs no destructor"
        );
        assert_eq!(
            item_runs.load(AtomicOrdering::Relaxed),
            0,
            "the queued item is dropped unrun"
        );
    }
}
