//! The store as one turn, one item, or one trampoline reaches it.

use core::task::{Poll, Waker};
use std::sync::{Arc, Mutex, MutexGuard};

use wasm_runtime_layer::{AsContextMut, StoreContextMut as RuntimeContextMut, Val as RuntimeVal};

use crate::abi::boundary_call::BoundaryCall;
use crate::backend::Backend;
use crate::component::FunctionType;
use crate::concurrency::{
    Accessor, CallStatus, EventSlot, HostTask, InstanceId, Item, LowerKind, Outcome, ResultChannel,
    Scheduler, SubtaskId, SubtaskState, SuspendSeam, TaskId, TaskState, TurnGuard, WaitableSetId,
    YieldWake,
};
use crate::error::{AbiCause, AbiError, AbiPosition, Error, Result, SchedulerCause};
use crate::executor::ResourceDestructor;
use crate::executor::ir::CanonOptions;
use crate::resource::{HandleTables, ResourceHandle, ResourceTypeId, TableId};
use crate::types::ResourceType;
use crate::value::Val;

use super::store_data::StoreData;
use super::store_id::StoreId;

/// The store as one turn, one item, or one trampoline reaches it.
///
/// [`Store`] owns the core store the runtime layer gives it, and a
/// borrow of that store is the one thing guest work cannot do
/// without: an item calls into the guest, a crossing of the
/// canonical ABI reads the guest's memory, and a destructor is a
/// core function of the defining instance. The runtime layer hands
/// that borrow to a host trampoline as a context, and hands the
/// polyfill nothing else, so a trampoline can never produce the
/// `&mut Store<T>` the host holds: the store the guest call is
/// running against is already borrowed by the call.
///
/// This type is what both of them can produce. It wraps the core
/// store's context — owned by [`Store`], borrowed by a trampoline —
/// and the polyfill's own state rides in that store's data, as
/// [`StoreData`] says. A turn therefore runs the same way from a
/// driver's poll and from inside a trampoline, and the suspend seam
/// and the host tasks, which both run turns, are reachable from
/// either. PDD018 asks for exactly that: a suspended guest thread
/// resumes outside any poll of a driver, so the scheduler's state
/// has to be reachable from a trampoline with no driver on the
/// stack.
///
/// The value borrows the core store for as long as it lives, so it
/// is passed by mutable reference and reborrowed — like the runtime
/// layer's own store context, which is what it wraps.
///
/// [`Store`]: super::Store
pub struct StoreContext<'a, T: 'static> {
    runtime: RuntimeContextMut<'a, StoreData<T>, Backend>,
}

