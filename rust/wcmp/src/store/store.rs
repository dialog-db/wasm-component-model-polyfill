//! The polyfill's owner of guest state.

use core::task::Waker;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::concurrency::{
    Accessor, HostSuspensionProvider, Outcome, Scheduler, StackSwitchingProvider, StoreProvider,
};
use crate::engine::Engine;
use crate::error::Result;
use crate::internal::EngineInternal;
use crate::resource::{HandleTables, ResourceHandle, ResourceTypeId};
use crate::runtime_layer::{AsContextMut, MaybeSend, Store as RuntimeStore, substrate_failure};
use crate::suspend_provider_kind::SuspendProviderKind;

use super::store_context::StoreContext;
use super::store_context::internal::StoreContextInternalExt;
use super::store_data::StoreData;
use super::store_id::StoreId;

pub mod internal;

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
///
/// # A trap poisons the store
///
/// A trap anywhere in the store poisons it, as Wasmtime keeps one
/// trapped flag per store, and a poisoned store runs no more guest
/// code. [`Func::call`], [`Func::call_concurrent`], their typed
/// counterparts, every instantiation into the store, and
/// [`Store::resource_drop`] of a resource a guest defines fail with
/// the cannot-enter cause, [`TaskCause::CannotEnter`]. What runs no
/// guest code still works: the release of a resource the host
/// defines, [`Store::run_concurrent`] whose closure does only host
/// work, the host data, and dropping the store. Nothing clears the
/// flag, and nothing answers whether it is set, as in Wasmtime; a
/// host that meets a trap drops the store and builds a new one.
///
/// Wasmtime lets an instantiation into a poisoned store run. The
/// polyfill refuses it, because an instantiation runs the `start`
/// functions of its core modules, and the Component Model runs no
/// guest code after a trap.
///
/// At the moment of the trap the store discards its work. Every
/// queued guest work item goes — a callback, the start of a task, a
/// thread that was ready to resume — and so does every pending host
/// future: the future of a host `async` function, a stream or future
/// producer, and a consumer, each dropped there. Host work that
/// starts after the trap, such as a pipe of the host's own stream to
/// a consumer of its own, touches no guest and runs. The records of
/// the store's tasks and subtasks stay until the store drops. A later
/// driver therefore meets no stale work, and fails only for an entry
/// it makes itself. A trap in a turn ends the driver that is polling,
/// so a call future of a `run_concurrent` closure goes with the
/// closure; one whose store a trap poisoned in some other way fails
/// with the cannot-enter cause the next time it is polled. Wasmtime
/// keeps its queued items and host futures, and a later
/// `run_concurrent` runs them. The polyfill discards them, because
/// the Component Model runs no guest code after a trap.
///
/// A discarded future is dropped where host code may run: no lock of
/// the store is held, and the store is not lent to a poll. A `Drop`
/// that reaches the store through its [`Accessor`] therefore neither
/// deadlocks nor panics. The reach fails with the store-not-in-poll
/// cause, or with the recursive-driver cause inside another reach,
/// as it would anywhere else the store is not lent.
///
/// [`Func::call`]: crate::Func::call
/// [`Func::call_concurrent`]: crate::Func::call_concurrent
/// [`TaskCause::CannotEnter`]: crate::TaskCause::CannotEnter
///
/// # What the store does not lend
///
/// The scheduler, the handle tables, and the runtime-layer store are
/// the polyfill's own bookkeeping. Host code holding a `Store` reads
/// and writes its host data, mints and releases handles, and drives
/// the store; it reaches none of the three, and the compiler is what
/// says so. The scheduler:
///
/// ```compile_fail
/// # use wcmp::{Engine, Store};
/// let engine = Engine::with_backend(wcmp_wasm_core_wasmtime::Wasmtime::new().unwrap()).unwrap();
/// let mut store = Store::new(&engine, ()).unwrap();
/// let _ = store.scheduler_mut();
/// ```
///
/// The handle tables:
///
/// ```compile_fail
/// # use wcmp::{Engine, Store};
/// let engine = Engine::with_backend(wcmp_wasm_core_wasmtime::Wasmtime::new().unwrap()).unwrap();
/// let store = Store::new(&engine, ()).unwrap();
/// let _ = store.lock_tables();
/// ```
///
/// The runtime-layer store:
///
/// ```compile_fail
/// # use wcmp::{Engine, Store};
/// let engine = Engine::with_backend(wcmp_wasm_core_wasmtime::Wasmtime::new().unwrap()).unwrap();
/// let mut store = Store::new(&engine, ()).unwrap();
/// let _ = store.inner_mut();
/// ```
///
/// And the seam the crate reaches all three through. That one is not
/// a private method but a method of [`StoreInternalExt`], so a block
/// that calls it has to import the trait, and the import is what has
/// to fail: a caller outside the crate has no name for it, because
/// `lib.rs` re-exports the store and not its seam. Importing a name
/// `lib.rs` does re-export, from the same module, is what the working
/// case looks like:
///
/// ```rust
/// use wcmp::Store;
/// let _: Option<Store<()>> = None;
/// ```
///
/// Importing the seam does not resolve, so the call never gets as far
/// as being looked up:
///
/// ```compile_fail
/// # use wcmp::{Engine, Store};
/// use wcmp::StoreInternalExt;
/// let engine = Engine::with_backend(wcmp_wasm_core_wasmtime::Wasmtime::new().unwrap()).unwrap();
/// let mut store = Store::new(&engine, ()).unwrap();
/// let _ = store.internal();
/// ```
///
/// The same holds of the borrow of the store a host function is
/// handed. A turn of the store is reachable from the [`Store`] and
/// from nowhere else:
///
/// ```compile_fail
/// # use wcmp::{Engine, Store};
/// # use core::task::Waker;
/// let engine = Engine::with_backend(wcmp_wasm_core_wasmtime::Wasmtime::new().unwrap()).unwrap();
/// let mut store = Store::new(&engine, ()).unwrap();
/// let _ = store.turn(Waker::noop());
/// ```
///
/// A private-method block compiles the moment the method is made
/// public, and the seam block compiles the moment `lib.rs` re-exports
/// the trait, so each one is a standing check that neither has
/// happened.
///
/// [`StoreInternalExt`]: super::StoreInternalExt
pub struct Store<T: 'static> {
    inner: crate::runtime_layer::Store<StoreData<T>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl<T: Send + 'static> Store<T> {
    /// Construct a `Store` against an [`Engine`] and an initial value
    /// for the host-data slot.
    ///
    /// When the engine selected a suspend provider, the store
    /// instantiates it here and keeps it for its whole life, and the
    /// construction fails when the backend refuses that instantiation.
    /// The construction also fails when the backend refuses to make a
    /// store, which no backend does today.
    ///
    /// Natively the host data must be `Send`, as it must be for
    /// Wasmtime's asynchronous API, because the store moves with the
    /// futures that drive it. In the browser it need not be.
    pub fn new(engine: &Engine, data: T) -> Result<Self> {
        Self::build(engine, data)
    }
}

