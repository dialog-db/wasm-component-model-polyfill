//! The polyfill's owner of guest state.
//!
//! `Store<T>` carries host data of type `T` and is the unit of
//! isolation between independent component instances: the
//! polyfill's analogue to `wasmtime::Store`. The store also owns the
//! per-resource-type handle tables the canonical-ABI runtime-state
//! rules require, and carries a process-unique identity so that an
//! instance can refuse a call made through a different store.

use core::task::{Poll, Waker};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use wasm_runtime_layer::Val as RuntimeVal;

use crate::backend::Backend;
use crate::component::FunctionType;
use crate::concurrency::{HostTask, InstanceId, Item, ItemKind, Outcome, Scheduler, TaskId};
use crate::engine::Engine;
use crate::error::{AbiCause, AbiError, AbiPosition, Error, Result, SchedulerCause};
use crate::executor::ResourceDestructor;
use crate::executor::ir::CanonOptions;
use crate::resource::{HandleLookupError, HandleTables, ResourceHandle, ResourceTypeId};
use crate::types::{ResourceType, ValueType};
use crate::value::Val;

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
    /// The store's cooperative scheduler: the ready queues, the host
    /// tasks, and the entry gate. An item runs against this store
    /// and a host task's future need not be `Send` in the browser,
    /// so neither can live behind the tables lock; the half a
    /// trampoline reaches — the waker of the running turn and the
    /// in-turn flag — sits in the tables instead.
    /// Workspace-internal; not re-exported by `lib.rs`.
    pub scheduler: Scheduler<T>,
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
            scheduler: Scheduler::new(),
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
        let table = guard.host_table(type_id);
        let index = guard.insert_own(table, type_id, self.is_guest_defined(type_id), rep);
        Ok(ResourceHandle {
            type_id,
            index,
            rep,
        })
    }

    /// Whether a component defines the resource type, as far as the
    /// store knows: an instance registered an in-binary destructor
    /// for it. Read for the wrong-type trap message.
    fn is_guest_defined(&self, type_id: ResourceTypeId) -> bool {
        matches!(
            self.destructors.get(&type_id),
            Some(ResourceDestructor::Local(_))
        )
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
            guard
                .remove_own(
                    table,
                    handle.index,
                    handle.type_id,
                    self.is_guest_defined(handle.type_id),
                )
                .map_err(|e| {
                    invalid(match e {
                        HandleLookupError::Unknown { index } => {
                            format!("handle index {index} is not live in the host's resource table")
                        }
                        HandleLookupError::NotOwned { index } => {
                            format!("handle index {index} is a borrow, which returns with its call")
                        }
                        other => other.to_string(),
                    })
                })?
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

    /// Run one turn of the store's scheduler.
    ///
    /// A turn runs every item that is ready, one at a time, each to
    /// its next yield point; then it polls the host tasks with
    /// `waker`, the waker of the driver that is polling. The waker
    /// is recorded for the duration of the turn so that a trampoline
    /// which starts a host task can poll its future once with the
    /// same waker before it returns to the guest.
    ///
    /// The order is the one Wasmtime resolves the specification's
    /// open choices to: the resume-after-yield slot first, then the
    /// switch slot, then fresh readiness, and a resumption after a
    /// yield last — and that one only after the driver has returned
    /// control to the host executor, which is what the [`Yield`]
    /// outcome asks for.
    ///
    /// [`Yield`]: Outcome::Yield
    ///
    /// Workspace-internal; not re-exported by `lib.rs`.
    pub fn turn(&mut self, waker: &Waker) -> Result<Outcome> {
        Self::lock_handle(&self.tables)?.scheduler.enter_turn(waker);
        let outcome = self.run_turn(waker);
        if let Ok(mut guard) = self.tables.lock() {
            guard.scheduler.leave_turn();
        }
        outcome
    }

    /// Whether a turn of this store is running. A driver entered
    /// while another driver of the same store is inside a turn fails
    /// with the recursive-driver cause. Workspace-internal.
    pub fn turn_in_flight(&self) -> Result<bool> {
        Ok(self.lock_tables()?.scheduler.in_turn())
    }

    /// Run `body` inside a turn of this store, so that the guest
    /// code it reaches runs where every other piece of guest code
    /// runs. Instantiation uses this for the initializers of the
    /// plan: they are not queued items, because they run against
    /// borrowed plan state that no item could hold, but they are
    /// still guest work and belong inside a turn.
    /// Workspace-internal.
    pub fn run_in_turn<R>(
        &mut self,
        waker: &Waker,
        body: impl FnOnce(&mut Self) -> R,
    ) -> Result<R> {
        Self::lock_handle(&self.tables)?.scheduler.enter_turn(waker);
        let value = body(self);
        if let Ok(mut guard) = self.tables.lock() {
            guard.scheduler.leave_turn();
        }
        Ok(value)
    }

    /// Why a driver that went idle failed: the cannot-block cause
    /// when the task it waits on is one that must not block, and the
    /// deadlock cause otherwise. Workspace-internal.
    pub fn idle_cause(&self, task: Option<TaskId>) -> SchedulerCause {
        let Ok(guard) = self.tables.lock() else {
            return SchedulerCause::Deadlock;
        };
        let must_not_block = task
            .and_then(|task| guard.tasks.task(task))
            .and_then(|record| guard.tasks.instance(record.instance))
            .map(|record| record.may_not_suspend)
            .unwrap_or(false);
        if must_not_block {
            SchedulerCause::CannotBlock
        } else {
            SchedulerCause::Deadlock
        }
    }

    /// Give a host task to the store. The next turn polls it with
    /// the driver's waker, so no wake is lost. Workspace-internal.
    pub fn push_host_task(&mut self, task: HostTask) {
        self.scheduler.push_host_task(task);
    }

    /// The body of one turn, with the waker already recorded.
    fn run_turn(&mut self, waker: &Waker) -> Result<Outcome> {
        self.open_entry_gate()?;
        if let Some(item) = self.scheduler.take_resume_after_yield() {
            item.run(self);
        }
        loop {
            self.open_entry_gate()?;
            if let Some(item) = self.scheduler.take_ready() {
                item.run(self);
                continue;
            }
            if self.scheduler.defer_low_priority() {
                return Ok(Outcome::Yield);
            }
            break;
        }
        self.poll_host_tasks(waker)?;
        if self.scheduler.has_ready_item() {
            return Ok(Outcome::Progress);
        }
        if self.scheduler.host_task_count() == 0 {
            return Ok(Outcome::Idle);
        }
        Ok(Outcome::Waiting)
    }

    /// Let through every task the entry gate can release. Costs
    /// nothing while no task waits, which is every turn of the
    /// synchronous baseline.
    fn open_entry_gate(&mut self) -> Result<()> {
        if self.scheduler.waiting_at_gate() == 0 {
            return Ok(());
        }
        let mut guard = Self::lock_handle(&self.tables)?;
        self.scheduler.open_entry_gate(&mut guard.tasks);
        Ok(())
    }

    /// Poll every host task the store holds with the turn's waker.
    /// A host task that joined since the last turn counts as woken,
    /// and the waker the executor holds is the driver's, so polling
    /// them all is what "the ones the executor woke" comes to while
    /// one waker serves the whole store. A future that completes
    /// leaves its value in the slot its caller supplied and queues
    /// the lowering of that value into the subtask that awaits it.
    fn poll_host_tasks(&mut self, waker: &Waker) -> Result<()> {
        let mut tasks = self.scheduler.take_host_tasks();
        if tasks.is_empty() {
            return Ok(());
        }
        let mut pending = Vec::with_capacity(tasks.len());
        let mut completed = Vec::new();
        for mut task in tasks.drain(..) {
            match task.poll(waker) {
                Poll::Ready(value) => {
                    completed.push((task.subtask(), task.handle_index(), task.result(), value));
                }
                Poll::Pending => pending.push(task),
            }
        }
        self.scheduler.restore_host_tasks(pending);
        for (subtask, handle_index, slot, value) in completed {
            self.scheduler.push_high_priority(Item::new(
                ItemKind::HostResultLowering,
                move |store: &mut Self| {
                    if let Ok(mut held) = slot.lock() {
                        *held = Some(value);
                    }
                    let Ok(mut guard) = store.tables.lock() else {
                        return;
                    };
                    // The subtask has returned, and its readiness is
                    // the subtask event a thread waiting on it takes
                    // delivery of. The event carries the subtask's
                    // index in the caller instance's handle table and
                    // the state it moved to.
                    if guard.tasks.subtask_returned(subtask).is_ok() {
                        let _ = guard.tasks.record_subtask_event(subtask, handle_index);
                    }
                },
            ));
        }
        Ok(())
    }

    /// Create the task of one call into an export, without making it
    /// the current scope: the task's thread runs when a turn runs the
    /// item that starts it. Workspace-internal.
    pub fn create_export_task(
        &self,
        function: FunctionType,
        options: CanonOptions,
        instance: InstanceId,
    ) -> Result<TaskId> {
        Ok(self
            .lock_tables()?
            .tasks
            .create_task(Some(function), Some(options), instance))
    }

    /// Queue `item` as the start of `task`'s implicit thread, past
    /// the entry gate of `instance`. Workspace-internal.
    pub fn start_export_thread(
        &mut self,
        task: TaskId,
        instance: InstanceId,
        async_function: bool,
        needs_exclusive: bool,
        item: Item<T>,
    ) -> Result<()> {
        let mut guard = Self::lock_handle(&self.tables)?;
        self.scheduler.enter_implicit_thread(
            &mut guard.tasks,
            task,
            instance,
            async_function,
            needs_exclusive,
            item,
        );
        Ok(())
    }

    /// Make an export's task the current scope, as its thread starts
    /// to run. Workspace-internal.
    pub fn enter_export_task(&self, task: TaskId) -> Result<()> {
        self.lock_tables()?.tasks.push_task_scope(task);
        Ok(())
    }

    /// Mark an export's task started: its thread is about to run.
    /// Workspace-internal.
    pub fn start_export_task(&self, task: TaskId) -> Result<()> {
        self.lock_tables()?.tasks.start_task(task);
        Ok(())
    }

    /// Resolve an export's task with the result it returned, which
    /// the caller on the stack takes as the call returns.
    /// Workspace-internal.
    pub fn resolve_export_task(&self, task: TaskId, result: Option<Val>) -> Result<()> {
        if let Some(record) = self.lock_tables()?.tasks.task_mut(task) {
            record.resolve(result);
        }
        Ok(())
    }

    /// Pop the export's task on its success path, together with any
    /// scope left above it. The inner `Err` carries the count of
    /// borrows the guest did not drop. Workspace-internal.
    pub fn exit_export_task(&self, task: TaskId) -> Result<core::result::Result<(), u32>> {
        Ok(self.lock_tables()?.exit_task(task))
    }

    /// Pop the export's task on its failure path, with no borrow
    /// check. Every scope the failure left above the task — the task
    /// of a callee that trapped, the subtask of a host call that
    /// failed — is popped with it, and the lends of each are given
    /// back. Workspace-internal.
    pub fn abandon_export_task(&self, task: TaskId) -> Result<()> {
        self.lock_tables()?.abandon_task(task);
        Ok(())
    }

    /// Lock the store's handle tables and record state.
    fn lock_tables(&self) -> Result<MutexGuard<'_, HandleTables>> {
        Self::lock_handle(&self.tables)
    }

    /// Lock the store's handle tables through the shared handle, so
    /// that the caller keeps the rest of the store borrowable. The
    /// turn needs this: it holds the scheduler's queues mutably
    /// while it reads the instance records.
    fn lock_handle(tables: &Arc<Mutex<HandleTables>>) -> Result<MutexGuard<'_, HandleTables>> {
        tables
            .lock()
            .map_err(|_| Error::internal("resource handle tables lock poisoned"))
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    use crate::concurrency::{HostTask, Item, ItemKind};
    use crate::engine::Engine;

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
        store.scheduler.push_high_priority(Item::new(
            ItemKind::TaskStart,
            move |_store: &mut Store<()>| {
                counted.fetch_add(1, AtomicOrdering::Relaxed);
            },
        ));
        let subtask = store.lock_tables().expect("tables").tasks.push_subtask();
        store.push_host_task(HostTask::new(
            subtask,
            0,
            Arc::new(Mutex::new(None)),
            core::future::pending::<Result<Vec<Val>>>(),
        ));
        assert_eq!(store.scheduler.queued_items(), 1);
        assert_eq!(store.scheduler.host_task_count(), 1);

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
