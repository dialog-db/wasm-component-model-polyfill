//! The polyfill's owner of guest state.
//!
//! `Store<T>` carries host data of type `T` and is the unit of
//! isolation between independent component instances: the
//! polyfill's analogue to `wasmtime::Store`. The store also owns the
//! handle tables the canonical-ABI runtime-state rules require — one
//! per component instance, shared by every handle kind that instance
//! uses, plus one per resource type for the host's own handles — and
//! carries a process-unique identity so that an instance can refuse a
//! call made through a different store.

use core::task::{Poll, Waker};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use wasm_runtime_layer::Val as RuntimeVal;

use crate::backend::Backend;
use crate::component::FunctionType;
use crate::concurrency::{
    Accessor, CallStatus, HostTask, InstanceId, Item, LowerKind, Outcome, Scheduler, SubtaskState,
    TaskId, TurnGuard, YieldWake,
};
use crate::engine::Engine;
use crate::error::{AbiCause, AbiError, AbiPosition, Error, Result, SchedulerCause};
use crate::executor::ResourceDestructor;
use crate::executor::ir::CanonOptions;
use crate::resource::{HandleLookupError, HandleTables, ResourceHandle, ResourceTypeId, TableId};
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
    /// The store's handle tables: one per component instance, shared
    /// by every handle kind, plus one per resource type for the
    /// host's own handles. The `Arc<Mutex<...>>` shape
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
    /// and a host task's body need not be `Send` in the browser,
    /// so neither can live behind the tables lock; the half a
    /// trampoline reaches — the waker of the running turn and
    /// whether one is running — sits in the tables instead.
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
        let _turn = TurnGuard::enter(&self.tables, waker);
        self.run_turn(waker, false)
    }

    /// Whether a turn of this store is running. A driver entered
    /// while another driver of the same store is inside a turn fails
    /// with the recursive-driver cause. Workspace-internal.
    pub fn turn_in_flight(&self) -> Result<bool> {
        Ok(self.lock_tables()?.scheduler.in_turn())
    }

    /// Whether the store holds work only a turn can carry forward:
    /// an item in one of the ready queues, deferred work included,
    /// or a host task that has not resolved. A driver that parked on
    /// such a store would wait for a wake that the work it parked on
    /// is what produces. Workspace-internal.
    pub fn has_pending_work(&self) -> bool {
        self.scheduler.has_ready_item() || self.scheduler.host_task_count() > 0
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
        // The refusal happens before the accessor exists, so a
        // refused entry leaves the store untouched.
        if self.turn_in_flight()? {
            return Err(Error::Scheduler(SchedulerCause::RecursiveDriver));
        }

        let accessor = Accessor::new(self);
        let mut future = core::pin::pin!(body(&accessor));
        let mut yield_wake: Option<YieldWake> = None;

        core::future::poll_fn(|context| {
            let waker = context.waker();

            // A turn that ended in a yield returns control to the
            // host executor before the item that yielded runs.
            if let Some(wake) = &yield_wake {
                if !wake.landed() {
                    return Poll::Pending;
                }
                yield_wake = None;
            }
            if let Err(error) = accessor.attend(waker) {
                return Poll::Ready(Err(error));
            }

            loop {
                if let Poll::Ready(value) = future.as_mut().poll(context) {
                    return Poll::Ready(Ok(value));
                }
                let outcome = match accessor.lend(|store| store.turn(waker)) {
                    Ok(Ok(outcome)) => outcome,
                    Ok(Err(error)) | Err(error) => return Poll::Ready(Err(error)),
                };
                match outcome {
                    Outcome::Progress => continue,
                    Outcome::Yield => {
                        yield_wake = Some(YieldWake::after_yield(waker));
                        return Poll::Pending;
                    }
                    // A turn that leaves a host task pending can
                    // have completed the closure's future all the
                    // same: it runs the items that are ready before
                    // it polls the host tasks. The future is
                    // therefore polled once more before the entry
                    // parks, for the reason the other drivers
                    // consult their condition once more — a closure
                    // that is done would otherwise wait on a host
                    // task it does not wait for, and against one
                    // that never returns it would wait for ever.
                    Outcome::Waiting => {
                        if let Poll::Ready(value) = future.as_mut().poll(context) {
                            return Poll::Ready(Ok(value));
                        }
                        return Poll::Pending;
                    }
                    // An idle store is not a deadlock here: what
                    // `body` waits on can be outside the store. The
                    // closure's future is polled once more first,
                    // for the same reason the other drivers consult
                    // their condition once more: the turn that has
                    // just run is what it was waiting for.
                    Outcome::Idle => {
                        if let Poll::Ready(value) = future.as_mut().poll(context) {
                            return Poll::Ready(Ok(value));
                        }
                        // That poll can have left work in the store:
                        // the closure reaches the store through its
                        // accessor, and what it queues there or
                        // starts there is work only a turn runs.
                        // Parking on it would wait for a wake that
                        // the parked work is what produces.
                        match accessor.lend(|store| store.has_pending_work()) {
                            Ok(true) => continue,
                            Ok(false) => return Poll::Pending,
                            Err(error) => return Poll::Ready(Err(error)),
                        }
                    }
                }
            }
        })
        .await
    }

    /// Run `body` inside a turn of this store, so that the guest
    /// code it reaches runs where every other piece of guest code
    /// runs. Instantiation uses this for the initializers of the
    /// plan: they are not queued items, because they run against
    /// borrowed plan state that no item could hold, but they are
    /// still guest work and belong inside a turn. An [`Accessor`]
    /// uses it for the closure it runs, which is guest work for the
    /// same reason and which a driver must not be entered from.
    /// Workspace-internal.
    pub fn run_in_turn<R>(
        &mut self,
        waker: &Waker,
        body: impl FnOnce(&mut Self) -> R,
    ) -> Result<R> {
        let _turn = TurnGuard::enter(&self.tables, waker);
        Ok(body(self))
    }

    /// Run one nested turn of this store's scheduler: the fallback
    /// of the suspend seam, run from inside the guest call that
    /// blocked.
    ///
    /// A nested turn is not a driver. It neither consults the count
    /// of running turns nor raises it, so the recursive-driver rule
    /// does not apply to it, and it takes the waker of the outer
    /// turn rather than recording one of its own, so a host task it
    /// leaves pending carries the waker the executor already holds.
    /// That count is the nesting a host task body raises when it
    /// reaches the store through its accessor, which is a different
    /// thing: the suspend seam's own documentation sets the two side
    /// by side.
    ///
    /// It also leaves every resumption after a yield alone: it
    /// neither takes the resume-after-yield slot nor fills it. Such
    /// a resumption runs only after a driver has returned control to
    /// the host executor, and a nested turn runs from inside a guest
    /// call, so it has no control to return. It reports deferred work
    /// it cannot run as [`Outcome::Yield`], which is the outer turn's
    /// cue to end and the seam's cue to stop.
    /// Workspace-internal.
    pub fn nested_turn(&mut self, waker: &Waker) -> Result<Outcome> {
        self.run_turn(waker, true)
    }

    /// The waker of the turn that is running, or a waker that does
    /// nothing when no turn is running — which is the case for a
    /// thread resumed outside any poll of a driver.
    ///
    /// Two callers want it. A trampoline that starts a host task
    /// polls its body once before it returns to the guest, and a
    /// body polled outside a turn counts as woken all the same,
    /// because the next turn polls every host task the store holds.
    /// The suspend seam's nested turn wants it for the same reason:
    /// it polls with the waker the outer turn recorded rather than
    /// recording one of its own, so a host task it leaves pending
    /// carries the waker the executor already holds.
    /// Workspace-internal.
    pub fn active_waker(&self) -> Waker {
        self.tables
            .lock()
            .ok()
            .and_then(|guard| guard.scheduler.active_waker())
            .unwrap_or_else(|| Waker::noop().clone())
    }

    /// Why a driver that went idle failed: the cannot-block cause
    /// when the task it waits on is one that must not block, and the
    /// deadlock cause otherwise. Workspace-internal.
    pub fn idle_cause(&self, task: Option<TaskId>) -> SchedulerCause {
        if self.must_not_block(task) {
            SchedulerCause::CannotBlock
        } else {
            SchedulerCause::Deadlock
        }
    }

    /// Why a nested turn that went idle with its condition unmet
    /// failed: the cannot-block cause when the current task is one
    /// that must not block, which is the rule of the reference, and
    /// the stack-switch cause otherwise, because the reference
    /// permits that block and only the target cannot serve it.
    /// Workspace-internal.
    pub fn suspend_cause(&self) -> SchedulerCause {
        let current = self
            .tables
            .lock()
            .ok()
            .and_then(|guard| guard.tasks.current_task());
        if self.must_not_block(current) {
            SchedulerCause::CannotBlock
        } else {
            SchedulerCause::StackSwitchNeeded
        }
    }

    /// Whether `task` runs in an instance that forbids its threads
    /// to suspend. A store whose tables are unreachable answers
    /// `false`, so a poisoned lock never turns a deadlock into a
    /// cannot-block.
    fn must_not_block(&self, task: Option<TaskId>) -> bool {
        let Ok(guard) = self.tables.lock() else {
            return false;
        };
        task.and_then(|task| guard.tasks.task(task))
            .and_then(|record| guard.tasks.instance(record.instance))
            .map(|record| record.may_not_suspend)
            .unwrap_or(false)
    }

    /// Start the host task of one call of a host `async` function,
    /// and report what the guest is told.
    ///
    /// `task` carries the body of the call, the subtask the caller
    /// pushed for it, and the lowering that takes what the body
    /// produces into the guest. `caller` is the handle table of the
    /// instance that made the call, where a subtask the guest has to
    /// wait on gets its entry. `lower` is the lowering the guest
    /// called through.
    ///
    /// The body is polled once here, with the waker of the turn that
    /// is running, or with a waker that does nothing when no turn is:
    /// a host task that joined the store counts as woken, so the next
    /// turn polls it again with the driver's waker and no wake is
    /// lost. A body that resolves at once has its result lowered
    /// here, and the guest sees the returned status with no subtask
    /// behind it. A body that is still running joins the store's host
    /// tasks, its subtask enters `caller` in the started state, and
    /// the guest sees the started status carrying that index.
    ///
    /// A body that fails is a call that never returned: its subtask
    /// resolves as a cancellation, the handles the guest lent for it
    /// go back, and the failure travels out to the call the guest
    /// still has on the stack.
    ///
    /// Through a synchronous lower the guest expects the result when
    /// the call returns, so a body that is still running would have
    /// to block the guest thread where it stands. That needs the
    /// suspend seam, and no target fills it today, so the call fails
    /// with the stack-switch cause and the subtask is given back.
    ///
    /// Workspace-internal; not re-exported by `lib.rs`.
    pub fn start_host_task(
        &mut self,
        mut task: HostTask<T>,
        caller: TableId,
        lower: LowerKind,
    ) -> Result<CallStatus> {
        let subtask = task.subtask();
        let waker = self.active_waker();
        let outcome = {
            // The body reaches the host data through this accessor
            // and through nothing else, for the length of a closure
            // it runs with it.
            let accessor = Accessor::new(&mut *self);
            accessor.attend(&waker)?;
            task.poll(&accessor, &waker)
        };

        match outcome {
            Poll::Ready(Ok(values)) => {
                // The call is over, so the subtask leaves the stack
                // and gives back the handles the guest lent for it
                // before the result crosses: a borrow lowered back
                // out belongs to the caller's task.
                Self::lock_handle(&self.tables)?.exit_subtask(subtask, SubtaskState::Returned);
                task.lower(self, Ok(values))?;
                Ok(CallStatus::returned())
            }
            // The call never returned, so the subtask's resolution
            // is a cancellation, and the handles the guest lent for
            // it are given back all the same. The guest's call is on
            // the stack, so the failure travels out to it and
            // nothing crosses.
            Poll::Ready(Err(error)) => {
                Self::lock_handle(&self.tables)?.abandon_subtask(subtask);
                Err(error)
            }
            Poll::Pending => match lower {
                LowerKind::Sync => {
                    Self::lock_handle(&self.tables)?.abandon_subtask(subtask);
                    Err(Error::Scheduler(SchedulerCause::StackSwitchNeeded))
                }
                LowerKind::Async => {
                    let index = {
                        let mut guard = Self::lock_handle(&self.tables)?;
                        let index = guard.insert_subtask(caller, subtask);
                        guard.tasks.start_subtask(subtask);
                        // The guest runs on while the host side does,
                        // so the subtask is no longer the scope the
                        // guest's work counts against. Its record
                        // stays, and with it the handles the call
                        // borrowed, until its resolution is
                        // delivered.
                        if guard.tasks.current_subtask() == Some(subtask) {
                            guard.tasks.pop_scope();
                        }
                        index
                    };
                    task.set_handle_index(index);
                    self.push_host_task(task);
                    Ok(CallStatus::started(index))
                }
            },
        }
    }

    /// Give a host task to the store. The next turn polls it with
    /// the driver's waker, so no wake is lost. Workspace-internal.
    pub fn push_host_task(&mut self, task: HostTask<T>) {
        self.scheduler.push_host_task(task);
    }

    /// The body of one turn, with the waker already recorded.
    ///
    /// `nested` marks the turn the suspend seam runs from inside a
    /// guest call. Only a turn that is not nested touches the
    /// resumptions after a yield, because only a driver's turn can
    /// end and hand control back to the host executor first.
    ///
    /// An item that fails ends the turn and its failure is the
    /// turn's. Almost no item can fail: what an item produces it
    /// leaves in the store, and the failure of the call it ran is
    /// part of that. The ones that can are the items whose own
    /// bookkeeping failed, which belongs to no caller.
    fn run_turn(&mut self, waker: &Waker, nested: bool) -> Result<Outcome> {
        self.open_entry_gate()?;
        if !nested && let Some(item) = self.scheduler.take_resume_after_yield() {
            item.run(self)?;
        }
        loop {
            self.open_entry_gate()?;
            if let Some(item) = self.scheduler.take_ready() {
                item.run(self)?;
                continue;
            }
            if !nested && self.scheduler.defer_low_priority() {
                return Ok(Outcome::Yield);
            }
            break;
        }
        self.poll_host_tasks(waker)?;
        if self.scheduler.has_immediate_item() {
            return Ok(Outcome::Progress);
        }
        // Only deferred work is left. A nested turn reaches this and
        // gives way: the resumption belongs to the outer turn, after
        // the driver has returned control to the host executor. A
        // driver's turn does not reach it, because the loop above
        // deferred its front and returned already.
        if self.scheduler.has_deferred_item() {
            return Ok(Outcome::Yield);
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
    /// one waker serves the whole store. Each poll is handed an
    /// accessor to this store, which is how a body that has to read
    /// the host data reaches it. A body that completes queues the
    /// lowering of what it produced into the subtask that awaits it.
    fn poll_host_tasks(&mut self, waker: &Waker) -> Result<()> {
        let mut tasks = self.scheduler.take_host_tasks();
        if tasks.is_empty() {
            return Ok(());
        }
        let mut pending = Vec::with_capacity(tasks.len());
        let mut completed = Vec::new();
        {
            let accessor = Accessor::new(&mut *self);
            accessor.attend(waker)?;
            for mut task in tasks.drain(..) {
                match task.poll(&accessor, waker) {
                    Poll::Ready(value) => completed.push((task, value)),
                    Poll::Pending => pending.push(task),
                }
            }
        }
        self.scheduler.restore_host_tasks(pending);
        for (task, value) in completed {
            self.scheduler.push_high_priority(task.lowering_item(value));
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
    ///
    /// The task's implicit thread ends first. That is the reference's
    /// `exit_implicit_thread`: the thread the task's call ran on is
    /// over, so the instance it held exclusively, if it held one,
    /// goes back, and the next task waiting at that instance's entry
    /// gate can take it. The release belongs to this exit rather than
    /// to a call of its own because it reads the task record — the
    /// thread it names and the instance that thread holds — and this
    /// exit is what removes that record. A release that ran after the
    /// record was gone would find nothing and give nothing back, and
    /// the gate would stay shut for the life of the instance.
    pub fn exit_export_task(&self, task: TaskId) -> Result<core::result::Result<(), u32>> {
        let mut guard = Self::lock_handle(&self.tables)?;
        self.scheduler.exit_implicit_thread(&mut guard.tasks, task);
        Ok(guard.exit_task(task))
    }

    /// Pop the export's task on its failure path, with no borrow
    /// check. Every scope the failure left above the task — the task
    /// of a callee that trapped, the subtask of a host call that
    /// failed — is popped with it, and the lends of each are given
    /// back. The task's implicit thread ends first, for the reason
    /// [`exit_export_task`](Self::exit_export_task) gives: a call
    /// that failed gives the instance back exactly as one that
    /// returned does. Workspace-internal.
    pub fn abandon_export_task(&self, task: TaskId) -> Result<()> {
        let mut guard = Self::lock_handle(&self.tables)?;
        self.scheduler.exit_implicit_thread(&mut guard.tasks, task);
        guard.abandon_task(task);
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
    use core::future::Future;
    use core::pin::Pin;
    use core::task::Context;

    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    use crate::concurrency::{Driver, Event, HostTask, Item, ItemKind, SubtaskId, WaitableId};
    use crate::engine::Engine;
    use crate::resource::HandleKind;

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
                Ok(())
            },
        ));
        let subtask = store.lock_tables().expect("tables").tasks.push_subtask();
        store.push_host_task(HostTask::from_future(
            subtask,
            |_store: &mut Store<()>, _outcome: Result<Vec<Val>>| Ok(()),
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

    /// A future that resolves when the host resolves it: something
    /// outside the store for a `run_concurrent` closure to wait on.
    #[derive(Clone, Default)]
    struct Outside(Arc<Mutex<OutsideState>>);

    /// Whether [`Outside`] has resolved, and the waker of whoever
    /// waits on it.
    #[derive(Default)]
    struct OutsideState {
        resolved: bool,
        waker: Option<Waker>,
    }

    impl Outside {
        /// Resolve the future and wake whoever waits on it.
        fn resolve(&self) {
            let waker = {
                let mut state = self.0.lock().expect("outside");
                state.resolved = true;
                state.waker.take()
            };
            if let Some(waker) = waker {
                waker.wake();
            }
        }
    }

    impl Future for Outside {
        type Output = ();

        fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
            let mut state = self.0.lock().expect("outside");
            if state.resolved {
                return Poll::Ready(());
            }
            state.waker = Some(context.waker().clone());
            Poll::Pending
        }
    }

    /// A condition that is never met, so the driver runs until the
    /// scheduler goes idle or gives way.
    fn never(_store: &mut Store<()>, _waker: &Waker) -> Option<Result<()>> {
        None
    }

    /// Poll `future` once, as an executor would.
    fn poll_once<F: Future>(future: &mut Pin<Box<F>>, waker: &Waker) -> Poll<F::Output> {
        let mut context = Context::from_waker(waker);
        future.as_mut().poll(&mut context)
    }

    #[wcmp_macros::test]
    async fn it_returns_pending_when_the_store_is_idle_and_the_closure_waits_outside_it() {
        let engine = Engine::new().expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");
        let outside = Outside::default();
        let awaited = outside.clone();

        let mut entry = Box::pin(store.run_concurrent(async move |_accessor| {
            awaited.await;
            "resolved"
        }));

        assert!(
            poll_once(&mut entry, Waker::noop()).is_pending(),
            "nothing is ready and no host task is pending, but the closure \
             waits on something outside the store, so the entry returns \
             pending rather than the deadlock cause"
        );

        outside.resolve();

        let Poll::Ready(value) = poll_once(&mut entry, Waker::noop()) else {
            panic!("the entry is still pending after the outside future resolved");
        };
        assert_eq!(
            value.expect("run the closure"),
            "resolved",
            "the entry completes when the future outside the store resolves"
        );
    }

    #[wcmp_macros::test]
    async fn it_runs_the_item_the_closure_queued_as_the_store_went_idle() {
        let engine = Engine::new().expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");

        // The closure queues the item on the poll the entry makes
        // when the turn before it went idle, and nothing outside the
        // store ever wakes it: the item it queued is the only thing
        // that resolves it, and only a turn runs an item.
        let mut entry = Box::pin(store.run_concurrent(async |accessor| {
            let ran = Arc::new(AtomicUsize::new(0));
            let mut polls = 0usize;
            core::future::poll_fn(move |_context| {
                polls += 1;
                if ran.load(AtomicOrdering::Relaxed) > 0 {
                    return Poll::Ready("the item ran");
                }
                if polls == 2 {
                    let counted = ran.clone();
                    accessor
                        .with(|store: &mut Store<()>| {
                            store.scheduler.push_high_priority(Item::new(
                                ItemKind::TaskStart,
                                move |_store: &mut Store<()>| {
                                    counted.fetch_add(1, AtomicOrdering::Relaxed);
                                    Ok(())
                                },
                            ));
                        })
                        .expect("queue the item");
                }
                Poll::Pending
            })
            .await
        }));

        let Poll::Ready(value) = poll_once(&mut entry, Waker::noop()) else {
            panic!("the entry parked on a store that held a ready item");
        };
        assert_eq!(
            value.expect("run the closure"),
            "the item ran",
            "the entry ran the item rather than park on it"
        );
    }

    #[wcmp_macros::test]
    async fn it_polls_the_host_task_the_closure_started_as_the_store_went_idle() {
        let engine = Engine::new().expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");

        // As above, with a host task in place of the item: what
        // resolves the closure is the lowering of the body's result,
        // and only a turn polls a host task.
        let mut entry = Box::pin(store.run_concurrent(async |accessor| {
            let lowered = Arc::new(AtomicUsize::new(0));
            let mut polls = 0usize;
            core::future::poll_fn(move |_context| {
                polls += 1;
                if lowered.load(AtomicOrdering::Relaxed) > 0 {
                    return Poll::Ready("the body's result crossed");
                }
                if polls == 2 {
                    let counted = lowered.clone();
                    accessor
                        .with(|store: &mut Store<()>| {
                            let subtask =
                                store.lock_tables().expect("tables").tasks.insert_subtask();
                            store.push_host_task(HostTask::from_future(
                                subtask,
                                move |_store: &mut Store<()>, _outcome: Result<Vec<Val>>| {
                                    counted.fetch_add(1, AtomicOrdering::Relaxed);
                                    Ok(())
                                },
                                core::future::ready(Ok(vec![Val::U32(3)])),
                            ));
                        })
                        .expect("start the host task");
                }
                Poll::Pending
            })
            .await
        }));

        let Poll::Ready(value) = poll_once(&mut entry, Waker::noop()) else {
            panic!("the entry parked on a store that held a pending host task");
        };
        assert_eq!(
            value.expect("run the closure"),
            "the body's result crossed",
            "the entry polled the host task rather than park on it"
        );
    }

    #[wcmp_macros::test]
    async fn it_returns_when_the_turn_that_completed_the_closure_left_a_host_task_pending() {
        let engine = Engine::new().expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");

        // A host task that never returns, and an item that completes
        // the closure. A turn runs the items that are ready before it
        // polls the host tasks, so the closure is done by the time the
        // turn reports that a host task is still pending — and the
        // entry must not park on a host task it does not wait for.
        let subtask = store.lock_tables().expect("tables").tasks.insert_subtask();
        store.push_host_task(HostTask::from_future(
            subtask,
            |_store: &mut Store<()>, _outcome: Result<Vec<Val>>| Ok(()),
            core::future::pending::<Result<Vec<Val>>>(),
        ));
        let ran = Arc::new(AtomicUsize::new(0));
        let counted = ran.clone();
        store.scheduler.push_high_priority(Item::new(
            ItemKind::TaskStart,
            move |_store: &mut Store<()>| {
                counted.fetch_add(1, AtomicOrdering::Relaxed);
                Ok(())
            },
        ));

        let watched = ran.clone();
        let mut entry = Box::pin(store.run_concurrent(async move |_accessor| {
            core::future::poll_fn(move |_context| {
                if watched.load(AtomicOrdering::Relaxed) > 0 {
                    Poll::Ready("the item ran")
                } else {
                    Poll::Pending
                }
            })
            .await
        }));

        let Poll::Ready(value) = poll_once(&mut entry, Waker::noop()) else {
            panic!("the entry parked on a host task that never returns");
        };
        assert_eq!(
            value.expect("run the closure"),
            "the item ran",
            "the turn ran the item that completed the closure before it polled \
             the host tasks, so the entry returns instead of waiting on a host \
             task that never returns"
        );
    }

    /// Run `body` and catch the panic it is expected to unwind
    /// with, keeping the report of that panic out of the test's
    /// output.
    #[cfg(not(target_arch = "wasm32"))]
    fn unwind<R>(body: impl FnOnce() -> R) -> std::thread::Result<R> {
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
        std::panic::set_hook(hook);
        outcome
    }

    // The browser target aborts on a panic instead of unwinding, so
    // there is nothing to catch there and the test is native only.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn it_ends_the_turn_an_item_panicked_out_of() {
        let engine = Engine::new().expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");
        store.scheduler.push_high_priority(Item::new(
            ItemKind::TaskStart,
            |_store: &mut Store<()>| -> Result<()> { panic!("the item panicked") },
        ));

        let unwound = unwind(|| store.turn(Waker::noop()));

        assert!(unwound.is_err(), "the item's panic unwound the turn");
        assert!(
            !store.turn_in_flight().expect("the store's turn state"),
            "the turn the panic unwound out of is over"
        );
        let mut driver = Box::pin(Driver::new(&mut store, None, |_store, _waker| Some(Ok(()))));
        assert!(
            matches!(poll_once(&mut driver, Waker::noop()), Poll::Ready(Ok(()))),
            "a driver entered after the panic is not refused"
        );
    }

    // Native only, for the reason the test above is: the browser
    // aborts on a panic, so no panic ever reaches a lock there.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn it_takes_the_tables_back_from_a_panic_that_held_their_lock() {
        let engine = Engine::new().expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");
        store.scheduler.push_high_priority(Item::new(
            ItemKind::TaskStart,
            |store: &mut Store<()>| -> Result<()> {
                let _tables = store.lock_tables().expect("tables");
                panic!("the item panicked with the tables locked")
            },
        ));

        let unwound = unwind(|| store.turn(Waker::noop()));

        assert!(unwound.is_err(), "the item's panic unwound the turn");
        assert!(
            store.lock_tables().is_ok(),
            "the turn's guard took the tables back from the poison the panic \
             left, so the store is not refusing every later reader"
        );
        let mut driver = Box::pin(Driver::new(&mut store, None, |_store, _waker| Some(Ok(()))));
        assert!(
            matches!(poll_once(&mut driver, Waker::noop()), Poll::Ready(Ok(()))),
            "a driver entered after the panic is not refused"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn it_enters_a_turn_on_tables_a_panic_outside_the_store_poisoned() {
        let engine = Engine::new().expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");

        // A panic taken with the lock held and no turn in flight, so
        // what meets the poison is the guard on its way in rather
        // than on its way out.
        let tables = store.tables.clone();
        let poisoned = unwind(move || {
            let _tables = tables.lock().expect("tables");
            panic!("the host panicked with the tables locked")
        });
        assert!(poisoned.is_err(), "the panic unwound");
        assert!(store.tables.is_poisoned(), "and poisoned the lock");

        let outcome = store.turn(Waker::noop());

        assert!(
            outcome.is_ok(),
            "the turn's guard took the tables back from the poison on its way in"
        );
        assert!(
            !store.tables.is_poisoned(),
            "and cleared it, so every later reader of the store reaches them too"
        );
    }

    #[wcmp_macros::test]
    async fn it_runs_an_item_an_earlier_driver_left_in_the_store() {
        let engine = Engine::new().expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");
        let outside = Outside::default();
        let signal = outside.clone();
        let item_runs = Arc::new(AtomicUsize::new(0));
        let counted = item_runs.clone();
        store.scheduler.push_low_priority(Item::new(
            ItemKind::TaskStart,
            move |_store: &mut Store<()>| {
                counted.fetch_add(1, AtomicOrdering::Relaxed);
                signal.resolve();
                Ok(())
            },
        ));

        {
            // A driver the item gave way to, dropped before the item
            // ran. Dropping it cancels nothing.
            let mut abandoned = Box::pin(Driver::new(&mut store, None, never));
            assert!(
                poll_once(&mut abandoned, Waker::noop()).is_pending(),
                "the turn gave way, so the driver returns pending"
            );
        }
        assert_eq!(
            item_runs.load(AtomicOrdering::Relaxed),
            0,
            "the item that gave way has not run yet"
        );

        store
            .run_concurrent(async move |_accessor| {
                outside.await;
            })
            .await
            .expect("run the closure");

        assert_eq!(
            item_runs.load(AtomicOrdering::Relaxed),
            1,
            "the entry ran the item the earlier driver left in the store"
        );
    }

    /// Where a test's lowering leaves what it was handed. The
    /// lowering of a real call takes the value across into the
    /// guest's memory; a test only has to see that it ran, with
    /// what, and when.
    type Lowered = Arc<Mutex<Option<Result<Vec<Val>>>>>;

    /// A lowering that records what it was handed.
    fn recording(
        slot: &Lowered,
    ) -> impl FnOnce(&mut Store<()>, Result<Vec<Val>>) -> Result<()> + Send + 'static {
        let slot = slot.clone();
        move |_store, outcome| {
            *slot.lock().expect("the lowering's slot") = Some(outcome);
            Ok(())
        }
    }

    /// What a test's lowering was handed, or a panic when it has not
    /// run yet.
    fn lowered(slot: &Lowered) -> Vec<Val> {
        slot.lock()
            .expect("the lowering's slot")
            .take()
            .expect("the lowering ran")
            .expect("the host task's value")
    }

    /// The store, the calling instance's handle table, and the
    /// subtask a host call pushes before it starts: the state a
    /// trampoline has built by the time it starts a host task.
    fn host_call(engine: &Engine) -> (Store<()>, TableId, SubtaskId) {
        let store = Store::new(engine, ()).expect("store");
        let subtask = store.lock_tables().expect("tables").tasks.push_subtask();
        (store, TableId::fresh(), subtask)
    }

    #[wcmp_macros::test]
    async fn it_lowers_the_result_at_once_when_the_first_poll_resolves_the_future() {
        let engine = Engine::new().expect("engine");
        let (mut store, table, subtask) = host_call(&engine);
        let slot: Lowered = Arc::new(Mutex::new(None));

        let status = store
            .start_host_task(
                HostTask::from_future(
                    subtask,
                    recording(&slot),
                    core::future::ready(Ok(vec![Val::U32(7)])),
                ),
                table,
                LowerKind::Async,
            )
            .expect("start the host task");

        assert_eq!(
            status,
            CallStatus::returned(),
            "a call whose future resolved at once returns to the guest"
        );
        assert_eq!(
            status.subtask_index(),
            None,
            "a call that returned at once leaves no subtask behind"
        );
        assert_eq!(
            lowered(&slot),
            vec![Val::U32(7)],
            "the result crossed into the guest before the call returned"
        );
        assert_eq!(
            store.scheduler.host_task_count(),
            0,
            "nothing joined the store's host tasks"
        );
        let guard = store.lock_tables().expect("tables");
        assert!(
            guard.tasks.subtask(subtask).is_none(),
            "the subtask resolved and left the store"
        );
        assert!(
            guard.entry(table, 0).is_none(),
            "no subtask entered the caller's handle table"
        );
    }

    #[wcmp_macros::test]
    async fn it_starts_a_subtask_for_a_pending_future_and_lowers_its_result_in_a_later_turn() {
        let engine = Engine::new().expect("engine");
        let (mut store, table, subtask) = host_call(&engine);
        let slot: Lowered = Arc::new(Mutex::new(None));
        let outside = Outside::default();
        let awaited = outside.clone();

        let status = store
            .start_host_task(
                HostTask::from_future(subtask, recording(&slot), async move {
                    awaited.await;
                    Ok(vec![Val::U32(9)])
                }),
                table,
                LowerKind::Async,
            )
            .expect("start the host task");

        assert_eq!(
            status.state(),
            SubtaskState::Started.value(),
            "a call whose future is still running is a started subtask"
        );
        let index = status
            .subtask_index()
            .expect("a started call carries the index of its subtask");
        {
            let guard = store.lock_tables().expect("tables");
            assert_eq!(
                guard.tasks.subtask(subtask).map(|record| record.state),
                Some(SubtaskState::Started),
                "the subtask record is in its started state"
            );
            assert_eq!(
                guard.entry(table, index),
                Some(HandleKind::Subtask { subtask }),
                "the status names the entry the subtask took in the caller's table"
            );
        }
        assert_eq!(
            store.scheduler.host_task_count(),
            1,
            "the future joined the store's host tasks"
        );
        assert!(
            slot.lock().expect("the lowering's slot").is_none(),
            "nothing has crossed into the guest yet"
        );

        outside.resolve();

        // The turn that sees the future complete queues the
        // lowering, and the turn after it runs the item.
        assert_eq!(
            store.turn(Waker::noop()).expect("a turn"),
            Outcome::Progress,
            "the completed host task left an item ready to run"
        );
        store.turn(Waker::noop()).expect("a turn");

        assert_eq!(
            lowered(&slot),
            vec![Val::U32(9)],
            "the result crossed in the turn that ran the lowering"
        );
        let mut guard = store.lock_tables().expect("tables");
        assert_eq!(
            guard.tasks.subtask(subtask).map(|record| record.state),
            Some(SubtaskState::Returned),
            "the subtask moved to its returned state"
        );
        assert_eq!(
            guard
                .take_event(WaitableId::Subtask(subtask))
                .expect("the subtask's waitable state"),
            Some(Event::subtask(index, SubtaskState::Returned)),
            "the subtask's pending event says which entry returned, and how"
        );
    }

    /// A waker that counts the wakes it receives. A wake that lands
    /// here is how a test says which waker a poll carried, without
    /// comparing waker identities, which two clones of one waker do
    /// not always agree on.
    #[derive(Default)]
    struct CountingWaker(AtomicUsize);

    impl CountingWaker {
        fn count(&self) -> usize {
            self.0.load(AtomicOrdering::Relaxed)
        }
    }

    impl std::task::Wake for CountingWaker {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, AtomicOrdering::Relaxed);
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, AtomicOrdering::Relaxed);
        }
    }

    /// A future that counts its polls, wakes whatever waker each one
    /// carried, and never resolves.
    #[derive(Clone, Default)]
    struct WakesItsWaker(Arc<AtomicUsize>);

    impl Future for WakesItsWaker {
        type Output = Result<Vec<Val>>;

        fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
            self.0.fetch_add(1, AtomicOrdering::Relaxed);
            context.waker().wake_by_ref();
            Poll::Pending
        }
    }

    #[wcmp_macros::test]
    async fn it_polls_a_host_task_started_outside_a_turn_with_the_next_turns_waker() {
        let engine = Engine::new().expect("engine");
        let (mut store, table, subtask) = host_call(&engine);
        let slot: Lowered = Arc::new(Mutex::new(None));
        let future = WakesItsWaker::default();
        let polls = future.0.clone();
        let counted = Arc::new(CountingWaker::default());
        let driver_waker = Waker::from(counted.clone());

        // No turn is running, so the first poll uses a waker that
        // does nothing. The task joins the store all the same, and
        // counts as woken for the turn that follows.
        store
            .start_host_task(
                HostTask::from_future(subtask, recording(&slot), future),
                table,
                LowerKind::Async,
            )
            .expect("start the host task");
        assert_eq!(
            polls.load(AtomicOrdering::Relaxed),
            1,
            "the call polled the future once before it returned to the guest"
        );
        assert_eq!(
            counted.count(),
            0,
            "a host task started outside a turn is polled with a waker that does nothing"
        );

        {
            let mut driver = Box::pin(Driver::new(&mut store, None, never));
            assert!(
                poll_once(&mut driver, &driver_waker).is_pending(),
                "the host task is still pending, so the driver waits on it"
            );
        }

        assert_eq!(
            polls.load(AtomicOrdering::Relaxed),
            2,
            "the next turn polled the host task again"
        );
        assert_eq!(
            counted.count(),
            1,
            "the next turn polled it with the driver's waker, so no wake is lost"
        );
    }

    #[wcmp_macros::test]
    async fn it_fails_a_synchronous_lower_of_a_pending_future_with_the_stack_switch_cause() {
        let engine = Engine::new().expect("engine");
        let (mut store, table, subtask) = host_call(&engine);
        let slot: Lowered = Arc::new(Mutex::new(None));

        // A synchronous lower whose future resolves at once needs
        // nothing of the suspend seam: the guest gets its result as
        // the call returns.
        store
            .start_host_task(
                HostTask::from_future(
                    subtask,
                    recording(&slot),
                    core::future::ready(Ok(vec![Val::U32(1)])),
                ),
                table,
                LowerKind::Sync,
            )
            .expect("start a host task whose future is ready");
        assert_eq!(lowered(&slot), vec![Val::U32(1)]);

        let subtask = store.lock_tables().expect("tables").tasks.push_subtask();
        let error = store
            .start_host_task(
                HostTask::from_future(
                    subtask,
                    recording(&slot),
                    core::future::pending::<Result<Vec<Val>>>(),
                ),
                table,
                LowerKind::Sync,
            )
            .expect_err("a synchronous lower cannot block the guest thread here");

        assert!(
            matches!(error, Error::Scheduler(SchedulerCause::StackSwitchNeeded)),
            "a synchronous lower of a future that is still running fails with the \
             stack-switch cause while no target fills the suspend seam, and failed \
             with {error} instead"
        );
        assert_eq!(
            store.scheduler.host_task_count(),
            0,
            "the failed call left no host task in the store"
        );
        assert!(
            store
                .lock_tables()
                .expect("tables")
                .tasks
                .subtask(subtask)
                .is_none(),
            "the failed call gave the subtask back"
        );
    }

    #[wcmp_macros::test]
    async fn it_fails_the_guests_call_when_the_host_task_fails_on_its_first_poll() {
        let engine = Engine::new().expect("engine");
        let (mut store, table, subtask) = host_call(&engine);
        let slot: Lowered = Arc::new(Mutex::new(None));

        let error = store
            .start_host_task(
                HostTask::from_future(
                    subtask,
                    recording(&slot),
                    core::future::ready(Err(Error::internal("the host call failed"))),
                ),
                table,
                LowerKind::Async,
            )
            .expect_err("the host call failed, so the guest's call fails");

        assert!(
            error.to_string().contains("the host call failed"),
            "the failure the host call produced is the one the guest sees, and it \
             saw {error} instead"
        );
        assert!(
            slot.lock().expect("the lowering's slot").is_none(),
            "a call that never returned has nothing to lower"
        );
        assert!(
            store
                .lock_tables()
                .expect("tables")
                .tasks
                .subtask(subtask)
                .is_none(),
            "the failed call gave the subtask back"
        );
    }

    #[wcmp_macros::test]
    async fn it_resolves_the_subtask_when_the_host_task_fails_in_a_later_turn() {
        let engine = Engine::new().expect("engine");
        let (mut store, table, subtask) = host_call(&engine);
        let slot: Lowered = Arc::new(Mutex::new(None));
        let outside = Outside::default();
        let awaited = outside.clone();

        let status = store
            .start_host_task(
                HostTask::from_future(subtask, recording(&slot), async move {
                    awaited.await;
                    let failed: Result<Vec<Val>> = Err(Error::internal("the host call failed"));
                    failed
                }),
                table,
                LowerKind::Async,
            )
            .expect("start the host task");
        let index = status
            .subtask_index()
            .expect("a started call carries the index of its subtask");

        outside.resolve();

        // The turn that sees the body fail queues the item, and the
        // turn after it runs the item.
        assert_eq!(
            store.turn(Waker::noop()).expect("a turn"),
            Outcome::Progress,
            "the failed host task left an item ready to run"
        );
        store.turn(Waker::noop()).expect("a turn");

        assert!(
            slot.lock().expect("the lowering's slot").is_none(),
            "a call that never returned has nothing to lower"
        );
        let mut guard = store.lock_tables().expect("tables");
        assert_eq!(
            guard.tasks.subtask(subtask).map(|record| record.state),
            Some(SubtaskState::CancelledBeforeReturned),
            "the call never returned, so its subtask resolved as a cancellation"
        );
        assert_eq!(
            guard
                .take_event(WaitableId::Subtask(subtask))
                .expect("the subtask's waitable state"),
            Some(Event::subtask(index, SubtaskState::CancelledBeforeReturned)),
            "the guest waiting on the subtask takes delivery of the cancellation"
        );
    }

    #[wcmp_macros::test]
    async fn it_resolves_the_subtask_when_the_crossing_fails_in_a_later_turn() {
        let engine = Engine::new().expect("engine");
        let (mut store, table, subtask) = host_call(&engine);
        let outside = Outside::default();
        let awaited = outside.clone();

        let status = store
            .start_host_task(
                HostTask::from_future(
                    subtask,
                    |_store: &mut Store<()>, _outcome: Result<Vec<Val>>| {
                        Err(Error::internal("the crossing failed"))
                    },
                    async move {
                        awaited.await;
                        let produced: Result<Vec<Val>> = Ok(vec![Val::U32(7)]);
                        produced
                    },
                ),
                table,
                LowerKind::Async,
            )
            .expect("start the host task");
        let index = status
            .subtask_index()
            .expect("a started call carries the index of its subtask");

        outside.resolve();

        // The turn that sees the body complete queues the lowering,
        // and the turn after it runs the lowering and fails.
        assert_eq!(
            store.turn(Waker::noop()).expect("a turn"),
            Outcome::Progress,
            "the completed host task left its lowering ready to run"
        );

        assert!(
            store.turn(Waker::noop()).is_err(),
            "the crossing's failure belongs to no caller, so it ends the turn"
        );
        let mut guard = store.lock_tables().expect("tables");
        assert_eq!(
            guard.tasks.subtask(subtask).map(|record| record.state),
            Some(SubtaskState::CancelledBeforeReturned),
            "nothing reached the guest, so the subtask resolved as a \
             cancellation rather than staying started for ever"
        );
        assert_eq!(
            guard
                .take_event(WaitableId::Subtask(subtask))
                .expect("the subtask's waitable state"),
            Some(Event::subtask(index, SubtaskState::CancelledBeforeReturned)),
            "a thread waiting on the subtask takes delivery of that \
             cancellation rather than waiting for an event that never comes"
        );
    }
}