impl<'a, T: 'static> StoreContext<'a, T> {
    /// The store a borrow of the core store reaches.
    ///
    /// A trampoline calls this with the context the runtime layer
    /// handed it, and reaches the whole store through it.
    /// Workspace-internal; not re-exported by `lib.rs`.
    pub fn new(runtime: RuntimeContextMut<'a, StoreData<T>, Backend>) -> Self {
        Self { runtime }
    }

    /// Borrow this context again, for the length of the borrow of
    /// `self`. Workspace-internal.
    pub fn reborrow(&mut self) -> StoreContext<'_, T> {
        StoreContext {
            runtime: self.runtime.as_context_mut(),
        }
    }

    /// Borrow the core store's context, which is what every crossing
    /// of the canonical ABI and every call into the guest runs
    /// against. Workspace-internal.
    pub fn runtime(&self) -> &RuntimeContextMut<'a, StoreData<T>, Backend> {
        &self.runtime
    }

    /// Mutably borrow the core store's context. Workspace-internal.
    pub fn runtime_mut(&mut self) -> &mut RuntimeContextMut<'a, StoreData<T>, Backend> {
        &mut self.runtime
    }

    /// Everything the store carries: the host's data and the
    /// polyfill's own state. Workspace-internal.
    pub fn store_data(&self) -> &StoreData<T> {
        self.runtime.data()
    }

    /// Everything the store carries, mutably. Workspace-internal.
    pub fn store_data_mut(&mut self) -> &mut StoreData<T> {
        self.runtime.data_mut()
    }

    /// The store's host data.
    pub fn data(&self) -> &T {
        self.store_data().host()
    }

    /// The store's host data, mutably.
    pub fn data_mut(&mut self) -> &mut T {
        self.store_data_mut().host_mut()
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
    /// Workspace-internal.
    pub fn tables_handle(&self) -> Arc<Mutex<HandleTables>> {
        self.store_data().tables_handle()
    }

    /// Lock the store's handle tables and record state.
    /// Workspace-internal.
    pub fn lock_tables(&self) -> Result<MutexGuard<'_, HandleTables>> {
        self.store_data().lock_tables()
    }

    /// Lock handle tables reached through a handle of their own,
    /// so that the caller keeps the rest of the store borrowable.
    /// A turn needs this: it holds the scheduler's queues mutably
    /// while it reads the instance records.
    fn lock(tables: &Arc<Mutex<HandleTables>>) -> Result<MutexGuard<'_, HandleTables>> {
        tables
            .lock()
            .map_err(|_| Error::internal("resource handle tables lock poisoned"))
    }

    /// The store's cooperative scheduler. Workspace-internal.
    pub fn scheduler(&self) -> &Scheduler<T> {
        self.store_data().scheduler()
    }

    /// The store's cooperative scheduler, mutably.
    /// Workspace-internal.
    pub fn scheduler_mut(&mut self) -> &mut Scheduler<T> {
        self.store_data_mut().scheduler_mut()
    }

    /// Record what the store knows about a resource type an
    /// instantiation introduced: the destructor to run when a handle
    /// to it is released, and the name an error about one of its
    /// handles renders, when the caller knows one.
    ///
    /// This is the seam an instantiation registers through. The
    /// store's own state is reached through the context rather than
    /// through [`StoreData`] directly, so that every write to it
    /// passes one entry. Workspace-internal.
    pub fn register_resource(
        &mut self,
        type_id: ResourceTypeId,
        name: Option<ResourceType>,
        destructor: ResourceDestructor<T>,
    ) {
        self.store_data_mut()
            .register_resource(type_id, name, destructor);
    }

    /// Record the label a component instantiated into this store
    /// imports or defines `type_id` under, which is the name an
    /// error about one of its handles renders. It outranks any
    /// fallback the store already holds; among labels components
    /// taught, the first wins. Workspace-internal.
    pub fn name_resource(&mut self, type_id: ResourceTypeId, name: ResourceType) {
        self.store_data_mut().name_resource(type_id, name);
    }

    /// Record a label to fall back on for `type_id` while no
    /// component in this store has named it: a host resource the
    /// linker carries that no component instantiated here brought
    /// in. It never displaces a label a component taught, and a
    /// component that names the identity later displaces it.
    /// Workspace-internal.
    pub fn fallback_resource_name(&mut self, type_id: ResourceTypeId, name: ResourceType) {
        self.store_data_mut().fallback_resource_name(type_id, name);
    }

    /// The name the store renders for the resource type `type_id`,
    /// when it learned one. An error about a handle of the type
    /// names it this way. Workspace-internal.
    pub fn resource_type(&self, type_id: ResourceTypeId) -> Option<ResourceType> {
        self.store_data().resource_type(type_id)
    }

    /// Mint a fresh `own<T>` handle in this store's resource table
    /// for the given registered resource type. Workspace-internal.
    pub fn resource_new(&self, type_id: ResourceTypeId, rep: u32) -> Result<ResourceHandle> {
        self.store_data().resource_new(type_id, rep)
    }

    /// Release a handle the host holds. The handle's entry leaves
    /// the host's table for its resource type, and the resource's
    /// destructor runs once: the registered closure for a host
    /// resource, or the defining component's own destructor for a
    /// locally-defined one. Workspace-internal.
    ///
    /// The destructor runs as a task with one fresh thread, which is
    /// the current scope until it returns or fails, exactly as a
    /// destructor a guest's `resource.drop` runs: the reference
    /// lifts the destructor as a synchronous function of one `u32`
    /// parameter and lowers a call to it, whoever released the
    /// handle. The thread's slots start at zero and end with it.
    pub fn resource_drop(&mut self, handle: ResourceHandle) -> Result<()> {
        let rep = self.store_data().remove_host_handle(handle)?;
        let Some(destructor) = self.store_data().destructor(handle.type_id) else {
            return Ok(());
        };
        let tables = self.tables_handle();
        let _call = BoundaryCall::destructor(&tables, destructor.instance())?;
        match destructor {
            ResourceDestructor::Host(body) => body(self.data_mut(), rep),
            ResourceDestructor::Local { function: slot, .. } => {
                let function = slot
                    .lock()
                    .map_err(|_| Error::internal("resource destructor slot poisoned"))?
                    .clone();
                if let Some(function) = function {
                    function
                        .call(&mut self.runtime, &[RuntimeVal::I32(rep as i32)], &mut [])
                        .map_err(|err| {
                            // The call that failed is the core
                            // destructor's, whose one argument is the
                            // resource's `u32` rep, not the own
                            // handle the caller released, so the
                            // failure names no value type.
                            Error::from(AbiError {
                                position: AbiPosition::Argument(0),
                                valtype: None,
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
        let _turn = TurnGuard::enter(self.tables(), waker);
        self.run_turn(waker, false, None)
    }

    /// Whether a turn of this store is running. Workspace-internal.
    pub fn turn_in_flight(&self) -> bool {
        self.store_data().turn_in_flight()
    }

    /// Whether the store holds work only a turn can carry forward.
    /// Workspace-internal.
    pub fn has_pending_work(&self) -> bool {
        self.store_data().has_pending_work()
    }

    /// Whether the store holds an item a turn would run.
    /// Workspace-internal.
    pub fn has_ready_item(&self) -> bool {
        self.store_data().has_ready_item()
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
        let _turn = TurnGuard::enter(self.tables(), waker);
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
    ///
    /// `only`, when it names an instance, holds the turn to the
    /// ready work of that instance: it runs no item of another
    /// instance and polls no host task. That is the lazy blocking
    /// rule of the reference for a task that must not block, which
    /// gives way to the ready threads of its own instance and then
    /// traps. Such a turn reports [`Outcome::Progress`] when it ran
    /// something and [`Outcome::Idle`] when that instance had
    /// nothing ready. Workspace-internal.
    pub fn nested_turn(&mut self, waker: &Waker, only: Option<InstanceId>) -> Result<Outcome> {
        self.run_turn(waker, true, only)
    }

    /// The instance a nested turn run for the current task may run
    /// the work of, or `None` when that task is allowed to block.
    /// Workspace-internal.
    pub fn must_not_block_instance(&self) -> Option<InstanceId> {
        self.store_data().must_not_block_instance()
    }

    /// The waker of the turn that is running, or a waker that does
    /// nothing when no turn is running. Workspace-internal.
    pub fn active_waker(&self) -> Waker {
        self.store_data().active_waker()
    }

    /// Why a driver that went idle failed. Workspace-internal.
    pub fn idle_cause(&self, task: Option<TaskId>) -> SchedulerCause {
        self.store_data().idle_cause(task)
    }

    /// Why a nested turn that went idle with its condition unmet
    /// failed. Workspace-internal.
    pub fn suspend_cause(&self) -> SchedulerCause {
        self.store_data().suspend_cause()
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
    /// the call returns, so a body that is still running has to block
    /// the guest thread where it stands. That block goes through the
    /// suspend seam, so the seam's provider slot decides it: a target
    /// that has filled the slot serves the block, and a target that
    /// has not fails the call with the stack-switch cause and gives
    /// the subtask back.
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
            let accessor = Accessor::new(self.reborrow());
            accessor.attend(&waker)?;
            task.poll(&accessor, &waker)
        };

        match outcome {
            Poll::Ready(Ok(values)) => {
                // The call is over, so the subtask leaves the stack
                // and gives back the handles the guest lent for it
                // before the result crosses: a borrow lowered back
                // out belongs to the caller's task.
                self.lock_tables()?
                    .exit_subtask(subtask, SubtaskState::Returned);
                task.lower(self, Ok(values))?;
                Ok(CallStatus::returned())
            }
            // The call never returned, so the subtask's resolution
            // is a cancellation, and the handles the guest lent for
            // it are given back all the same. The guest's call is on
            // the stack, so the failure travels out to it and
            // nothing crosses.
            Poll::Ready(Err(error)) => {
                self.lock_tables()?.abandon_subtask(subtask);
                Err(error)
            }
            Poll::Pending => match lower {
                LowerKind::Sync => self.block_on_host_task(task, subtask),
                LowerKind::Async => {
                    let index = {
                        let mut guard = self.lock_tables()?;
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

    /// Block the guest thread on a host task whose body is still
    /// running, which is what a synchronous lower of a host `async`
    /// function comes to: the guest expects the result when the call
    /// returns, so the call cannot return until the body does.
    ///
    /// PDD018 gives the rule: through a synchronous lower the call
    /// succeeds if the first poll resolved the body, or if the
    /// suspend seam is filled on this target, and otherwise the
    /// trampoline fails with the stack-switch cause. The seam is
    /// what the block goes through, so the slot decides it here
    /// rather than the target being read off somewhere else. The
    /// slot is empty on both targets today, so this call fails as it
    /// always has.
    ///
    /// The task stays in this frame while the thread is suspended.
    /// It belongs to a call the guest still has on the stack, and
    /// its subtask is still the current scope, so it is not one of
    /// the store's host tasks: the store's are the ones whose calls
    /// have returned, and a lowering queued for one of those
    /// resolves its subtask and fills its event, which is what a
    /// guest told the call started waits for and not what a guest
    /// still inside the call does.
    ///
    /// The body is therefore polled from the readiness condition,
    /// once per check, and the provider decides when to check — a
    /// suspension and a resumption apart. The wake that brings the
    /// thread back to a check is the provider's to supply, which is
    /// one of the two obligations the suspend provider trait states.
    fn block_on_host_task(
        &mut self,
        mut task: HostTask<T>,
        subtask: SubtaskId,
    ) -> Result<CallStatus> {
        if !self.scheduler().suspend_seam().has_provider() {
            self.lock_tables()?.abandon_subtask(subtask);
            return Err(Error::Scheduler(SchedulerCause::StackSwitchNeeded));
        }

        let mut produced: Option<Result<Vec<Val>>> = None;
        let suspended = SuspendSeam::suspend(self, |store| {
            if produced.is_some() {
                return true;
            }
            let waker = store.active_waker();
            let accessor = Accessor::new(store.reborrow());
            if let Err(error) = accessor.attend(&waker) {
                produced = Some(Err(error));
                return true;
            }
            match task.poll(&accessor, &waker) {
                Poll::Ready(value) => {
                    produced = Some(value);
                    true
                }
                Poll::Pending => false,
            }
        });

        match (suspended, produced) {
            (Ok(()), Some(Ok(values))) => {
                self.lock_tables()?
                    .exit_subtask(subtask, SubtaskState::Returned);
                task.lower(self, Ok(values))?;
                Ok(CallStatus::returned())
            }
            // The body failed, the suspension failed, or the
            // provider returned with the condition unmet. Each is a
            // call that never returned, so the subtask resolves as a
            // cancellation and the failure travels out to the
            // guest's call.
            (Ok(()), Some(Err(error))) | (Err(error), _) => {
                self.lock_tables()?.abandon_subtask(subtask);
                Err(error)
            }
            (Ok(()), None) => {
                self.lock_tables()?.abandon_subtask(subtask);
                Err(Error::Scheduler(self.suspend_cause()))
            }
        }
    }

    /// Give a host task to the store. The next turn polls it with
    /// the driver's waker, so no wake is lost. Workspace-internal.
    pub fn push_host_task(&mut self, task: HostTask<T>) {
        self.scheduler_mut().push_host_task(task);
    }

    /// The body of one turn, with the waker already recorded.
    ///
    /// `nested` marks the turn the suspend seam runs from inside a
    /// guest call. Only a turn that is not nested touches the
    /// resumptions after a yield, because only a driver's turn can
    /// end and hand control back to the host executor first.
    ///
    /// `only`, when it names an instance, holds the turn to that
    /// instance's ready work, which is what a task that must not
    /// block gives way to. Such a turn polls no host task: a task
    /// that must not block must not wait on one, and the cause it
    /// fails with says so.
    ///
    /// An item that fails ends the turn and its failure is the
    /// turn's. Almost no item can fail: what an item produces it
    /// leaves in the store, and the failure of the call it ran is
    /// part of that. The ones that can are the items whose own
    /// bookkeeping failed, which belongs to no caller.
    fn run_turn(
        &mut self,
        waker: &Waker,
        nested: bool,
        only: Option<InstanceId>,
    ) -> Result<Outcome> {
        self.open_entry_gate()?;
        if !nested {
            let resumed = self.scheduler_mut().take_resume_after_yield();
            if let Some(item) = resumed {
                item.run(self)?;
            }
        }
        let mut ran = false;
        loop {
            self.open_entry_gate()?;
            let ready = match only {
                Some(instance) => self.scheduler_mut().take_ready_in(instance),
                None => self.scheduler_mut().take_ready(),
            };
            if let Some(item) = ready {
                ran = true;
                item.run(self)?;
                continue;
            }
            if !nested && self.scheduler_mut().defer_low_priority() {
                return Ok(Outcome::Yield);
            }
            break;
        }
        if only.is_some() {
            // The instance's ready work is the whole of what this
            // turn was allowed to run. Progress sends the seam round
            // again to test its condition; an idle answer is its cue
            // to stop and trap with the cannot-block cause.
            return Ok(if ran {
                Outcome::Progress
            } else {
                Outcome::Idle
            });
        }
        self.poll_host_tasks(waker)?;
        if self.scheduler().has_immediate_item() {
            return Ok(Outcome::Progress);
        }
        // Only deferred work is left. A nested turn reaches this and
        // gives way: the resumption belongs to the outer turn, after
        // the driver has returned control to the host executor. A
        // driver's turn does not reach it, because the loop above
        // deferred its front and returned already.
        if self.scheduler().has_deferred_item() {
            return Ok(Outcome::Yield);
        }
        if self.scheduler().host_task_count() == 0 {
            return Ok(Outcome::Idle);
        }
        Ok(Outcome::Waiting)
    }

    /// Make ready every piece of work the store was holding back:
    /// the callback items whose condition now holds, then the tasks
    /// the entry gate can release. Costs nothing while the store
    /// holds neither, which is every turn of the synchronous
    /// baseline.
    ///
    /// The held callbacks go first, so that a task whose wait a
    /// previous turn satisfied resumes ahead of a task that has yet
    /// to enter its instance.
    fn open_entry_gate(&mut self) -> Result<()> {
        if self.scheduler().waiting_at_gate() == 0 && self.scheduler().held_callbacks() == 0 {
            return Ok(());
        }
        // The tables are reached through a handle of their own, so
        // that the guard on them and the borrow of the scheduler,
        // which the store's data holds, do not overlap.
        let tables = self.tables_handle();
        let mut guard = Self::lock(&tables)?;
        self.scheduler_mut().release_held_callbacks(&mut guard)?;
        self.scheduler_mut().open_entry_gate(&mut guard.tasks);
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
        let mut tasks = self.scheduler_mut().take_host_tasks();
        if tasks.is_empty() {
            return Ok(());
        }
        let mut pending = Vec::with_capacity(tasks.len());
        let mut completed = Vec::new();
        {
            let accessor = Accessor::new(self.reborrow());
            accessor.attend(waker)?;
            for mut task in tasks.drain(..) {
                match task.poll(&accessor, waker) {
                    Poll::Ready(value) => completed.push((task, value)),
                    Poll::Pending => pending.push(task),
                }
            }
        }
        self.scheduler_mut().restore_host_tasks(pending);
        for (task, value) in completed {
            self.scheduler_mut()
                .push_high_priority(task.lowering_item(value));
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
        let tables = self.tables_handle();
        let mut guard = Self::lock(&tables)?;
        self.scheduler_mut().enter_implicit_thread(
            &mut guard.tasks,
            task,
            instance,
            async_function,
            needs_exclusive,
            item,
        );
        Ok(())
    }

    /// Give an export's task a channel to resolve through and hand
    /// the caller its half.
    ///
    /// A host call into an asynchronous export takes this: the task
    /// outlives the call, so `task.return` sends the result through
    /// the channel and the call's driver takes it out, rather than
    /// the call reading the record of a task that may be gone by
    /// then. Workspace-internal.
    pub fn attach_result_channel(&self, task: TaskId) -> Result<ResultChannel> {
        self.lock_tables()?
            .tasks
            .attach_result_channel(task)
            .ok_or_else(|| Error::internal("an export's task is not in the store"))
    }

    /// Whether an export's task has resolved: the reference's
    /// `state == RESOLVED`. The exit of a callback task's implicit
    /// thread reads it, because a thread that exits without a result
    /// is the no-result trap. Workspace-internal.
    pub fn export_task_resolved(&self, task: TaskId) -> Result<bool> {
        Ok(self
            .lock_tables()?
            .tasks
            .task(task)
            .ok_or_else(|| Error::internal("an export's task is not in the store"))?
            .state
            == TaskState::Resolved)
    }

    /// Whether a thread holds `instance` exclusively. A callback item
    /// asks before it runs: the instance is one task's at a time, so
    /// an item that finds it taken waits for the holder to release it.
    /// Workspace-internal.
    pub fn instance_is_held(&self, instance: InstanceId) -> Result<bool> {
        Ok(self
            .lock_tables()?
            .tasks
            .instance(instance)
            .is_some_and(|record| record.exclusive_thread.is_some()))
    }

    /// Give the instance back that an export task's implicit thread
    /// holds exclusively. A callback task releases it between events,
    /// so a synchronous export of the same instance can run while the
    /// task waits. Workspace-internal.
    pub fn release_exclusive_thread(&mut self, task: TaskId) -> Result<()> {
        let tables = self.tables_handle();
        let mut guard = Self::lock(&tables)?;
        self.scheduler()
            .release_exclusive_thread(&mut guard.tasks, task);
        Ok(())
    }

    /// Give `instance` to an export task's implicit thread, which a
    /// callback item does before it runs core code.
    /// Workspace-internal.
    pub fn take_exclusive_thread(&mut self, task: TaskId, instance: InstanceId) -> Result<()> {
        let tables = self.tables_handle();
        let mut guard = Self::lock(&tables)?;
        self.scheduler()
            .take_exclusive_thread(&mut guard.tasks, task, instance);
        Ok(())
    }

    /// Deliver the wait a callback task's status word asked for.
    ///
    /// The set is looked up in `table`, the instance's own handle
    /// table, and the task's exclusive hold on the instance is
    /// released either way. A set that already holds an event
    /// delivers it and `item` is queued at once, with that event in
    /// `slot`. A set that holds none parks the task's implicit thread
    /// on it and the item waits with it, until a later turn finds the
    /// set filled. Workspace-internal.
    pub fn wait_callback_on_set(
        &mut self,
        task: TaskId,
        instance: InstanceId,
        table: TableId,
        set_index: u32,
        slot: EventSlot,
        item: Item<T>,
    ) -> Result<()> {
        let tables = self.tables_handle();
        let mut guard = Self::lock(&tables)?;
        let set = Self::waitable_set_at(&guard, table, set_index)?;
        let thread = guard
            .tasks
            .task(task)
            .map(|record| record.implicit_thread)
            .ok_or_else(|| Error::internal("an export's task is not in the store"))?;
        self.scheduler()
            .release_exclusive_thread(&mut guard.tasks, task);
        match guard.wait_on_waitable_set(set, thread)? {
            Some(event) => {
                slot.fill(event);
                self.scheduler_mut().push_high_priority(item);
            }
            None => self
                .scheduler_mut()
                .hold_for_event(instance, thread, set, slot, item),
        }
        Ok(())
    }

    /// The waitable set the entry at `index` of `table` names, or the
    /// failure a status word that named something else produces.
    fn waitable_set_at(tables: &HandleTables, table: TableId, index: u32) -> Result<WaitableSetId> {
        tables
            .waitable_set_from_handle(table, index)
            .map_err(|err| {
                Error::from(AbiError {
                    position: AbiPosition::Result,
                    valtype: None,
                    cause: AbiCause::InvalidHandle {
                        reason: err.to_string(),
                    },
                })
            })
    }

    /// Make an export's task the current scope, as its thread starts
    /// to run. Workspace-internal.
    pub fn enter_export_task(&self, task: TaskId) -> Result<()> {
        self.lock_tables()?.tasks.push_task_scope(task);
        Ok(())
    }

    /// Mark the instance of an export's task as one whose threads
    /// may not suspend, for the length of the call, and save the
    /// flag's old value on the task's thread. The task's own exit
    /// puts it back.
    ///
    /// A host call into a synchronous export holds the flag this
    /// way, as the enter intrinsic holds it for a synchronous call
    /// between two components: such a call must return before its
    /// instance may block, and a built-in that has to block gives
    /// way only to the ready work of the instance and then fails
    /// with the cannot-block cause. Workspace-internal.
    pub fn hold_may_not_suspend(&self, task: TaskId) -> Result<()> {
        self.lock_tables()?
            .tasks
            .hold_may_not_suspend(task)
            .ok_or_else(|| Error::internal("an export's task is not in the store"))
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
    pub fn exit_export_task(&mut self, task: TaskId) -> Result<core::result::Result<(), u32>> {
        let tables = self.tables_handle();
        let mut guard = Self::lock(&tables)?;
        self.scheduler_mut()
            .exit_implicit_thread(&mut guard.tasks, task);
        Ok(guard.exit_task(task))
    }

    /// Pop the scope of an export's task without ending the task, as
    /// the callback loop of an asynchronous export does when core
    /// code returns: the record stays in the store, because the
    /// status word decides what the task does next.
    /// Workspace-internal.
    pub fn leave_export_task(&self, task: TaskId) -> Result<()> {
        self.lock_tables()?.leave_task_scope(task);
        Ok(())
    }

    /// End an export's task whose scope is already popped, which is
    /// the reference's `exit_implicit_thread` for a callback task:
    /// the instance the task held exclusively goes back and its
    /// record leaves the store, exactly as a synchronous task's does
    /// when its call returns. The inner `Err` carries the count of
    /// borrows the guest did not drop. Workspace-internal.
    pub fn end_export_task(&mut self, task: TaskId) -> Result<core::result::Result<(), u32>> {
        let tables = self.tables_handle();
        let mut guard = Self::lock(&tables)?;
        self.scheduler()
            .exit_implicit_thread(&mut guard.tasks, task);
        Ok(guard.end_task(task))
    }

    /// Pop the export's task on its failure path, with no borrow
    /// check. Every scope the failure left above the task — the task
    /// of a callee that trapped, the subtask of a host call that
    /// failed — is popped with it, and the lends of each are given
    /// back. The task's implicit thread ends first, for the reason
    /// [`exit_export_task`](Self::exit_export_task) gives: a call
    /// that failed gives the instance back exactly as one that
    /// returned does. Workspace-internal.
    pub fn abandon_export_task(&mut self, task: TaskId) -> Result<()> {
        let tables = self.tables_handle();
        let mut guard = Self::lock(&tables)?;
        self.scheduler_mut()
            .exit_implicit_thread(&mut guard.tasks, task);
        guard.abandon_task(task);
        Ok(())
    }

    /// Run `body` with an accessor to this store, driving the
    /// store's scheduler until the future `body` returns completes,
    /// and return what that future resolved to.
    ///
    /// This is the body of [`Store::run_concurrent`], which is the
    /// entry a host calls. It takes the context by value because the
    /// accessor it hands `body` lends the store for as long as the
    /// entry's own future lives.
    ///
    /// This is a driver: one poll of the returned future is a turn,
    /// and guest code runs only inside a turn.
    ///
    /// Entering it while another driver of the same store is inside
    /// a turn fails with the recursive-driver cause. Dropping the
    /// returned future cancels nothing: whatever the driver queued
    /// stays in the store and runs in the next turn of any driver.
    ///
    /// A turn that finds nothing ready and no host task pending
    /// leaves this entry pending rather than failing with the
    /// deadlock cause, which is the one rule where it differs from
    /// the other drivers: `body`'s future can wait on something
    /// outside the store, and the waker it was polled with is the
    /// one that brings the entry back.
    ///
    /// [`Store::run_concurrent`]: super::Store::run_concurrent
    /// Workspace-internal.
    pub async fn run_concurrent<R, F>(self, body: F) -> Result<R>
    where
        F: AsyncFnOnce(&Accessor<'a, T>) -> R,
    {
        // The refusal happens before the accessor exists, so a
        // refused entry leaves the store untouched.
        if self.turn_in_flight() {
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
                        // That poll can have queued an item through
                        // the accessor, and only a turn runs an
                        // item. The re-check asks for a ready item
                        // rather than for pending work of any kind:
                        // a host task is pending under this outcome
                        // by definition, and it is not what the
                        // entry parks on — a host task that never
                        // returns would otherwise strand the item.
                        match accessor.lend(|store| store.has_ready_item()) {
                            Ok(true) => continue,
                            Ok(false) => return Poll::Pending,
                            Err(error) => return Poll::Ready(Err(error)),
                        }
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
}

#[cfg(test)]
mod tests {
    use core::future::Future;
    use core::pin::Pin;
    use core::task::Context;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    use wcmp_macros::component;

    use crate::component::Component;
    use crate::concurrency::{
        Driver, Event, HostTask, ItemKind, Scope, SuspendProvider, WaitableId,
    };
    use crate::engine::Engine;
    use crate::error::SchedulerCause;
    use crate::linker::{HostCall, Linker};
    use crate::resource::HandleKind;
    use crate::store::Store;
    use crate::types::ValueType;

    use super::*;

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
    fn never(_store: &mut StoreContext<'_, ()>, _waker: &Waker) -> Option<Result<()>> {
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
                        .with(|store: &mut StoreContext<'_, ()>| {
                            store.scheduler_mut().push_high_priority(Item::new(
                                ItemKind::TaskStart,
                                move |_store: &mut StoreContext<'_, ()>| {
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
                        .with(|store: &mut StoreContext<'_, ()>| {
                            let subtask =
                                store.lock_tables().expect("tables").tasks.insert_subtask();
                            store.scheduler_mut().push_host_task(HostTask::from_future(
                                subtask,
                                move |_store: &mut StoreContext<'_, ()>,
                                      _outcome: Result<Vec<Val>>| {
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
        store.scheduler_mut().push_host_task(HostTask::from_future(
            subtask,
            |_store: &mut StoreContext<'_, ()>, _outcome: Result<Vec<Val>>| Ok(()),
            core::future::pending::<Result<Vec<Val>>>(),
        ));
        let ran = Arc::new(AtomicUsize::new(0));
        let counted = ran.clone();
        store.scheduler_mut().push_high_priority(Item::new(
            ItemKind::TaskStart,
            move |_store: &mut StoreContext<'_, ()>| {
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

    #[wcmp_macros::test]
    async fn it_runs_the_item_the_closure_queued_as_the_turn_left_a_host_task_pending() {
        let engine = Engine::new().expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");

        // A host task that never returns, so every turn reports one
        // pending, and nothing outside the store ever wakes the
        // entry. The item the closure queues is the only thing that
        // resolves it, and only a turn runs an item — so an entry
        // that parked here would wait for ever on work it holds.
        let subtask = store.lock_tables().expect("tables").tasks.insert_subtask();
        store.scheduler_mut().push_host_task(HostTask::from_future(
            subtask,
            |_store: &mut StoreContext<'_, ()>, _outcome: Result<Vec<Val>>| Ok(()),
            core::future::pending::<Result<Vec<Val>>>(),
        ));

        // The closure queues the item on the poll the entry makes
        // after the turn reported the host task pending.
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
                        .with(|store: &mut StoreContext<'_, ()>| {
                            store.scheduler_mut().push_high_priority(Item::new(
                                ItemKind::TaskStart,
                                move |_store: &mut StoreContext<'_, ()>| {
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
            panic!("the entry parked on a ready item while a host task was pending");
        };
        assert_eq!(
            value.expect("run the closure"),
            "the item ran",
            "the entry ran the item the closure queued rather than park on a \
             host task it does not wait for"
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
        store.scheduler_mut().push_high_priority(Item::new(
            ItemKind::TaskStart,
            |_store: &mut StoreContext<'_, ()>| -> Result<()> { panic!("the item panicked") },
        ));

        let unwound = unwind(|| store.turn(Waker::noop()));

        assert!(unwound.is_err(), "the item's panic unwound the turn");
        assert!(
            !store.turn_in_flight(),
            "the turn the panic unwound out of is over"
        );
        let mut driver = Box::pin(Driver::new(store.context(), None, |_store, _waker| {
            Some(Ok(()))
        }));
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
        store.scheduler_mut().push_high_priority(Item::new(
            ItemKind::TaskStart,
            |store: &mut StoreContext<'_, ()>| -> Result<()> {
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
        let mut driver = Box::pin(Driver::new(store.context(), None, |_store, _waker| {
            Some(Ok(()))
        }));
        assert!(
            matches!(poll_once(&mut driver, Waker::noop()), Poll::Ready(Ok(()))),
            "a driver entered after the panic is not refused"
        );
    }

    /// Poison a store's handle tables the way a panic taken outside
    /// any turn does: with the lock held and no guard in flight to
    /// give it back.
    #[cfg(not(target_arch = "wasm32"))]
    fn poison_the_tables<T: 'static>(store: &Store<T>) {
        let tables = store.tables_handle();
        let poisoned = unwind(move || {
            let _tables = tables.lock().expect("tables");
            panic!("the host panicked with the tables locked")
        });
        assert!(poisoned.is_err(), "the panic unwound");
        assert!(store.tables().is_poisoned(), "and poisoned the lock");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn it_enters_a_turn_on_tables_a_panic_outside_the_store_poisoned() {
        let engine = Engine::new().expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");

        // A panic taken with the lock held and no turn in flight, so
        // what meets the poison is the guard on its way in rather
        // than on its way out.
        poison_the_tables(&store);

        let outcome = store.turn(Waker::noop());

        assert!(
            outcome.is_ok(),
            "the turn's guard took the tables back from the poison on its way in"
        );
        assert!(
            !store.tables().is_poisoned(),
            "and cleared it, so every later reader of the store reaches them too"
        );
    }

    // Native only, for the reason the tests above are: the browser
    // aborts on a panic, so no panic ever reaches a lock there.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn it_enters_a_driver_on_tables_a_panic_outside_a_turn_poisoned() {
        let engine = Engine::new().expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");

        // Both driver entries ask whether a turn is running before
        // either of them has a guard of its own, so the poison is
        // here before anything that could clear it.
        poison_the_tables(&store);

        let mut driver = Box::pin(Driver::new(store.context(), None, |_store, _waker| {
            Some(Ok(()))
        }));
        assert!(
            matches!(poll_once(&mut driver, Waker::noop()), Poll::Ready(Ok(()))),
            "the driver read the store's turn state past the poison, so a \
             panic outside any turn does not refuse it"
        );
        drop(driver);
        assert!(
            store.tables().is_poisoned(),
            "and the read left the poison where it found it: this driver's \
             condition was met before it ever entered a turn, and entering a \
             turn is where the recovery happens"
        );

        assert!(
            store.turn(Waker::noop()).is_ok(),
            "the turn this driver never needed takes the tables back"
        );
        assert!(!store.tables().is_poisoned(), "and clears the poison");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn it_enters_the_concurrent_entry_on_tables_a_panic_outside_a_turn_poisoned() {
        let engine = Engine::new().expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");

        // The same for the other driver entry, which reads the turn
        // state before it builds the accessor.
        poison_the_tables(&store);

        let mut entry = Box::pin(store.run_concurrent(async |_accessor| "the closure ran"));
        let Poll::Ready(value) = poll_once(&mut entry, Waker::noop()) else {
            panic!("the entry parked instead of running its closure");
        };
        assert_eq!(
            value.expect("the entry is not refused"),
            "the closure ran",
            "a panic outside any turn does not refuse the concurrent entry \
             either"
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
        store.scheduler_mut().push_low_priority(Item::new(
            ItemKind::TaskStart,
            move |_store: &mut StoreContext<'_, ()>| {
                counted.fetch_add(1, AtomicOrdering::Relaxed);
                signal.resolve();
                Ok(())
            },
        ));

        {
            // A driver the item gave way to, dropped before the item
            // ran. Dropping it cancels nothing.
            let mut abandoned = Box::pin(Driver::new(store.context(), None, never));
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
    ) -> impl FnOnce(&mut StoreContext<'_, ()>, Result<Vec<Val>>) -> Result<()> + Send + 'static
    {
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
            .context()
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
            store.scheduler().host_task_count(),
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
            .context()
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
            store.scheduler().host_task_count(),
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
            .context()
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
            let mut driver = Box::pin(Driver::new(store.context(), None, never));
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
            .context()
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
            .context()
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
            store.scheduler().host_task_count(),
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
            .context()
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
            .context()
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
            .context()
            .start_host_task(
                HostTask::from_future(
                    subtask,
                    |_store: &mut StoreContext<'_, ()>, _outcome: Result<Vec<Val>>| {
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

    /// A component whose export calls a host function and adds one
    /// to what it returns. A host `async` function's call starts its
    /// host task from exactly this frame: a trampoline, with the
    /// export call's driver the only driver on the stack.
    const CALLS_THE_HOST: &[u8] = component!(
        r#"
        (component
          (import "probe" (func $probe (param "x" u32) (result u32)))
          (core func $probe' (canon lower (func $probe)))
          (core module $m
            (import "" "probe" (func $probe (param i32) (result i32)))
            (func (export "run") (param i32) (result i32)
              local.get 0 call $probe i32.const 1 i32.add))
          (core instance $i (instantiate $m
            (with "" (instance (export "probe" (func $probe'))))))
          (func (export "run") (param "x" u32) (result u32)
            (canon lift (core func $i "run"))))
        "#
    );

    /// A host task's body that is still running on its first poll
    /// and completes on its second, as a call that waits on
    /// something outside the store is.
    struct ReadyOnSecondPoll {
        polls: usize,
        value: u32,
    }

    impl Future for ReadyOnSecondPoll {
        type Output = Result<Vec<Val>>;

        fn poll(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Self::Output> {
            let this = self.get_mut();
            this.polls += 1;
            if this.polls >= 2 {
                return Poll::Ready(Ok(vec![Val::U32(this.value)]));
            }
            Poll::Pending
        }
    }

    /// A provider that stands in for a target that can switch
    /// stacks. It consults the readiness condition until it holds,
    /// as a provider that suspended the guest thread and resumed it
    /// between checks would, and gives up rather than spinning for
    /// ever. What it does not do is run a turn: the point of a
    /// provider is that the thread suspends and the scheduler runs
    /// somewhere else.
    struct Resumes(usize);

    impl SuspendProvider<()> for Resumes {
        fn suspend(
            &mut self,
            store: &mut StoreContext<'_, ()>,
            condition: &mut dyn FnMut(&mut StoreContext<'_, ()>) -> bool,
        ) -> Result<()> {
            for _ in 0..self.0 {
                if condition(store) {
                    return Ok(());
                }
            }
            Err(Error::Scheduler(store.suspend_cause()))
        }
    }

    #[wcmp_macros::test]
    async fn it_fails_a_synchronous_lower_from_a_trampoline_with_no_provider() {
        let engine = Engine::new().expect("engine");
        let component = Component::new(&engine, CALLS_THE_HOST)
            .await
            .expect("component parses");
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");

        // What the host function's call of `start_host_task` came
        // out as, and how many host tasks the store held afterwards.
        let started: Arc<Mutex<Option<(String, usize)>>> = Arc::new(Mutex::new(None));
        let recorded = started.clone();

        let mut linker: Linker<()> = Linker::new(&engine);
        linker.root().func_wrap(
            "probe",
            move |mut call: HostCall<'_, ()>, (x,): (u32,)| -> Result<u32> {
                let store = call.store();
                let subtask = store.lock_tables()?.tasks.push_subtask();
                let error = store
                    .start_host_task(
                        HostTask::from_future(
                            subtask,
                            |_store: &mut StoreContext<'_, ()>, _outcome: Result<Vec<Val>>| Ok(()),
                            ReadyOnSecondPoll {
                                polls: 0,
                                value: x * 2,
                            },
                        ),
                        TableId::fresh(),
                        LowerKind::Sync,
                    )
                    .err()
                    .map_or_else(
                        || "the call succeeded".to_owned(),
                        |error| error.to_string(),
                    );
                *recorded.lock().expect("record") =
                    Some((error, store.scheduler().host_task_count()));
                Ok(x)
            },
        );

        let instance = linker
            .instantiate(&mut store, &component)
            .await
            .expect("instantiate");
        let run = instance.get_func("run").expect("run export");
        let result = run
            .call(&mut store, &[Val::U32(20)])
            .await
            .expect("call run");

        assert_eq!(
            *started.lock().expect("record"),
            Some((
                Error::Scheduler(SchedulerCause::StackSwitchNeeded).to_string(),
                0
            )),
            "the trampoline reached `start_host_task`, and a synchronous lower \
             of a body that is still running consulted the seam's empty \
             provider slot and failed with the stack-switch cause, leaving no \
             host task behind"
        );
        assert_eq!(
            result.first(),
            Some(&Val::U32(21)),
            "the guest's call returned, since the host function reported the \
             refusal rather than failing its own call"
        );
    }

    #[wcmp_macros::test]
    async fn it_blocks_a_synchronous_lower_through_the_seam_when_the_slot_is_filled() {
        let engine = Engine::new().expect("engine");
        let component = Component::new(&engine, CALLS_THE_HOST)
            .await
            .expect("component parses");
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");

        // The target fills the capability. The trampoline finds it
        // through the core store's context the runtime layer hands
        // it, which reaches the same scheduler this fills.
        store
            .scheduler_mut()
            .suspend_seam_mut()
            .set_provider(Resumes(4));

        // The status word the call reported, and what the lowering
        // was handed.
        let reported: Arc<Mutex<Option<u32>>> = Arc::new(Mutex::new(None));
        let recorded = reported.clone();
        let slot: Lowered = Arc::new(Mutex::new(None));
        let filled = slot.clone();

        let mut linker: Linker<()> = Linker::new(&engine);
        linker.root().func_wrap(
            "probe",
            move |mut call: HostCall<'_, ()>, (x,): (u32,)| -> Result<u32> {
                let store = call.store();
                let subtask = store.lock_tables()?.tasks.push_subtask();
                let lowering = filled.clone();
                let status = store.start_host_task(
                    HostTask::from_future(
                        subtask,
                        move |_store: &mut StoreContext<'_, ()>, outcome: Result<Vec<Val>>| {
                            *lowering.lock().expect("the lowering's slot") = Some(outcome);
                            Ok(())
                        },
                        ReadyOnSecondPoll {
                            polls: 0,
                            value: x * 2,
                        },
                    ),
                    TableId::fresh(),
                    LowerKind::Sync,
                )?;
                *recorded.lock().expect("record") = Some(status.value());
                Ok(x)
            },
        );

        let instance = linker
            .instantiate(&mut store, &component)
            .await
            .expect("instantiate");
        let run = instance.get_func("run").expect("run export");
        let result = run
            .call(&mut store, &[Val::U32(20)])
            .await
            .expect("call run");

        assert_eq!(
            *reported.lock().expect("record"),
            Some(CallStatus::returned().value()),
            "the seam's filled slot served the block, so the call returned its \
             result to the guest with no subtask behind it"
        );
        assert_eq!(
            lowered(&slot),
            vec![Val::U32(40)],
            "what the body produced crossed through the lowering of the call \
             that blocked on it"
        );
        assert_eq!(
            result.first(),
            Some(&Val::U32(21)),
            "the guest's call returned"
        );
        assert_eq!(
            store.scheduler().host_task_count(),
            0,
            "the task the call blocked on stayed in the frame that started it, \
             so the store holds none"
        );
    }

    /// What a host destructor saw of the store's records while it
    /// ran.
    #[derive(Clone, Debug, Default, Eq, PartialEq)]
    struct SeenByDestructor {
        /// How deep the stack of current scopes was.
        scopes: usize,
        /// How many task records the store held.
        tasks: usize,
        /// How many thread records the store held.
        threads: usize,
        /// Whether the current scope was a task.
        current_is_task: bool,
        /// The context slots of the current thread.
        context: [i32; 2],
    }

    /// Read the store's records as the destructor on the stack sees
    /// them, then write `value` into slot 0 of the current thread,
    /// which is what a `context.set` in a guest destructor does.
    fn record_and_set(tables: &Arc<Mutex<HandleTables>>, value: i32) -> SeenByDestructor {
        let mut guard = tables.lock().expect("handle tables");
        let thread = guard.tasks.current_thread();
        let seen = SeenByDestructor {
            scopes: guard.tasks.scopes().len(),
            tasks: guard.tasks.task_count(),
            threads: guard.tasks.thread_count(),
            current_is_task: matches!(guard.tasks.current_scope(), Some(Scope::Task(_))),
            context: thread
                .and_then(|thread| guard.tasks.thread(thread))
                .map(|record| record.context)
                .unwrap_or([0; 2]),
        };
        if let Some(record) = thread.and_then(|thread| guard.tasks.thread_mut(thread)) {
            record.context[0] = value;
        }
        seen
    }

    /// The scopes, tasks, and threads the store holds now.
    fn records(store: &Store<()>) -> (usize, usize, usize) {
        let guard = store.tables().lock().expect("handle tables");
        (
            guard.tasks.scopes().len(),
            guard.tasks.task_count(),
            guard.tasks.thread_count(),
        )
    }

    #[wcmp_macros::test]
    fn it_runs_a_host_resource_destructor_as_a_task_with_one_thread() {
        let engine = Engine::new().expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");
        let tables = store.tables_handle();
        let seen: Arc<Mutex<Option<SeenByDestructor>>> = Arc::new(Mutex::new(None));
        let recorded = seen.clone();

        let type_id = ResourceTypeId::fresh();
        store.context().register_resource(
            type_id,
            None,
            ResourceDestructor::Host(Arc::new(move |_data: &mut (), _rep: u32| {
                *recorded.lock().expect("record") = Some(record_and_set(&tables, 0xdead));
                Ok(())
            })),
        );
        let handle = store.resource_new(type_id, 7).expect("mint a handle");

        store.resource_drop(handle).expect("the host releases it");

        assert_eq!(
            seen.lock().expect("record").clone(),
            Some(SeenByDestructor {
                scopes: 1,
                tasks: 1,
                threads: 1,
                current_is_task: true,
                context: [0, 0],
            }),
            "the destructor ran as the current scope, on a task with one fresh \
             thread whose context slots started at zero"
        );
        assert_eq!(
            records(&store),
            (0, 0, 0),
            "the task, its thread, and the slot it set all ended with the \
             destructor, so nothing of it reaches the host that dropped the \
             handle"
        );
    }

    #[wcmp_macros::test]
    fn it_ends_the_destructor_task_when_a_host_destructor_fails() {
        let engine = Engine::new().expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");
        let tables = store.tables_handle();
        let seen: Arc<Mutex<Option<SeenByDestructor>>> = Arc::new(Mutex::new(None));
        let recorded = seen.clone();

        let type_id = ResourceTypeId::fresh();
        store.context().register_resource(
            type_id,
            None,
            ResourceDestructor::Host(Arc::new(move |_data: &mut (), _rep: u32| {
                *recorded.lock().expect("record") = Some(record_and_set(&tables, 0xdead));
                Err(Error::internal("the destructor failed"))
            })),
        );
        let handle = store.resource_new(type_id, 7).expect("mint a handle");

        let released = store.resource_drop(handle);

        assert!(
            released.is_err(),
            "the failing destructor failed the release"
        );
        assert_eq!(
            seen.lock()
                .expect("record")
                .as_ref()
                .map(|seen| seen.current_is_task),
            Some(true),
            "the destructor's task was the current scope while it ran"
        );
        assert_eq!(
            records(&store),
            (0, 0, 0),
            "nothing of the failed destructor is left in the store"
        );
    }

    #[wcmp_macros::test]
    fn it_keeps_the_first_name_it_learns_for_a_resource_type() {
        let engine = Engine::new().expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");

        // One identity under two labels, which is what a host
        // resource registered against two interfaces is. The store
        // keeps the first of them and renders that one; it does not
        // carry the set.
        let type_id = ResourceTypeId::fresh();
        store.context().register_resource(
            type_id,
            Some(ResourceType::new("first")),
            ResourceDestructor::Host(Arc::new(|_data: &mut (), _rep: u32| Ok(()))),
        );
        store
            .context()
            .name_resource(type_id, ResourceType::new("second"));

        let handle = store.resource_new(type_id, 1).expect("mint a handle");
        store.resource_drop(handle).expect("the host releases it");
        let err = store
            .resource_drop(handle)
            .expect_err("a released handle is not live");

        let Error::Abi(abi) = &err else {
            panic!("expected a canonical-ABI error, got {err:?}");
        };
        assert_eq!(
            abi.valtype.as_ref().and_then(|valtype| match valtype {
                ValueType::Own(resource) => Some(resource.label()),
                _ => None,
            }),
            Some("first"),
            "the store renders the first name it learned for the identity, \
             not the last, got {err}"
        );
    }

    #[wcmp_macros::test]
    fn it_lets_a_components_label_displace_a_fallback_name() {
        let engine = Engine::new().expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");

        // A fallback is what the sweep of a linker's registrations
        // leaves behind for an identity no component brought in. A
        // component that names the same identity afterwards is the
        // better name, so it takes over; a second fallback after it
        // does not take it back.
        let type_id = ResourceTypeId::fresh();
        store
            .context()
            .fallback_resource_name(type_id, ResourceType::new("swept"));
        store
            .context()
            .name_resource(type_id, ResourceType::new("imported"));
        store
            .context()
            .fallback_resource_name(type_id, ResourceType::new("swept-again"));
        store.context().register_resource(
            type_id,
            None,
            ResourceDestructor::Host(Arc::new(|_data: &mut (), _rep: u32| Ok(()))),
        );

        let handle = store.resource_new(type_id, 1).expect("mint a handle");
        store.resource_drop(handle).expect("the host releases it");
        let err = store
            .resource_drop(handle)
            .expect_err("a released handle is not live");

        let Error::Abi(abi) = &err else {
            panic!("expected a canonical-ABI error, got {err:?}");
        };
        assert_eq!(
            abi.valtype.as_ref().and_then(|valtype| match valtype {
                ValueType::Own(resource) => Some(resource.label()),
                _ => None,
            }),
            Some("imported"),
            "a label a component taught outranks a fallback, whichever \
             order the store learns them in, got {err}"
        );
    }
}