#[cfg(target_arch = "wasm32")]
impl<T: 'static> Store<T> {
    /// Construct a `Store` against an [`Engine`] and an initial value
    /// for the host-data slot.
    ///
    /// When the engine selected a suspend provider, the store
    /// instantiates it here and keeps it for its whole life, and the
    /// construction fails when the backend refuses that instantiation.
    /// The construction also fails when the backend refuses to make a
    /// store, which no backend does today.
    ///
    /// Natively the host data must be `Send`, as it must be for
    /// Wasmtime's asynchronous API, because the store moves with the
    /// futures that drive it. In the browser it need not be.
    pub fn new(engine: &Engine, data: T) -> Result<Self> {
        Self::build(engine, data)
    }
}

impl<T: 'static> Store<T> {
    /// Construct a store, as [`new`](Self::new) states, on either
    /// target. Workspace-internal.
    fn build(engine: &Engine, data: T) -> Result<Self>
    where
        StoreData<T>: MaybeSend,
    {
        let inner =
            RuntimeStore::new(engine.inner(), StoreData::new(data)).map_err(substrate_failure)?;
        let mut store = Self { inner };
        // The provider the engine selected is instantiated in the
        // store once, here, and stays in it for the store's life.
        let provider = match engine.suspend_provider() {
            SuspendProviderKind::StackSwitching => Some(StoreProvider::StackSwitching(
                StackSwitchingProvider::instantiate(
                    &mut store.context(),
                    engine.inner(),
                    engine.switch_modules(),
                )?,
            )),
            SuspendProviderKind::HostSuspension => Some(StoreProvider::HostSuspension(
                HostSuspensionProvider::instantiate(&mut store.context(), engine.switch_modules())?,
            )),
            _ => None,
        };
        if let Some(provider) = provider {
            store.store_data_mut().install_provider(provider);
        }
        Ok(store)
    }

    /// Borrow the host data carried by this store.
    pub fn data(&self) -> &T {
        self.inner.data().host()
    }

    /// Mutably borrow the host data carried by this store.
    pub fn data_mut(&mut self) -> &mut T {
        self.inner.data_mut().host_mut()
    }

    /// The copy budget each crossing of this store starts with, in
    /// bytes: the default of 128 MiB, or the last amount passed to
    /// [`Store::set_hostcall_fuel`]. Wasmtime calls this budget the
    /// store's hostcall fuel, and names the method the same.
    pub fn hostcall_fuel(&self) -> usize {
        self.store_data().hostcall_fuel()
    }

    /// Set the copy budget each crossing of this store starts with to
    /// `fuel` bytes.
    ///
    /// A value a guest hands the host — the arguments of a call into
    /// the host, a `task.return`, the result of a call the host made
    /// — is built on the host out of the guest's memory, and a lifted
    /// value can cost the host far more than the guest bytes it came
    /// from. The budget caps what one crossing may build, so a guest
    /// cannot have the host allocate without bound. Each crossing
    /// starts from the whole of it, and each lift charges it before
    /// reserving anything: a list or a fixed-length list 32 bytes per
    /// element, a map 64 bytes per entry, a string the byte length of
    /// its range, and a typed vector of numbers its own bytes. The
    /// lift that would pass it fails with
    /// [`AbiCause::CopyBudgetSpent`]. The per-element costs are fixed
    /// rather than the size of a host value on the target, so a
    /// guest's value is accepted or refused alike natively and in a
    /// browser.
    ///
    /// A value the host lowers into a guest is not charged: the host
    /// already holds it. A crossing already under way keeps the
    /// budget it started with.
    ///
    /// The default is 128 MiB, as in Wasmtime, whose
    /// `Store::set_hostcall_fuel` this mirrors.
    ///
    /// [`AbiCause::CopyBudgetSpent`]: crate::AbiCause::CopyBudgetSpent
    pub fn set_hostcall_fuel(&mut self, fuel: usize) {
        self.store_data_mut().set_hostcall_fuel(fuel);
    }

    /// Borrow the store as a [`StoreContext`], which is what a host
    /// hands [`StreamReader::new`] and [`FutureReader::new`] to create
    /// a stream or a future in this store. The name is Wasmtime's.
    ///
    /// The context lends what the store itself lends — the host data
    /// — and the entries that take a context; the store's own
    /// bookkeeping stays out of reach, as it does through the store.
    ///
    /// [`StreamReader::new`]: crate::StreamReader::new
    /// [`FutureReader::new`]: crate::FutureReader::new
    pub fn as_context_mut(&mut self) -> StoreContext<'_, T> {
        self.context()
    }

    /// The store as a turn, an item, or a trampoline reaches it: a
    /// borrow of the core store, which carries everything else the
    /// store holds. Every entry that touches guest state lives
    /// there. Workspace-internal; not re-exported by `lib.rs`.
    fn context(&mut self) -> StoreContext<'_, T> {
        StoreContext::new(self.inner.as_context_mut())
    }

    /// Everything the store carries: the host's data and the
    /// polyfill's own state. Workspace-internal.
    fn store_data(&self) -> &StoreData<T> {
        self.inner.data()
    }

    /// Everything the store carries, mutably. Workspace-internal.
    fn store_data_mut(&mut self) -> &mut StoreData<T> {
        self.inner.data_mut()
    }

    /// The store's process-unique identity. Workspace-internal.
    fn id(&self) -> StoreId {
        self.store_data().id()
    }

    /// The store's handle tables. Workspace-internal.
    fn tables(&self) -> &Arc<Mutex<HandleTables>> {
        self.store_data().tables()
    }

    /// Clone the handle for the per-store handle-tables ledger.
    /// Workspace-internal; not re-exported by `lib.rs`.
    fn tables_handle(&self) -> Arc<Mutex<HandleTables>> {
        self.store_data().tables_handle()
    }

    /// Lock the store's handle tables and record state.
    /// Workspace-internal.
    fn lock_tables(&self) -> Result<MutexGuard<'_, HandleTables>> {
        self.store_data().lock_tables()
    }

    /// The store's cooperative scheduler: the ready queues, the host
    /// tasks, the entry gate, and the suspend seam.
    /// Workspace-internal.
    fn scheduler(&self) -> &Scheduler<T> {
        self.store_data().scheduler()
    }

    /// The store's cooperative scheduler, mutably.
    /// Workspace-internal.
    fn scheduler_mut(&mut self) -> &mut Scheduler<T> {
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

    /// Release a handle the host holds. The handle's entry leaves the
    /// host's table for its resource type, and the resource's
    /// destructor runs once: the registered closure for a host
    /// resource, or the defining component's own destructor for a
    /// locally-defined one. A handle that is not live, because it was
    /// released or handed to a guest already, fails with the
    /// invalid-handle ABI cause; a handle lent out as a borrow cannot
    /// be released until the call that borrowed it ends.
    ///
    /// The destructor runs as a task with one thread of its own, as
    /// it does when a guest drops the last owning handle: it sees
    /// two context slots of zero, and what it writes into them ends
    /// with it.
    ///
    /// A destructor may not block. The Canonical ABI says so under
    /// `canon resource.drop`: the destructor call works like a
    /// synchronous cross-component call, and `canon lift` traps a
    /// call that is not `async`-typed and blocks before it returns.
    /// Wasmtime enters a destructor as a synchronous call and traps
    /// a block inside it with `Trap::CannotBlockSyncTask`. A
    /// destructor that blocks therefore fails the release with the
    /// cannot-block cause, on every target and whether or not a
    /// suspend provider is installed.
    ///
    /// A locally-defined resource's destructor is guest code, and
    /// guest code runs inside a turn of the store's scheduler, so
    /// the release runs one for it. Three rules come with the turn.
    /// A destructor that calls a host `async` import whose future is
    /// ready when the import polls it gets the result and returns,
    /// and the release stands. A destructor that lowers such an
    /// import synchronously and whose future stays pending blocks,
    /// which fails the release with the cannot-block cause. A
    /// destructor that lowers the import asynchronously does not
    /// block: it leaves its host task in the store, where the next
    /// turn of any driver polls it.
    ///
    /// A host resource's destructor is the host's own closure and
    /// not guest code, so it runs outside a turn, as the call that
    /// released the handle does.
    ///
    /// A locally-defined resource's destructor that fails is a trap,
    /// and poisons the store. A poisoned store refuses the release of
    /// a resource a guest defines with the cannot-enter cause, after
    /// the handle has left the host's table, as in Wasmtime: the
    /// destructor is guest code, and no guest code runs in the store
    /// again. The release of a resource the host defines still runs,
    /// because its destructor is the host's own.
    ///
    /// Dropping the store instead runs no destructor: a handle the
    /// host never released is leaked, as in Wasmtime.
    pub fn resource_drop(&mut self, handle: ResourceHandle) -> Result<()> {
        let mut context = self.context();
        context.internal().resource_drop(handle)
    }

    /// Whether a turn of this store is running. A driver entered
    /// while another driver of the same store is inside a turn fails
    /// with the recursive-driver cause. Workspace-internal.
    fn turn_in_flight(&self) -> bool {
        self.store_data().turn_in_flight()
    }

    /// Run one turn of the store's scheduler. Workspace-internal;
    /// see [`StoreContext::turn`], which is where a turn lives.
    fn turn(&mut self, waker: &Waker) -> Result<Outcome> {
        let mut context = self.context();
        context.internal().turn(waker)
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
    /// `body` does not borrow the store, and neither does the
    /// [`Accessor`] it is handed: the accessor is a token carrying
    /// the store's identity, so it has no lifetime and `body`'s
    /// future can hold one across its awaits. `body` reaches the
    /// store's host data only inside a closure the accessor runs,
    /// through [`Accessor::with`], and only while a poll of that
    /// future is running; a value taken from the host data must be
    /// cloned out of that closure. A reach made where no poll of
    /// this store is running fails with the store-not-in-poll
    /// cause.
    ///
    /// Entering this entry while another driver of the same store is
    /// inside a turn fails with the recursive-driver cause. Dropping
    /// the returned future cancels nothing: whatever the driver
    /// queued stays in the store and runs in the next turn of any
    /// driver, unless a trap poisons the store first and discards it.
    ///
    /// A turn that finds nothing ready and no host task pending
    /// leaves this entry pending rather than failing with the
    /// deadlock cause, which is the one rule where it differs from
    /// the other drivers: `body`'s future can wait on something
    /// outside the store, and the waker it was polled with is the
    /// one that brings the entry back.
    ///
    /// # Where a trap surfaces
    ///
    /// The first trap ends the driver that is polling the store, with
    /// that trap, and poisons the store in the same step. While this
    /// entry runs turns, it is that driver, whichever task the trap
    /// belongs to: a task a [`Func::call_concurrent`] inside `body`
    /// started, work that a task left after it resolved, a thread
    /// that outlives its task's host call, or a host `async` function
    /// whose future fails, which is a trap of the guest task that
    /// called it. The entry returns the trap, and `body` is dropped
    /// wherever its poll left it, with every call future inside it:
    /// the call whose task trapped does not answer, and neither does
    /// any other.
    ///
    /// The trap is never held for a caller that already has its
    /// result. A call that answered before its task trapped keeps its
    /// answer and never learns of the trap; this entry reports it
    /// when one of its turns meets the trap, and a later driver
    /// reports it when this entry has already returned. Every driver
    /// after the trap fails with the cannot-enter cause, and a
    /// [`Func::call_concurrent`] made inside `body` is refused with it.
    ///
    /// [`Func::call_concurrent`]: crate::Func::call_concurrent
    pub async fn run_concurrent<R, F>(&mut self, body: F) -> Result<R>
    where
        F: AsyncFnOnce(&Accessor<T>) -> R,
    {
        let mut context = self.context();
        context.internal().run_concurrent(body).await
    }

    /// Mutably borrow the wrapped runtime-layer store.
    ///
    /// Workspace-internal; not re-exported by `lib.rs`.
    fn inner_mut(&mut self) -> &mut crate::runtime_layer::Store<StoreData<T>> {
        &mut self.inner
    }
}

impl<T: 'static> Drop for Store<T> {
    /// Drop the store, and with it every task, host task, and
    /// suspended thread, with no destructor run.
    ///
    /// A thread the host-suspension provider resumed can run on after
    /// the driver that awaited it dropped, as it does in the browser,
    /// where it runs on a microtask. The runtime layer then keeps the
    /// store allocated for the thread until it stops, and the store is
    /// marked dropped here. The thread's shim finds the mark and traps,
    /// so the thread's stack unwinds where it suspended and runs no
    /// guest code, host import, or destructor. The host's data drops
    /// with the store once the thread stopped.
    fn drop(&mut self) {
        self.store_data_mut().mark_dropped();
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
    use crate::executor::ResourceDestructor;
    use crate::internal::ResourceTypeIdInternal;
    use crate::value::Val;

    use super::*;

    #[wcmp_macros::test]
    fn it_runs_no_destructor_when_the_store_is_dropped() {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");

        // A resource the host holds, whose destructor would run if
        // anything released the handle.
        let destructor_runs = Arc::new(AtomicUsize::new(0));
        let counted = destructor_runs.clone();
        let type_id = ResourceTypeId::fresh();
        store.context().internal().register_resource(
            type_id,
            None,
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
        let subtask = store
            .lock_tables()
            .expect("tables")
            .tasks
            .push_subtask()
            .expect("room under the record cap");
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
