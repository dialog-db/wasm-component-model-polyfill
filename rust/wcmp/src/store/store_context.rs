// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The store as one turn, one item, or one trampoline reaches it.

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::abi::boundary_call::BoundaryCall;
use crate::abi::signature::Signature;
use crate::concurrency::{
    Accessor, CallStatus, EntryFinish, EntryStatus, EventSlot, HostTask, InFlight, InstanceId,
    Item, ItemKind, LowerKind, Outcome, ParkedThread, PendingBlock, Plan, PollScope, Readiness,
    ResultChannel, Scheduler, Scope, SeamWait, StoreProvider, SubtaskId, SubtaskState,
    SuspendProvider, SuspendSeam, TaskId, TaskState, ThreadId, ThreadStart, TurnGuard,
    WaitableSetId, YieldWake,
};
use crate::error::{AbiCause, AbiError, AbiPosition, Error, Result, SchedulerCause, TaskCause};
use crate::executor::ResourceDestructor;
use crate::executor::ir::CanonOptions;
use crate::executor::release_subtask;
use crate::internal::{AccessorInternal, ErrorInternal};
use crate::resource::{HandleTables, ResourceHandle, ResourceTypeId, TableId};
use crate::runtime_layer::{
    AsContextMut, Func as RuntimeFunc, FuncType, StoreContextMut as RuntimeContextMut,
    Val as RuntimeVal, call_failure, substrate_failure,
};
use crate::types::ResourceType;
use crate::value::Val;

use super::resource_record::ResourceRecord;
use super::store_data::StoreData;
use super::store_id::StoreId;

pub mod internal;

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
/// # What the context does not lend
///
/// What a host function holding one of these reaches is the store's
/// host data, and the bookkeeping behind it is reached through
/// [`StoreContextInternalExt`], which `lib.rs` does not re-export.
/// The trait's entries are therefore entries only the crate can name.
/// The public face of the type imports and resolves:
///
/// ```rust
/// use wcmp::StoreContext;
/// fn host_data<'a, T: 'static>(context: &'a StoreContext<'_, T>) -> &'a T {
///     context.data()
/// }
/// ```
///
/// The seam does not, so the call behind it never gets as far as
/// being looked up:
///
/// ```compile_fail
/// use wcmp::{StoreContext, StoreContextInternalExt};
/// fn scheduler<T: 'static>(context: &mut StoreContext<'_, T>) {
///     let _ = context.internal();
/// }
/// ```
///
/// [`Store`]: super::Store
/// [`StoreContextInternalExt`]: internal::StoreContextInternalExt
pub struct StoreContext<'a, T: 'static> {
    runtime: RuntimeContextMut<'a, StoreData<T>>,
}

impl<'a, T: 'static> StoreContext<'a, T> {
    /// The store a borrow of the core store reaches.
    ///
    /// A trampoline calls this with the context the runtime layer
    /// handed it, and reaches the whole store through it.
    /// Workspace-internal; not re-exported by `lib.rs`.
    fn from_runtime(runtime: RuntimeContextMut<'a, StoreData<T>>) -> Self {
        Self { runtime }
    }

    /// Borrow this context again, for the length of the borrow of
    /// `self`. Workspace-internal.
    fn reborrow(&mut self) -> StoreContext<'_, T> {
        StoreContext {
            runtime: self.runtime.as_context_mut(),
        }
    }

    /// Borrow the core store's context, which is what every crossing
    /// of the canonical ABI and every call into the guest runs
    /// against. Workspace-internal.
    fn runtime(&self) -> &RuntimeContextMut<'a, StoreData<T>> {
        &self.runtime
    }

    /// Mutably borrow the core store's context. Workspace-internal.
    fn runtime_mut(&mut self) -> &mut RuntimeContextMut<'a, StoreData<T>> {
        &mut self.runtime
    }

    /// Everything the store carries: the host's data and the
    /// polyfill's own state. Workspace-internal.
    fn store_data(&self) -> &StoreData<T> {
        self.runtime.data()
    }

    /// Everything the store carries, mutably. Workspace-internal.
    fn store_data_mut(&mut self) -> &mut StoreData<T> {
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

    /// Mint a fresh `own<T>` handle in this store's resource table for
    /// the registered resource type `type_id`, with `rep` as its
    /// representation, as [`Store::resource_new`] does.
    ///
    /// This is the way to mint a handle where the host reaches the store
    /// only through a context: inside [`Accessor::with`], such as in the
    /// body of a host `async` function that answers with a resource.
    ///
    /// [`Store::resource_new`]: super::Store::resource_new
    /// [`Accessor::with`]: crate::Accessor::with
    pub fn resource_new(&self, type_id: ResourceTypeId, rep: u32) -> Result<ResourceHandle> {
        self.store_data().resource_new(type_id, rep)
    }

    /// The copy budget each crossing of the store starts with. See
    /// [`Store::hostcall_fuel`].
    ///
    /// [`Store::hostcall_fuel`]: super::Store::hostcall_fuel
    pub fn hostcall_fuel(&self) -> usize {
        self.store_data().hostcall_fuel()
    }

    /// Set the copy budget each later crossing of the store starts
    /// with. See [`Store::set_hostcall_fuel`].
    ///
    /// [`Store::set_hostcall_fuel`]: super::Store::set_hostcall_fuel
    pub fn set_hostcall_fuel(&mut self, fuel: usize) {
        self.store_data_mut().set_hostcall_fuel(fuel);
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
    /// Workspace-internal.
    fn tables_handle(&self) -> Arc<Mutex<HandleTables>> {
        self.store_data().tables_handle()
    }

    /// Lock the store's handle tables and record state.
    /// Workspace-internal.
    fn lock_tables(&self) -> Result<MutexGuard<'_, HandleTables>> {
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
    fn scheduler(&self) -> &Scheduler<T> {
        self.store_data().scheduler()
    }

    /// The store's cooperative scheduler, mutably.
    /// Workspace-internal.
    fn scheduler_mut(&mut self) -> &mut Scheduler<T> {
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
    fn register_resource(
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
    fn name_resource(&mut self, type_id: ResourceTypeId, name: ResourceType) {
        self.store_data_mut().name_resource(type_id, name);
    }

    /// Record a label to fall back on for `type_id` while no
    /// component in this store has named it: a host resource the
    /// linker carries that no component instantiated here brought
    /// in. It never displaces a label a component taught, and a
    /// component that names the identity later displaces it.
    /// Workspace-internal.
    fn fallback_resource_name(&mut self, type_id: ResourceTypeId, name: ResourceType) {
        self.store_data_mut().fallback_resource_name(type_id, name);
    }

    /// The name the store renders for the resource type `type_id`,
    /// when it learned one. An error about a handle of the type
    /// names it this way. Workspace-internal.
    fn resource_type(&self, type_id: ResourceTypeId) -> Option<ResourceType> {
        self.store_data().resource_type(type_id)
    }

    /// What the store knows about `type_id` at this moment, as a
    /// record an instantiation whose plan fails hands back to
    /// `restore_resource`. Workspace-internal.
    fn resource_record(&self, type_id: ResourceTypeId) -> ResourceRecord {
        self.store_data().resource_record(type_id)
    }

    /// Put back what the store knew about one resource type before
    /// an instantiation registered it, so that a failed
    /// instantiation leaves the store's registrations as it found
    /// them. Workspace-internal.
    fn restore_resource(&mut self, record: ResourceRecord) {
        self.store_data_mut().restore_resource(record);
    }

    /// How many resource types the store has a destructor
    /// registered for. Workspace-internal.
    fn registered_destructors(&self) -> usize {
        self.store_data().registered_destructors()
    }

    /// How many resource types the store has learned a name for.
    /// Workspace-internal.
    fn learned_resource_names(&self) -> usize {
        self.store_data().learned_resource_names()
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
    ///
    /// A locally-defined resource's destructor is guest code, so it
    /// runs inside a turn, which is where guest code runs. The turn
    /// takes the waker of the turn the release is already inside,
    /// and a waker that does nothing when the host released the
    /// handle from outside every turn, which is the case for a host
    /// that only drops handles. A host task the destructor starts
    /// and leaves pending therefore counts as woken, and the next
    /// turn of any driver polls it again.
    ///
    /// Two rules follow from the turn, and they are the rules every
    /// other piece of guest work gets. A driver entered from inside
    /// the destructor fails with the recursive-driver cause, because
    /// [`turn_in_flight`](Self::turn_in_flight) is true for as long
    /// as the destructor runs. A destructor that blocks on a host
    /// task through a synchronous lower blocks through the suspend
    /// seam, whose nested turns nest in this one, and a task that
    /// never resolves fails the release with the cannot-block cause.
    ///
    /// That cause holds on every target and whatever provider the
    /// engine selected, because a destructor may not block. The
    /// Canonical ABI says so under `canon resource.drop`, where the
    /// destructor call works like a synchronous cross-component call,
    /// and `canon lift` traps a call that is not `async`-typed and
    /// blocks before it returns. Wasmtime enters a destructor as a
    /// synchronous call and traps a block inside it with
    /// `Trap::CannotBlockSyncTask`. The destructor's task holds its
    /// instance's may-not-suspend flag for as long as it runs, which is
    /// what the seam reads for that rule.
    ///
    /// A host resource's destructor is the host's own closure rather
    /// than guest code. It reaches the store's host data and nothing
    /// else, so it runs outside a turn, as the host call that
    /// released the handle does.
    #[tracing::instrument(level = "trace", name = "host resource drop", skip_all)]
    fn resource_drop(&mut self, handle: ResourceHandle) -> Result<()> {
        let rep = self.store_data().remove_host_handle(handle)?;
        let Some(destructor) = self.store_data().destructor(handle.type_id()) else {
            return Ok(());
        };
        let tables = self.tables_handle();
        let instance = destructor.instance();
        match destructor {
            ResourceDestructor::Host(body) => {
                let _call = BoundaryCall::destructor(&tables, instance)?;
                body(self.data_mut(), rep)
            }
            ResourceDestructor::Local { function: slot, .. } => {
                // The handle is gone either way, as in Wasmtime: the
                // store's own table lets go of it before the entry
                // into the guest is refused.
                self.enter_guest()?;
                let waker = self.active_waker();
                let released = self.run_in_turn(&waker, move |store| {
                    let _call = BoundaryCall::destructor(&tables, instance)?;
                    let function = *slot
                        .lock()
                        .map_err(|_| Error::internal("resource destructor slot poisoned"))?;
                    let Some(function) = function else {
                        return Ok(());
                    };
                    function
                        .call(&mut store.runtime, &[RuntimeVal::I32(rep as i32)], &mut [])
                        .map_err(|err| {
                            // The call that failed is the core
                            // destructor's, whose one argument is the
                            // resource's `u32` rep, not the own handle
                            // the caller released, so the failure
                            // names no value type.
                            Error::from(AbiError {
                                position: AbiPosition::Argument(0),
                                valtype: None,
                                cause: AbiCause::SubstrateFailure(
                                    crate::runtime_layer::into_anyhow(err),
                                ),
                            })
                        })?;
                    Ok(())
                })?;
                // A destructor that failed is a trap of guest code,
                // and a trap poisons the store.
                if released.is_err() {
                    self.poison();
                }
                released
            }
        }
    }

    /// Run one turn of the store's scheduler.
    ///
    /// A turn runs every item that is ready, one at a time, each to
    /// its next yield point; then it polls the host tasks woken
    /// since the last turn, each with a waker of its own that passes
    /// its wakes on to `waker`, the waker of the driver that is
    /// polling. The waker
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
    /// A failure that ends a turn is a trap: guest code, a built-in,
    /// or the store's work on a guest's behalf failed, and nothing
    /// took the failure as a call's own. It poisons the store before
    /// the driver reports it. The one failure that ends a turn and is
    /// no trap is a failure of the host's own work, a pipe from a
    /// producer of the host's to a consumer of the host's, which
    /// touches no guest and leaves the store as it was.
    ///
    /// Workspace-internal; not re-exported by `lib.rs`.
    #[tracing::instrument(level = "trace", name = "driver turn", skip_all)]
    fn turn(&mut self, waker: &Waker) -> Result<Outcome> {
        let outcome = self.run_driver_turn(waker);
        let host_failure = self.scheduler_mut().take_host_failure();
        if outcome.is_err() && !host_failure {
            self.poison();
        }
        outcome
    }

    /// The body of [`turn`](Self::turn), which poisons the store when
    /// this fails. Workspace-internal.
    fn run_driver_turn(&mut self, waker: &Waker) -> Result<Outcome> {
        let _turn = TurnGuard::enter(self.tables(), waker);
        self.scheduler().watch_host_tasks(waker);
        loop {
            // Work a turn stopped for comes first, and nothing else
            // runs before it is done. A thread that runs on after the
            // call that resumed it returned is what such work waits
            // for, and the turn ends there: the thread runs once the
            // driver has returned control to the host executor.
            if self.run_deferred_work(waker)?.is_pending() {
                return Ok(Outcome::Resuming);
            }
            let resume = core::mem::take(&mut self.scheduler_mut().deferred_mut().turn_open);
            let outcome = self.run_turn(waker, false, None, resume)?;
            if !self.defers_work() {
                return Ok(outcome);
            }
            // An item of the turn left work to the store. The turn
            // goes on where it stopped once that work is done, and the
            // item that stopped owes the evaluation that follows it.
            let deferred = self.scheduler_mut().deferred_mut();
            deferred.turn_open = true;
            deferred.turn_note_owed |= core::mem::take(&mut deferred.note_owed);
        }
    }

    /// Whether a turn of this store is running. Workspace-internal.
    fn turn_in_flight(&self) -> bool {
        self.store_data().turn_in_flight()
    }

    /// Whether the store holds work only a turn can carry forward.
    /// Workspace-internal.
    fn has_pending_work(&self) -> bool {
        self.store_data().has_pending_work()
    }

    /// Whether the store holds an item a turn would run.
    /// Workspace-internal.
    fn has_ready_item(&self) -> bool {
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
    fn run_in_turn<R>(&mut self, waker: &Waker, body: impl FnOnce(&mut Self) -> R) -> Result<R> {
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
    /// The suspend seam states the five rules that hold for the
    /// fallback, and two of them are this turn's.
    ///
    /// A nested turn runs a resumption after a yield itself, once
    /// it has run every other ready item and polled every woken
    /// host task. It takes that resumption out of the resume-after-yield
    /// slot or off the front of the low-priority queue, runs it,
    /// and reports [`Outcome::Progress`]. It never fills the slot
    /// and never reports [`Outcome::Yield`]. A driver's turn keeps
    /// the other half of the yield rule: it defers the resumption,
    /// ends, and returns control to the host executor before the
    /// item runs. A nested turn runs from inside a guest call and
    /// has no control to return, and a task blocked on what a
    /// yielded item will produce would otherwise wait for ever on
    /// work the store was holding back.
    ///
    /// `only`, when it names an instance, holds the turn to the
    /// work of that instance: it runs no item of another instance
    /// and polls no host task. That is the lazy blocking rule of
    /// the reference for a task that must not block, which gives
    /// way to the ready threads of its own instance and then traps.
    /// Those threads include the ones suspended in the provider: the
    /// turn evaluates the waiting threads' conditions itself and
    /// queues the resumptions of the instance's own threads, which it
    /// then runs like its other items, through the provider.
    /// Such a turn reports [`Outcome::Progress`] when it ran
    /// something and [`Outcome::Idle`] when that instance had
    /// nothing to run.
    ///
    /// The other two rules belong to the seam: the cause a
    /// suspension that cannot progress fails with, and that an item
    /// a nested turn runs may block and open a nested turn of its
    /// own.
    ///
    /// With `resume`, the turn goes on with the nested turn that last
    /// stopped for work it left to the store, from the item after the
    /// one it stopped in. A turn that stops that way answers
    /// [`Outcome::Progress`] with the work pending, and one whose
    /// stop ended it also marks the work as having stopped at the
    /// turn's end, which its caller takes at once: such a turn is not
    /// gone on with. Workspace-internal.
    fn continue_nested_turn(
        &mut self,
        waker: &Waker,
        only: Option<InstanceId>,
        resume: bool,
    ) -> Result<Outcome> {
        if !resume {
            self.scheduler_mut().note_nested_turn();
        }
        self.run_turn(waker, true, only, resume)
    }

    /// The instance a nested turn run for the current task may run
    /// the work of, or `None` when that task is allowed to block.
    /// Workspace-internal.
    fn must_not_block_instance(&self) -> Option<InstanceId> {
        self.store_data().must_not_block_instance()
    }

    /// The waker of the turn that is running, or a waker that does
    /// nothing when no turn is running. Workspace-internal.
    fn active_waker(&self) -> Waker {
        self.store_data().active_waker()
    }

    /// Why a driver that went idle failed. Workspace-internal.
    fn idle_cause(&self) -> SchedulerCause {
        self.store_data().idle_cause()
    }

    /// Why a nested turn that went idle with its condition unmet
    /// failed. Workspace-internal.
    fn suspend_cause(&self) -> SchedulerCause {
        self.store_data().suspend_cause()
    }

    /// Why a nested turn held to `instance` went idle with its
    /// condition unmet. Workspace-internal.
    fn suspend_cause_in(&self, instance: InstanceId) -> SchedulerCause {
        self.store_data().suspend_cause_in(instance)
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
    /// go back, and the failure is a trap of the guest task that made
    /// the call. It poisons the store and travels out to the call the
    /// guest still has on the stack, and from there to the driver.
    /// A poll that poisoned the store fails the guest's call with the
    /// cannot-enter cause, whatever the body answered.
    ///
    /// Through a synchronous lower the guest expects the result when
    /// the call returns, so a body that is still running has to block
    /// the guest thread where it stands. The body is parked among the
    /// store's host tasks, and the block goes through the suspend
    /// seam on every target until the poll that completes the body
    /// resolves the call, so a body that resolves after a few polls
    /// resolves inside the block and the call returns its result. A
    /// body that stays pending leaves the store waiting and the call
    /// fails with the cause the seam selects.
    ///
    /// Workspace-internal; not re-exported by `lib.rs`.
    fn start_host_task(
        &mut self,
        task: HostTask<T>,
        caller: TableId,
        lower: LowerKind,
    ) -> Result<CallStatus> {
        match lower {
            LowerKind::Sync => {
                self.block_on_host_task(task)?;
                Ok(CallStatus::returned())
            }
            LowerKind::Async => self.start_async_host_task(task, caller),
        }
    }

    /// Start the host task of one call made through an asynchronous
    /// lower, and report what the guest is told: the returned status
    /// when its first poll resolved it, and the started status with
    /// the subtask's index in `caller` otherwise.
    fn start_async_host_task(
        &mut self,
        mut task: HostTask<T>,
        caller: TableId,
    ) -> Result<CallStatus> {
        let subtask = task
            .subtask()
            .ok_or_else(|| Error::internal("a copy's host task was started as a call"))?;
        let waker = self.active_waker();
        // The body reaches the host data through the accessor this
        // poll hands it and through nothing else, for the length of
        // a closure it runs with it.
        let outcome = task.poll(self, &waker);

        match outcome {
            // The call never returned, so the subtask's resolution
            // is a cancellation, and the handles the guest lent for
            // it are given back all the same. The failure is a trap
            // of the guest task that made the call, which poisons
            // the store. The guest's call is on the stack, so the
            // failure travels out through it to the driver, and
            // nothing crosses.
            Poll::Ready(Err(error)) => {
                self.lock_tables()?.abandon_subtask(subtask);
                self.poison();
                Err(error)
            }
            // The poll itself poisoned the store, after the trap had
            // let go of every host task the store held, so the task
            // goes as they went rather than joining the store, and a
            // result it produced crosses into no guest: the guest
            // code that would read it is guest code a poisoned store
            // does not run.
            _ if self.poisoned() => self.abandon_poisoned_call(subtask, task),
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
            Poll::Pending => {
                let index = {
                    let mut guard = self.lock_tables()?;
                    // The host task joins the store's records here,
                    // so a call past the cap on them fails as one
                    // whose body failed would.
                    if let Err(error) = guard.tasks.admit_records(1) {
                        guard.abandon_subtask(subtask);
                        return Err(error);
                    }
                    // The subtask starts before its entry is made,
                    // because the status word this call returns is
                    // what tells the caller it started. A subtask
                    // that started while the caller already held an
                    // entry would take on the start event instead,
                    // which is the callee the entry gate held and not
                    // this call.
                    guard.tasks.start_subtask(subtask);
                    let index = guard.insert_subtask(caller, subtask);
                    // The guest runs on while the host side does, so
                    // the subtask is no longer the scope the guest's
                    // work counts against. Its record stays, and with
                    // it the handles the call borrowed, until its
                    // resolution is delivered.
                    if guard.tasks.current_subtask() == Some(subtask) {
                        guard.tasks.pop_scope();
                    }
                    index
                };
                task.started_in(caller);
                self.push_host_task(task);
                Ok(CallStatus::started(index))
            }
        }
    }

    /// The first part of a synchronous lower of a host `async`
    /// function: poll the call's body once, and park it when it is
    /// still running.
    ///
    /// A body that resolves at once has its result lowered here, and
    /// the call is over: the answer is `None`. A body that fails is a
    /// call that never returned: its subtask resolves as a
    /// cancellation, the handles the guest lent for it go back, the
    /// store is poisoned, and the failure travels out to the guest's
    /// call. A poll that poisoned the store fails the guest's call
    /// with the cannot-enter cause, whatever the body answered.
    ///
    /// A body that is still running is parked among the store's host
    /// tasks, in every case, and the answer is the subtask of its
    /// call, whose resolution is what the guest thread waits for.
    /// Turns poll the parked task with the driver's waker, as they
    /// poll every host task, and the poll that completes it settles
    /// it: the subtask resolves, which makes the waiting thread ready,
    /// and what the body produced stays in the store for
    /// [`finish_blocking_host_task`](Self::finish_blocking_host_task).
    /// The store therefore always knows about the pending future, and
    /// the cause a block that gives up fails with reads it there.
    fn begin_blocking_host_task(&mut self, mut task: HostTask<T>) -> Result<Option<SubtaskId>> {
        let subtask = task
            .subtask()
            .ok_or_else(|| Error::internal("a copy's host task was started as a call"))?;
        let waker = self.active_waker();
        match task.poll(self, &waker) {
            // The failure is a trap of the guest task that made the
            // call, which poisons the store.
            Poll::Ready(Err(error)) => {
                self.lock_tables()?.abandon_subtask(subtask);
                self.poison();
                Err(error)
            }
            // The poll poisoned the store, and the task is not
            // parked, as no host task outlives the trap. A result it
            // produced crosses into no guest.
            _ if self.poisoned() => self.abandon_poisoned_call(subtask, task),
            Poll::Ready(Ok(values)) => {
                self.lock_tables()?
                    .exit_subtask(subtask, SubtaskState::Returned);
                task.lower(self, Ok(values))?;
                Ok(None)
            }
            Poll::Pending => {
                // The host task joins the store's records here, so a
                // call past the cap on them fails as one whose body
                // failed would.
                let mut guard = self.lock_tables()?;
                if let Err(error) = guard.tasks.admit_records(1) {
                    guard.abandon_subtask(subtask);
                    return Err(error);
                }
                drop(guard);
                self.scheduler_mut().park_call(task);
                Ok(Some(subtask))
            }
        }
    }

    /// The finish part of a synchronous lower of a host `async`
    /// function whose body was still running, once the wait on the
    /// call's subtask went as `waited` says.
    ///
    /// It delivers the subtask's resolution, which gives back the
    /// handles the guest lent for the call, exactly as a call whose
    /// first poll resolved the body delivers it, and it lowers what
    /// the body produced. A body that failed and a wait that failed
    /// are each a call that never returned: the subtask resolves as a
    /// cancellation, the parked task leaves the store, and the
    /// failure travels out to the guest's call.
    fn finish_blocking_host_task(&mut self, subtask: SubtaskId, waited: Result<()>) -> Result<()> {
        let settled = self.scheduler_mut().take_settled_call(subtask);
        match (waited, settled) {
            (Ok(()), Some((task, Ok(values)))) => {
                self.lock_tables()?
                    .exit_subtask(subtask, SubtaskState::Returned);
                task.lower(self, Ok(values))
            }
            // The body failed, so the poll that saw it resolved the
            // subtask as a cancellation already, and the failure
            // travels out to the guest's call.
            (Ok(()), Some((_task, Err(error)))) => {
                let mut guard = self.lock_tables()?;
                let state = guard
                    .tasks
                    .subtask(subtask)
                    .map_or(SubtaskState::CancelledBeforeReturned, |record| record.state);
                guard.exit_subtask(subtask, state);
                Err(error)
            }
            (Err(error), _) => {
                self.scheduler_mut().withdraw_call(subtask);
                self.lock_tables()?.abandon_subtask(subtask);
                Err(error)
            }
            (Ok(()), None) => {
                // The cause is read with the task still parked, so a
                // body that can still resolve names the stack switch.
                let cause = self.suspend_cause();
                self.scheduler_mut().withdraw_call(subtask);
                self.lock_tables()?.abandon_subtask(subtask);
                Err(Error::Scheduler(cause))
            }
        }
    }

    /// Let go of the host task of the call `subtask` records, whose
    /// first poll poisoned the store while its guest caller was on
    /// the stack. The trap already let go of every other host task,
    /// and this one goes the same way, its future dropped with no
    /// lock held, whether the body had completed or not: a result it
    /// produced is not lowered. The call never returned, so its
    /// subtask resolves as a cancellation and the handles the guest
    /// lent for it go back. The guest's call fails with the
    /// cannot-enter cause: a poisoned store runs no more guest code,
    /// and the caller's code is the guest code that would run next.
    fn abandon_poisoned_call<R>(&mut self, subtask: SubtaskId, task: HostTask<T>) -> Result<R> {
        drop(task);
        self.lock_tables()?.abandon_subtask(subtask);
        Err(Error::Task(TaskCause::CannotEnter))
    }

    /// Block the guest thread on a host task, where it stands, which
    /// is what a synchronous lower of a host `async` function comes
    /// to when its thread cannot suspend its stack: the guest expects
    /// the result when the call returns, so the call cannot return
    /// until the body does.
    ///
    /// The block has the two parts of every blocking built-in:
    /// [`begin_blocking_host_task`](Self::begin_blocking_host_task),
    /// a wait through the suspend seam until the call's subtask
    /// resolves, and
    /// [`finish_blocking_host_task`](Self::finish_blocking_host_task).
    /// The seam's fallback polls the parked task before each nested
    /// turn it runs, which is what serves a body that stays pending
    /// once or twice and a caller that must not block.
    ///
    /// A wait that unwinds takes the parked task out of the store as
    /// well, pending or settled, before the panic carries on. An item
    /// a nested turn runs and a host task it polls can each panic,
    /// and a task left parked after the frame that parked it was gone
    /// would keep the store holding a host future nothing waits on:
    /// every later block would read it as one that can still resolve,
    /// and a later turn would settle it into a call that no longer
    /// exists. The task sits in the store's own data rather than
    /// behind a handle a guard could hold, so the unwind is caught
    /// here and resumed once the task is out.
    fn block_on_host_task(&mut self, task: HostTask<T>) -> Result<()> {
        let Some(subtask) = self.begin_blocking_host_task(task)? else {
            return Ok(());
        };
        let waited = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            SuspendSeam::wait_until(&mut *self, Readiness::Subtask { subtask })
        }))
        .unwrap_or_else(|panic| {
            self.scheduler_mut().withdraw_call(subtask);
            std::panic::resume_unwind(panic)
        });
        self.finish_blocking_host_task(subtask, waited)
    }

    /// Give a host task to the store. It counts as woken, so the next
    /// turn polls it with its own waker and no wake is lost.
    /// Workspace-internal.
    fn push_host_task(&mut self, task: HostTask<T>) {
        self.scheduler_mut().push_host_task(task);
    }

    /// The body of one turn, with the waker already recorded.
    ///
    /// `nested` marks the turn the suspend seam runs from inside a
    /// guest call. The two turns differ over the resumptions after
    /// a yield: a driver's turn defers one and ends, so that
    /// control goes back to the host executor before the item runs,
    /// and a nested turn, which has no control to give back, runs
    /// the resumption itself once nothing else is ready.
    ///
    /// A driver's turn that has run an item ends with `Progress`
    /// before it defers a resumption, and the next turn defers it.
    /// The driver consults its condition in between, so a call whose
    /// result is in its slot returns and leaves the resumption in the
    /// low-priority queue, behind the work the next call queues. That
    /// is Wasmtime's order: it polls the future it was given before it
    /// takes each work item, and takes a low-priority item only when
    /// the future is still pending. A thread the call left ready, of
    /// a task that outlives the call, therefore runs after the start
    /// of the next call and not before it.
    ///
    /// `only`, when it names an instance, holds the turn to that
    /// instance's work, which is what a task that must not block
    /// gives way to: its items, and the resumptions of its threads
    /// suspended in the provider whose condition holds. Such a turn
    /// polls no host task: a task that must not block must not wait
    /// on one, and the cause it fails with says so.
    ///
    /// The entry gate is opened at the top of the turn, once the
    /// turn has run everything that was ready, and once more after
    /// the host tasks have been polled. The middle one is the open
    /// that ends a turn: a task it lets through belongs to the next
    /// turn, not to this one, and the turn that released it ends
    /// with `Progress` so that whichever loop is running turns
    /// consults what it is waiting for first. That is how a call
    /// that already has its result is answered before the store
    /// runs the callee the gate was holding while the caller ran.
    /// The open at the top is the other side of the same rule: it
    /// collects what the turn before released, and what it lets
    /// through is this turn's own work.
    ///
    /// The open after the host tasks is what makes work a host
    /// task's body released or queued past the gate ready before the
    /// turn reports `Waiting` or `Idle`: a body reaches the store
    /// through its accessor, so a poll of one can fill the wait a
    /// callback is held for, or free the instance a start is held
    /// at the gate for, and work that became ready there would
    /// otherwise wait for the wake that running it is what produces.
    ///
    /// None of that breaks a deadlock. A task that waits on a start
    /// while holding the instance that start needs waits for
    /// something only it can give back, so the gate stays shut and
    /// the turn reports `Waiting` or `Idle` as it found it.
    ///
    /// An item that fails ends the turn and its failure is the
    /// turn's: a trap of the guest work the item ran, whichever task
    /// that work belongs to, or a failure of the item's own
    /// bookkeeping. The first trap ends the driver that is polling.
    /// No item keeps a trap for the call that started its task, not
    /// even a `Func::call`'s: that call's future may have been dropped
    /// before a later driver's turn ran the task.
    #[tracing::instrument(level = "trace", name = "turn", skip_all, fields(nested = nested))]
    fn run_turn(
        &mut self,
        waker: &Waker,
        nested: bool,
        only: Option<InstanceId>,
        resume: bool,
    ) -> Result<Outcome> {
        // A turn that goes on after the work an item left to the
        // store has been done starts again with the next item, as
        // though the item had just returned, and it has run one.
        let mut ran = resume;
        if !resume {
            self.open_entry_gate()?;
            if !nested {
                let resumed = self.scheduler_mut().take_resume_after_yield();
                if let Some(item) = resumed {
                    ran = true;
                    self.run_item(item)?;
                    if self.defers_work() {
                        return Ok(Outcome::Progress);
                    }
                }
            }
        }
        loop {
            let ready = match only {
                Some(instance) => self.scheduler_mut().take_ready_in(instance),
                None => self.scheduler_mut().take_ready(),
            };
            if let Some(item) = ready {
                ran = true;
                self.run_item(item)?;
                if self.defers_work() {
                    return Ok(Outcome::Progress);
                }
                continue;
            }
            // Work this turn released — a task the entry gate can
            // now let through, a callback whose wait an event
            // answered — is fresh readiness, and the turn hands it
            // to the turn that follows rather than running it
            // itself. The turn ends with progress, so nothing waits
            // on it: whichever loop runs turns comes straight back
            // for the next one.
            //
            // Ending the turn is the point of it. A driver consults
            // its condition between two turns, so a call whose
            // result is already in its slot is answered before the
            // store runs what the call left behind: the task of a
            // callee the gate held while the caller ran, and
            // whatever that task goes on to fail with. Running the
            // release here instead would let such a callee
            // overwrite an answer its caller already has.
            //
            // What this guarantees is that a release ends the turn,
            // and no more than that. Wasmtime polls the future it
            // was given ahead of every work item; a turn here still
            // runs everything that is ready in one unbroken run,
            // with no condition consulted between two items of it.
            // The two agree on the case that matters — work the
            // gate released never runs in the turn that released
            // it — and a driver that wants a look in between two
            // items that were ready together does not get one.
            if self.open_entry_gate()? {
                return Ok(Outcome::Progress);
            }
            if !nested && self.scheduler().has_deferred_item() {
                if ran {
                    return Ok(Outcome::Progress);
                }
                self.scheduler_mut().defer_low_priority();
                return Ok(Outcome::Yield);
            }
            // A turn held to one instance never reaches the evaluation
            // that follows the poll of the host tasks below, so a
            // thread of the instance suspended in the provider whose
            // condition came to hold before the turn, or through what
            // a thread outside any item did, would never be resumed
            // from inside the block. The turn evaluates the conditions
            // here and queues the resumptions of the instance's own
            // threads, which it then runs like any other item of the
            // instance: through the provider, from inside the block.
            if let Some(instance) = only
                && self.note_ready_threads_in(instance)?
            {
                continue;
            }
            break;
        }
        if let Some(instance) = only {
            // The instance's own work is the whole of what this turn
            // was allowed to run, a resumption of that instance
            // after a yield included: nothing else of it is ready,
            // so the yield has given way to everything it can.
            // Progress sends the seam round again to test its
            // condition; an idle answer is its cue to stop and trap
            // with the cannot-block cause.
            if let Some(item) = self.scheduler_mut().take_deferred_in(instance) {
                self.run_item(item)?;
                self.note_stop_at_turn_end();
                return Ok(Outcome::Progress);
            }
            return Ok(if ran {
                Outcome::Progress
            } else {
                Outcome::Idle
            });
        }
        self.poll_host_tasks(waker)?;
        self.note_ready_threads()?;
        // A host task's body reaches the store through its accessor,
        // so a poll of one can have satisfied what the store was
        // holding back: a callback whose wait it filled the event
        // for, or a task the gate can now release. The gate is
        // therefore opened once more before the turn decides it has
        // nothing left to run, or work that became ready in the poll
        // would wait for the wake that running it is what produces.
        self.open_entry_gate()?;
        if self.scheduler().has_immediate_item() {
            return Ok(Outcome::Progress);
        }
        // Only deferred work is left. A nested turn runs it: every
        // other ready item has run and every woken host task has
        // been polled, so the yield has given way to all there was, and
        // the turn has no control to hand the host executor first.
        // A driver's turn does not reach this, because the loop
        // above deferred the front of the queue and returned
        // already.
        if nested {
            if let Some(item) = self.scheduler_mut().take_deferred() {
                self.run_item(item)?;
                self.note_stop_at_turn_end();
                return Ok(Outcome::Progress);
            }
        } else if self.scheduler().has_deferred_item() {
            return Ok(Outcome::Yield);
        }
        if self.scheduler().host_task_count() == 0 {
            return Ok(Outcome::Idle);
        }
        Ok(Outcome::Waiting)
    }

    /// Make ready every piece of work the store was holding back:
    /// the callback items whose condition now holds, then the tasks
    /// the entry gate can release. While the store holds neither,
    /// which is every turn of the synchronous baseline, it costs one
    /// look at the tables.
    ///
    /// The held callbacks go first, so that a task whose wait a
    /// previous turn satisfied resumes ahead of a task that has yet
    /// to enter its instance.
    ///
    /// Answers whether anything was released, which is what tells a
    /// turn that it produced fresh readiness. The count of what is
    /// still held is the measure: everything this releases is queued
    /// and leaves the two holdings, so the answer is true exactly
    /// when something that was held no longer is.
    ///
    /// The look happens even when nothing is held, because it is also
    /// what takes the list of waitable sets signalled since the last
    /// look, which would otherwise grow without bound in a store that
    /// never holds a callback item.
    fn open_entry_gate(&mut self) -> Result<bool> {
        let held = self.held_work();
        if held == 0 {
            // Nothing is held, so nothing is released. The signals go
            // all the same, as the release would take them.
            self.lock_tables()?.tasks.take_signalled_sets();
            return Ok(false);
        }
        // The tables are reached through a handle of their own, so
        // that the guard on them and the borrow of the scheduler,
        // which the store's data holds, do not overlap.
        let tables = self.tables_handle();
        let mut guard = Self::lock(&tables)?;
        self.scheduler_mut().release_held_callbacks(&mut guard)?;
        self.scheduler_mut().open_entry_gate(&mut guard.tasks);
        drop(guard);
        Ok(self.held_work() < held)
    }

    /// How many pieces of work the store is holding back: the tasks
    /// at the entry gates and the held callback items.
    fn held_work(&self) -> usize {
        self.scheduler().waiting_at_gate() + self.scheduler().held_callbacks()
    }

    /// Poll the host tasks woken since the last turn polled any, in
    /// the order they were woken, each with a waker of its own.
    ///
    /// A host task that joined since the last turn counts as woken.
    /// One that was not woken is not polled: its future said it would
    /// wake when it could go on, and the store holds it until then. A
    /// wake reaches the store through the task's own waker, which
    /// marks the task for the next turn and wakes `waker`, the waker
    /// of the driver polling the store, so the executor still hears
    /// of it.
    ///
    /// Each poll puts this store in the thread's slot and hands the
    /// body an accessor to it, which is how a body that has to read
    /// the host data reaches it. A body that completes queues the
    /// lowering of what it produced into the subtask that awaits it.
    ///
    /// A call's body that fails is a trap of the guest task that made
    /// the call, which can never resolve its subtask. It poisons the
    /// store at once, which lets go of every other host task, and the
    /// failure ends this turn, so the driver whose turn polled the
    /// body reports it. That holds whichever guest task made the call
    /// and whether or not the call that started that task has
    /// returned.
    fn poll_host_tasks(&mut self, waker: &Waker) -> Result<()> {
        // The waker a nested turn polls with when no turn is running
        // is one that does nothing, and passing wakes on to that one
        // would lose them for the driver that polls the store next.
        if self.turn_in_flight() {
            self.scheduler().watch_host_tasks(waker);
        }
        let woken = self.scheduler_mut().take_woken_host_tasks();
        if woken.is_empty() {
            return Ok(());
        }
        let mut completed = Vec::new();
        let mut failed = None;
        for (key, task_waker, mut task) in woken {
            // A poll before this one can have poisoned the store: its
            // body reached the store and ran a destructor that
            // trapped. The trap let go of every host task the store
            // held but the ones this turn has out, and those go here,
            // unpolled.
            if self.scheduler().host_task_retired(key) {
                self.scheduler_mut().complete_host_task(key);
                drop(task);
                continue;
            }
            // The failure of an abort ends the turn as a failed call
            // does, once every other task this turn took is back.
            if self.scheduler().host_task_aborted(key) {
                if let Err(error) = self.abort_host_task(key, &task_waker, task) {
                    failed.get_or_insert(error);
                }
                continue;
            }
            let outcome = task.poll(self, &task_waker);
            // The poll itself poisoned the store, and the task goes
            // the way of every other the trap let go of, whatever the
            // poll answered: what it produced crosses into no guest.
            if self.scheduler().host_task_retired(key) {
                self.scheduler_mut().complete_host_task(key);
                drop(task);
                continue;
            }
            match outcome {
                Poll::Ready(Err(error)) if task.subtask().is_some() => {
                    self.scheduler_mut().complete_host_task(key);
                    // The subtask of a synchronous lower is still on
                    // the stack of the guest thread that waits on it,
                    // and that thread's block gives it back as the
                    // failure unwinds through it.
                    let parked = task
                        .subtask()
                        .is_some_and(|subtask| self.scheduler().is_parked_call(subtask));
                    let error = if parked {
                        drop(task);
                        self.poison();
                        error
                    } else {
                        task.trap(self, error)
                    };
                    failed.get_or_insert(error);
                }
                Poll::Ready(value) => {
                    self.scheduler_mut().complete_host_task(key);
                    completed.push((task, value));
                }
                Poll::Pending => self.scheduler_mut().restore_host_task(key, task),
            }
        }
        // A call that failed poisoned the store, and what the other
        // polls completed crosses into no guest.
        if let Some(error) = failed {
            drop(completed);
            return Err(error);
        }
        for (task, value) in completed {
            // The task of a synchronous lower settles where it
            // completed. Its caller is still inside the call, so there
            // is no subtask event to fill and nothing to queue: the
            // resolution is what the caller's readiness condition
            // watches for.
            if let Some(subtask) = task.subtask()
                && self.scheduler().is_parked_call(subtask)
            {
                self.settle_call(subtask, task, value)?;
                continue;
            }
            self.scheduler_mut()
                .push_high_priority(task.lowering_item(value));
        }
        Ok(())
    }

    /// End the host task handed out under `key`, whose caller
    /// cancelled the call: the task leaves the store, its body is
    /// dropped unpolled, and the subtask resolves as cancelled before
    /// it returned, which takes on the subtask event the caller waits
    /// for. Nothing crosses into the guest.
    ///
    /// The drop happens here, in the turn, never in the built-in that
    /// asked for it: a body's `Drop` can reach the store through its
    /// accessor, and it reaches it as a poll would. A drop that
    /// poisoned the store resolves nothing, as a poll that poisoned it
    /// crosses nothing.
    fn abort_host_task(&mut self, key: u64, waker: &Waker, task: HostTask<T>) -> Result<()> {
        self.scheduler_mut().complete_host_task(key);
        let subtask = task.abort(self, waker);
        if self.poisoned() {
            return Ok(());
        }
        if let Some(subtask) = subtask {
            self.lock_tables()?
                .tasks
                .subtask_cancelled_by_callee(subtask)?;
        }
        Ok(())
    }

    /// Settle the parked host task of the synchronous lower of the
    /// call `subtask` records: `outcome` is what its body produced.
    ///
    /// The subtask resolves, which is what the waiting thread's
    /// readiness condition watches for, and the task and its outcome
    /// stay in the store for the finish part of the lower. A value
    /// resolves it as returned. A failure resolves it as cancelled,
    /// because the call never returned, and the finish part hands the
    /// failure to the guest's call. Nothing crosses into the guest
    /// here: the lower's finish part runs the lowering once the
    /// thread resumes.
    fn settle_call(
        &mut self,
        subtask: SubtaskId,
        task: HostTask<T>,
        outcome: Result<Vec<Val>>,
    ) -> Result<()> {
        {
            let mut guard = self.lock_tables()?;
            match outcome {
                Ok(_) => guard.tasks.subtask_returned(subtask)?,
                Err(_) => guard.tasks.subtask_cancelled(subtask)?,
            }
        }
        self.scheduler_mut().settle_call(subtask, task, outcome);
        Ok(())
    }

    /// Poll the parked host task of the synchronous lower of the call
    /// `subtask` records, once, with the task's own waker, and settle
    /// it when the poll completes it. Nothing happens when the store
    /// holds no such task pending, or when a turn has it out.
    ///
    /// The suspend seam's fallback calls this before each nested turn
    /// it runs for the lower, which is what makes a block poll the
    /// call's own future at every check of its condition. A nested turn
    /// held to one instance polls no host task, and a future that
    /// answered pending without asking for a wake would otherwise never
    /// be polled again inside the block.
    fn poll_parked_call(&mut self, subtask: SubtaskId) -> Result<()> {
        // The task's own waker passes a wake on to the waker of the
        // turn that is running, as a turn's poll of it does.
        if self.turn_in_flight() {
            let waker = self.active_waker();
            self.scheduler().watch_host_tasks(&waker);
        }
        let Some((key, waker, mut task)) = self.scheduler_mut().take_parked_call(subtask) else {
            return Ok(());
        };
        let outcome = task.poll(self, &waker);
        // A poll that poisoned the store lets the task go, as the trap
        // let go of every other host task, whatever the poll answered:
        // a call settled now would hand its result to guest code a
        // poisoned store never runs.
        if self.scheduler().host_task_retired(key) {
            self.scheduler_mut().complete_host_task(key);
            drop(task);
            return Ok(());
        }
        match outcome {
            Poll::Ready(value) => {
                self.scheduler_mut().complete_host_task(key);
                // A body that failed is a trap of the guest task that
                // made the call, and poisons the store. The settle
                // comes after the poison, which would discard it, and
                // hands the failure to the guest's call, which is on
                // the stack and carries it out to the driver.
                if value.is_err() {
                    self.poison();
                }
                self.settle_call(subtask, task, value)
            }
            Poll::Pending => {
                self.scheduler_mut().restore_host_task(key, task);
                Ok(())
            }
        }
    }

    /// Evaluate the readiness condition of every waiting thread and
    /// note the threads that became ready, which a turn does between
    /// two items. The evaluation only reads the tables.
    fn note_ready_threads(&mut self) -> Result<()> {
        self.lock_tables()?.tasks.note_ready_threads();
        self.queue_resumptions(None).map(|_| ())
    }

    /// Evaluate the readiness condition of every waiting thread, as
    /// [`note_ready_threads`](Self::note_ready_threads) does, and
    /// queue the resumptions of the threads of `instance` alone,
    /// answering whether it queued any. This is the evaluation of a
    /// turn held to `instance`.
    fn note_ready_threads_in(&mut self, instance: InstanceId) -> Result<bool> {
        self.lock_tables()?.tasks.note_ready_threads();
        self.queue_resumptions(Some(instance))
    }

    /// Run one item of a turn, and evaluate the conditions of the
    /// waiting threads after it, since the item can have met them.
    #[tracing::instrument(level = "trace", name = "turn item", skip_all)]
    fn run_item(&mut self, item: Item<T>) -> Result<()> {
        // A guest work item queued after a trap, by guest code that
        // was unwinding from it, is dropped rather than run: a
        // poisoned store runs no more guest code. The host's own work
        // still runs.
        if self.poisoned() && !item.is_host_only() {
            drop(item);
            return Ok(());
        }
        self.scheduler_mut().note_item_run();
        let host_only = item.is_host_only();
        if let Err(error) = item.run(self) {
            // The host's own work touches no guest, so its failure is
            // no trap when no guest frame lies below it: the failure
            // ends the driver's turn, and nothing more. Below a guest
            // frame, in a nested turn, it unwinds through that frame,
            // which is a trap.
            if host_only && self.scope_depth()? == 0 {
                self.scheduler_mut().note_host_failure();
            }
            return Err(error);
        }
        if self.defers_work() {
            // The item stopped for work it left to the store, and is
            // done only once that work is. The evaluation that follows
            // it waits until then too.
            self.scheduler_mut().deferred_mut().note_owed = true;
            return Ok(());
        }
        self.note_ready_threads()
    }

    /// Mark the work the item that ended a nested turn left to the
    /// store, when it left any, as work the turn stopped for at its
    /// end.
    fn note_stop_at_turn_end(&mut self) {
        if self.defers_work() {
            self.scheduler_mut().deferred_mut().stopped_at_end = true;
        }
    }

    /// Create the task of one call into an export, without making it
    /// the current scope: the task's thread runs when a turn runs the
    /// item that starts it. Workspace-internal.
    fn create_export_task(
        &self,
        function: Arc<Signature>,
        options: Arc<CanonOptions>,
        instance: InstanceId,
    ) -> Result<TaskId> {
        self.lock_tables()?
            .tasks
            .create_task(Some(function), Some(options), instance)
    }

    /// Queue `item` as the start of `task`'s implicit thread, past
    /// the entry gate of `instance`. Workspace-internal.
    fn start_export_thread(
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

    /// Queue the item the scheduler's switch slot holds as the start
    /// of `task`'s implicit thread, past the entry gate of
    /// `instance`. A task the gate holds clears the slot and waits
    /// at the gate in arrival order. Workspace-internal.
    fn start_switched_export_thread(
        &mut self,
        task: TaskId,
        instance: InstanceId,
        async_function: bool,
        needs_exclusive: bool,
    ) -> Result<()> {
        let tables = self.tables_handle();
        let mut guard = Self::lock(&tables)?;
        self.scheduler_mut().enter_implicit_thread_in_switch_slot(
            &mut guard.tasks,
            task,
            instance,
            async_function,
            needs_exclusive,
        );
        Ok(())
    }

    /// Run the one item the scheduler's switch slot holds, and
    /// nothing else. This is the nested turn restricted to the
    /// thread the scheduler must switch to next, which is what the
    /// trampoline of a call between two components runs from inside
    /// the caller's frame. Does nothing when the slot is empty.
    /// Workspace-internal.
    fn run_switch_slot(&mut self) -> Result<()> {
        let Some(item) = self.scheduler_mut().take_switch_slot() else {
            return Ok(());
        };
        self.run_item(item)
    }

    /// The provider that fills the store's suspend capability, or
    /// `None` when the engine selected none. The store keeps the
    /// provider for its whole life; this hands out a handle to it.
    /// Workspace-internal.
    fn provider(&self) -> Option<StoreProvider> {
        self.store_data().provider().cloned()
    }

    /// Run the store's flight, the start or the resume of a thread a
    /// turn left for the driver, until the thread stops, inside a turn
    /// with the waker of the driver that awaits it. Workspace-internal;
    /// see [`StoreContextInternal::fly`](internal::StoreContextInternal::fly).
    async fn fly(&mut self) {
        let Some(provider) = self.provider() else {
            return;
        };
        let waker = core::future::poll_fn(|context| Poll::Ready(context.waker().clone())).await;
        let tables = self.tables_handle();
        let _turn = TurnGuard::enter(&tables, &waker);
        provider.fly(self).await;
    }

    /// Whether the store's owner dropped it while a thread the
    /// provider resumed had yet to run. That thread finds the store
    /// kept for it, and must run nothing in it. Workspace-internal.
    fn dropped(&self) -> bool {
        self.store_data().dropped()
    }

    /// Whether a trap poisoned the store. Workspace-internal.
    fn poisoned(&self) -> bool {
        self.store_data().poisoned()
    }

    /// Record that a trap happened in the store, so that no guest
    /// code of it runs again, and discard the work it holds.
    ///
    /// Every queued guest work item goes, wherever it waits, and so
    /// does every host task, producer, and consumer, each future
    /// dropped here. The task and subtask records stay until the
    /// store drops. A later driver therefore meets no stale work, and
    /// fails only for an entry it makes itself. The work frames left to
    /// the store, such as a switcher to take back or a plan, goes at
    /// the next driver's first turn, which starts and resumes no thread
    /// of a poisoned store. Wasmtime keeps its
    /// queued items and host futures, and a later `run_concurrent`
    /// runs them; the polyfill discards them, because the Component
    /// Model runs no guest code after a trap.
    ///
    /// Every caller holds no lock on the handle tables, so a future's
    /// `Drop` that reaches the store runs where host code may run: an
    /// accessor it reaches through fails with the store-not-in-poll
    /// or the recursive-driver cause, as it would anywhere else a
    /// poll of the store is not lending it. Workspace-internal.
    fn poison(&mut self) {
        // The work goes at the moment the store is poisoned, and only
        // then. What reaches a poisoned store afterwards is the
        // host's own work, which still runs, or an item that guest
        // code unwinding from the trap queued, which no turn runs.
        if self.poisoned() {
            return;
        }
        self.store_data_mut().poison();
        let discarded = self.scheduler_mut().discard_all_work();
        // The borrow of the scheduler has ended, and no lock is held,
        // so the host code a drop runs meets neither.
        drop(discarded);
    }

    /// Refuse a host entry into a guest of a store a trap poisoned,
    /// with the cannot-enter cause. Every entry that would run guest
    /// code asks this before it changes anything. Workspace-internal.
    fn enter_guest(&self) -> Result<()> {
        if self.poisoned() {
            return Err(Error::Task(TaskCause::CannotEnter));
        }
        Ok(())
    }

    /// How deep the stack of current scopes is, which is where the
    /// scopes of a thread entry about to start begin.
    /// Workspace-internal.
    fn scope_depth(&self) -> Result<usize> {
        Ok(self.lock_tables()?.tasks.scopes().len())
    }

    /// The implicit thread of `task`. Workspace-internal.
    fn implicit_thread(&self, task: TaskId) -> Result<ThreadId> {
        self.lock_tables()?
            .tasks
            .task(task)
            .map(|record| record.implicit_thread)
            .ok_or_else(|| Error::internal("a task's thread started with no task in the store"))
    }

    /// Run `entry`, a thread entry of `thread`, with `args`, and hand
    /// what it produced to `finish`: its core results, or the trap it
    /// raised. `results` are the slots a direct call fills.
    ///
    /// A thread entry is a guest function that starts a thread: a
    /// task's core function, a callback, or a thread's start
    /// function. The caller is an item of a turn or a trampoline,
    /// and `finish` is the part of it that runs once the entry
    /// returns. `base` is how deep the stack of current scopes was
    /// before the caller pushed the scopes of the thread.
    ///
    /// Without a provider the entry is a direct call on the real
    /// stack, and `finish` runs as it returns.
    ///
    /// With a provider the entry starts through the provider's start,
    /// on a stack of its own, and the thread is marked as running on
    /// one, so that a blocking built-in it reaches suspends it. The
    /// start returns when the entry finishes or first suspends. An
    /// entry that finished hands its results to `finish` at once, as
    /// the switch module's entry wrapper handed them to the host
    /// whether or not the entry suspended on the way. An entry that
    /// suspended leaves the real stack with the scopes it pushed from
    /// `base` up, and waits among the parked threads with `finish`
    /// until the scheduler resumes it. The caller goes on either way:
    /// a trampoline that made a nested start continues once the new
    /// thread suspends or finishes, as the reference's `canon_lower`
    /// continues once the `thread.resume` it made returns.
    /// Workspace-internal.
    #[tracing::instrument(level = "trace", name = "thread entry", skip_all)]
    fn run_thread_entry(
        &mut self,
        thread: ThreadId,
        base: usize,
        entry: &RuntimeFunc,
        args: &[RuntimeVal],
        results: Vec<RuntimeVal>,
        finish: impl EntryFinish<T>,
    ) -> Result<()> {
        let ty = entry.ty(self.runtime()).map_err(substrate_failure)?;
        let mark = self.scheduler().switcher_mark();
        self.start_thread_entry(thread, base, entry, ty.as_ref(), args, results, finish)?;
        self.follow_switches(mark)
    }

    /// Run `entry` as [`run_thread_entry`](Self::run_thread_entry)
    /// does, up to the moment it finishes or first suspends, and leave
    /// a switch the thread made as it suspended for the caller.
    ///
    /// Under a provider that hands over a failure only after the start
    /// returned, an entry that failed before it first suspended leaves
    /// its failure to the store, as a resumed thread's stop is: the
    /// scheduler waits for the failure before anything else, and runs
    /// the finish with it then.
    #[allow(clippy::too_many_arguments)]
    fn start_thread_entry(
        &mut self,
        thread: ThreadId,
        base: usize,
        entry: &RuntimeFunc,
        ty: Option<&FuncType>,
        args: &[RuntimeVal],
        results: Vec<RuntimeVal>,
        finish: impl EntryFinish<T>,
    ) -> Result<()> {
        let Some(provider) = self.provider() else {
            let mut results = results;
            let called = entry
                .call(self.runtime_mut(), args, &mut results)
                .map_err(call_failure)
                .map(|()| results);
            return finish(self, called);
        };
        // A provider starts the entry through a wrapper of the entry's
        // own type, so it needs the type, which an engine knows for
        // every function a guest exports.
        let ty =
            ty.ok_or_else(|| Error::internal("a thread entry has no type the engine knows"))?;
        let task = {
            let mut guard = self.lock_tables()?;
            guard.tasks.set_own_stack(thread, true);
            guard.tasks.note_returns_to(thread, base);
            guard
                .tasks
                .thread(thread)
                .map(|record| record.task)
                .ok_or_else(|| Error::internal("a thread entry started for no thread"))?
        };
        // Whether the store runs no guest code, which is where a start
        // that becomes the store's flight keeps its scopes on the stack
        // until the driver awaits it.
        let at_rest = provider.may_resume_here(self);
        // A start the first part of a blocking built-in makes, from inside
        // a guest call, under a provider that runs a thread only once the
        // driver awaits it, is left to the store where the built-in's
        // thread suspends for it. Only then does the thread's end reach
        // the scheduler whole, a trap's reason included. Every other start
        // from inside a guest call starts the thread in place.
        let may_defer = core::mem::take(&mut self.scheduler_mut().deferred_mut().may_defer_start);
        let defer =
            may_defer && !at_rest && provider.resumes_later() && provider.may_suspend_here(self);
        let started = if defer {
            provider.defer_start(self, thread, entry, ty, args)
        } else {
            provider.start(self, thread, entry, ty, args)
        };
        match started {
            Ok(EntryStatus::Suspended) => {
                let (scopes, running) = self.lock_tables()?.tasks.cut_scopes(base);
                self.scheduler_mut()
                    .park_thread(thread, ParkedThread::new(task, scopes, running, finish));
                self.note_plan_of(thread);
                Ok(())
            }
            Ok(EntryStatus::Finished(values)) => {
                self.lock_tables()?.tasks.set_own_stack(thread, false);
                finish(self, Ok(values))
            }
            // The start is the store's flight: the thread runs once the
            // driver awaits it, and the store waits for its stop before
            // anything else. A start made where the store runs no guest
            // code keeps its scopes on the stack, as a resumed thread's
            // are, since the turn ends there.
            Ok(EntryStatus::Running) if at_rest => {
                let parked = ParkedThread::new(task, Vec::new(), Vec::new(), finish);
                let deferred = self.scheduler_mut().deferred_mut();
                if deferred.resumed.is_some() {
                    return Err(Error::internal(
                        "a thread started while another thread's stop is awaited",
                    ));
                }
                deferred.resumed = Some(InFlight {
                    thread,
                    parked,
                    base: Some(base),
                });
                deferred.resume_issued = true;
                Ok(())
            }
            // A start made from inside a guest call becomes the store's
            // flight too, and the thread that made it suspends for it,
            // leaving the rest of its built-in as a plan. The started
            // thread leaves the stack with its scopes until the store
            // runs it, and they go back on the stack then, above the
            // scopes of the plan's thread, as they were when it started.
            Ok(EntryStatus::Running) => {
                let (scopes, running) = self.lock_tables()?.tasks.cut_scopes(base);
                let parked = ParkedThread::new(task, scopes, running, finish);
                let deferred = self.scheduler_mut().deferred_mut();
                if deferred.deferred_start.is_some() {
                    return Err(Error::internal(
                        "a thread started while another deferred start waits",
                    ));
                }
                deferred.deferred_start = Some(InFlight {
                    thread,
                    parked,
                    base: None,
                });
                Ok(())
            }
            Err(error) => {
                if let Ok(mut guard) = self.lock_tables() {
                    guard.tasks.set_own_stack(thread, false);
                }
                finish(self, Err(error))
            }
        }
    }

    /// Resume `thread`, which is suspended in the provider, and run
    /// it until it suspends again or its entry finishes. A thread
    /// that is not parked, which is one whose task ended while it
    /// waited, is not resumed.
    ///
    /// The scopes the thread left the real stack with go back on top
    /// of it, so the thread finds the stack as it left it, whatever
    /// runs below. A thread that suspends again takes them off again.
    /// A thread whose entry finishes runs the finish its starter left
    /// with it. A thread that suspends in a switch has the thread it
    /// named run next, from here, as
    /// [`follow_switches`](Self::follow_switches) states.
    /// Workspace-internal.
    fn resume_parked_thread(&mut self, thread: ThreadId) -> Result<()> {
        let mark = self.scheduler().switcher_mark();
        self.resume_thread_once(thread)?;
        self.follow_switches(mark)
    }

    /// Run the resumption a turn queued for `thread`, which resumes
    /// the suspension numbered `number`. A thread that has resumed
    /// since, through a switch that named it, and suspended again is
    /// in a later suspension, which its own condition resumes: the
    /// queued resumption is spent, and does nothing.
    fn run_queued_resumption(&mut self, thread: ThreadId, number: u64) -> Result<()> {
        if self.scheduler().parked_number(thread) != Some(number) {
            return Ok(());
        }
        self.resume_parked_thread(thread)
    }

    /// Resume `thread` as
    /// [`resume_parked_thread`](Self::resume_parked_thread) does, up
    /// to the moment it suspends again or finishes, and leave a switch
    /// it made as it suspended for the caller.
    ///
    /// A provider that runs the thread on a microtask resumes it only
    /// where the store runs no guest code, and runs it after the frame
    /// that resumed it returned. Anywhere else the resumption is left
    /// to the store, as the switch the thread would be: the thread is
    /// named to run next, and the frame goes no further. Where it may,
    /// the resume leaves the thread's stop to the store, which waits
    /// for it before anything else and does the rest then.
    #[tracing::instrument(level = "trace", name = "resume thread", skip_all)]
    fn resume_thread_once(&mut self, thread: ThreadId) -> Result<()> {
        let Some(provider) = self.provider() else {
            return Ok(());
        };
        if !self.scheduler().is_parked(thread) {
            return Ok(());
        }
        if provider.resumes_later()
            && (self.defers_work()
                || self.scheduler().deferred().resumed.is_some()
                || !provider.may_resume_here(self))
        {
            self.scheduler_mut().leave_resumption(thread);
            return Ok(());
        }
        let Some(mut parked) = self.scheduler_mut().take_parked_thread(thread) else {
            return Ok(());
        };
        parked.queued = false;
        let base = {
            let mut guard = self.lock_tables()?;
            let base = guard.tasks.scopes().len();
            guard.tasks.note_returns_to(thread, base);
            let scopes = core::mem::take(&mut parked.scopes);
            let running = core::mem::take(&mut parked.running);
            guard.tasks.restore_scopes(scopes, running);
            base
        };
        let resumed = provider.resume(self, thread);
        self.thread_stopped(thread, parked, base, resumed)
    }

    /// Act on where `thread`, which ran with its scopes on the stack
    /// from `base` up, stopped: `stopped` is what its start or resume
    /// answered, or what the provider answered once it stopped. A
    /// thread that suspended again is parked again, one that finished
    /// or failed runs the finish `parked` carries, and one that runs on
    /// leaves its stop to the store.
    fn thread_stopped(
        &mut self,
        thread: ThreadId,
        mut parked: ParkedThread<T>,
        base: usize,
        stopped: Result<EntryStatus>,
    ) -> Result<()> {
        match stopped {
            Ok(EntryStatus::Suspended) => {
                let (scopes, running) = self.lock_tables()?.tasks.cut_scopes(base);
                parked.scopes = scopes;
                parked.running = running;
                self.scheduler_mut().park_thread(thread, parked);
                self.note_plan_of(thread);
                Ok(())
            }
            Ok(EntryStatus::Finished(values)) => {
                self.lock_tables()?.tasks.set_own_stack(thread, false);
                parked.finish(self, Ok(values))
            }
            Ok(EntryStatus::Running) => {
                let deferred = self.scheduler_mut().deferred_mut();
                deferred.resumed = Some(InFlight {
                    thread,
                    parked,
                    base: Some(base),
                });
                deferred.resume_issued = true;
                Ok(())
            }
            Err(error) => {
                if let Ok(mut guard) = self.lock_tables() {
                    guard.tasks.set_own_stack(thread, false);
                }
                parked.finish(self, Err(error))
            }
        }
    }

    /// Take the plan the trampoline of the parked `thread` left as the
    /// thread suspended, when it left one: the plan is the thread's,
    /// and holds it until the plan is done. The plan waits among the
    /// plans stops left until the scheduler takes it up.
    fn note_plan_of(&mut self, thread: ThreadId) {
        let Some(mut plan) = self.scheduler_mut().deferred_mut().request.take() else {
            return;
        };
        plan.owner = Some(thread);
        if let Some(parked) = self.scheduler_mut().parked_mut(thread) {
            parked.held = true;
        }
        self.scheduler_mut().deferred_mut().stopped.push(plan);
    }

    /// Whether the frame that runs now left work to the store that it
    /// must not go past: under a provider that resumes a thread on a
    /// microtask, a thread whose stop the store waits for, a
    /// resumption the frame could not make, or a plan a thread's
    /// suspension left. Always `false` under every other provider.
    /// Workspace-internal.
    fn defers_work(&self) -> bool {
        self.scheduler().deferred().pending()
    }

    /// Whether the store is inside such work: a turn stopped for it,
    /// a plan runs, or work is pending. A driver consults its
    /// condition only when the store is not. Workspace-internal.
    fn deferred_busy(&self) -> bool {
        self.scheduler().deferred().busy()
    }

    /// Leave `plan` for the thread the running trampoline runs in,
    /// which is about to suspend in the trampoline's shim, and give it
    /// the marks and the evaluation the work it waits for owes. It
    /// fails with the stack-switch cause when that thread cannot
    /// suspend here, which is when a host frame lies between the start
    /// of its stack and the shim, or no provider suspends a thread
    /// that way. The work stays with the store then, and the marks
    /// come off at once, as the trampoline returns. Workspace-internal.
    fn leave_plan(&mut self, mut plan: Plan<T>) -> Result<()> {
        let may = match self.provider() {
            Some(provider) => provider.may_suspend_here(self),
            None => false,
        };
        let deferred = self.scheduler_mut().deferred_mut();
        plan.ends_nested_start = deferred.ends_nested_start.take();
        plan.restores_may_not_suspend = deferred.nested_may_not_suspend.take();
        plan.ends_thread_switch = core::mem::take(&mut deferred.ends_thread_switch);
        plan.note_owed = core::mem::take(&mut deferred.note_owed);
        if !may {
            self.end_marks(
                plan.ends_nested_start,
                plan.restores_may_not_suspend,
                plan.ends_thread_switch,
            )?;
            return Err(Error::Scheduler(SchedulerCause::StackSwitchNeeded));
        }
        debug_assert!(
            self.scheduler().deferred().request.is_none(),
            "a plan was left while another one waits for its thread to suspend"
        );
        self.scheduler_mut().deferred_mut().request = Some(plan);
        Ok(())
    }

    /// Take off the marks a trampoline that left a plan put on the
    /// stack: the nested-start mark of the subtask `nested_start` names
    /// and its thread-switch mark, as `thread_switch` says, and put back the
    /// may-not-suspend flag the nested start cleared, as
    /// `may_not_suspend` says.
    fn end_marks(
        &mut self,
        nested_start: Option<SubtaskId>,
        may_not_suspend: Option<(InstanceId, bool)>,
        thread_switch: bool,
    ) -> Result<()> {
        let mut guard = self.lock_tables()?;
        if let Some(subtask) = nested_start {
            guard.tasks.end_nested_start(subtask);
        }
        if let Some((instance, old)) = may_not_suspend {
            guard.tasks.set_may_not_suspend(instance, old);
        }
        if thread_switch {
            guard.tasks.end_thread_switch();
        }
        Ok(())
    }

    /// Do the work frames left to the store, before anything else and
    /// with nothing else in between, until none is left or a thread
    /// runs on a microtask, which is when this answers pending: the
    /// provider wakes `waker` once the thread stopped.
    ///
    /// A failure of work a plan runs is a failure of the built-in the
    /// plan serves, as it would have failed the built-in's own frame:
    /// the plan ends with it, and its thread resumes with it. A
    /// failure with no plan to take it is the turn's.
    fn run_deferred_work(&mut self, waker: &Waker) -> Result<Poll<()>> {
        loop {
            match self.step_deferred_work(waker) {
                Ok(Some(true)) => {}
                Ok(Some(false)) => return Ok(Poll::Ready(())),
                Ok(None) => return Ok(Poll::Pending),
                Err(error) => {
                    let Some(plan) = self.scheduler_mut().deferred_mut().plans.pop() else {
                        let deferred = self.scheduler_mut().deferred_mut();
                        deferred.turn_open = false;
                        deferred.turn_note_owed = false;
                        return Err(error);
                    };
                    self.finish_plan(plan, Err(error))?;
                }
            }
        }
    }

    /// Take one step of the work frames left to the store. It answers
    /// `Some(true)` when it did something, `Some(false)` when nothing
    /// is left, and `None` while a thread runs on a microtask.
    ///
    /// The order is fixed: the thread a turn resumed, then the plans
    /// that stops left, the outermost first so that the innermost runs
    /// first, then the failure of a start, then the thread named to run
    /// next, then a switcher recorded at the level the store runs now,
    /// then the innermost plan.
    ///
    /// A store a trap poisoned takes only the first step: the thread a
    /// turn resumed was running before the driver came back to it, and
    /// nothing can call it back, so its stop is awaited as ever. The
    /// rest would start or resume a thread, and is let go of instead.
    fn step_deferred_work(&mut self, waker: &Waker) -> Result<Option<bool>> {
        match self.take_stop(waker, false)? {
            Some(true) => return Ok(Some(true)),
            Some(false) => {}
            None => return Ok(None),
        }
        if self.poisoned() {
            self.release_deferred_work()?;
            return Ok(Some(false));
        }
        let stopped = core::mem::take(&mut self.scheduler_mut().deferred_mut().stopped);
        if !stopped.is_empty() {
            for plan in stopped.into_iter().rev() {
                self.activate_plan(plan)?;
            }
            return Ok(Some(true));
        }
        match self.take_stop(waker, true)? {
            Some(true) => return Ok(Some(true)),
            Some(false) => {}
            None => return Ok(None),
        }
        if let Some(next) = self.scheduler_mut().take_next_thread() {
            self.enter_switched_thread(next)?;
            return Ok(Some(true));
        }
        if let Some(switcher) = self.scheduler_mut().pop_switcher_at_level() {
            self.take_back_switcher(switcher)?;
            return Ok(Some(true));
        }
        if self.scheduler().deferred().plans.is_empty() {
            if core::mem::take(&mut self.scheduler_mut().deferred_mut().turn_note_owed) {
                self.note_ready_threads()?;
            }
            return Ok(Some(false));
        }
        self.step_plan()?;
        Ok(Some(true))
    }

    /// Let go of the work frames left to the store, for a store a trap
    /// poisoned, without starting or resuming a thread: a start a frame
    /// left, the thread named to run next, the switchers to take back,
    /// and every plan, with the marks each plan's trampoline put on the
    /// stack and the scopes of the plans the store runs. The threads
    /// stay where they are, suspended in the provider, until the store
    /// drops, and so does a thread whose start never ran. Nothing is
    /// left that keeps the store busy, so a later driver consults its
    /// condition as in a store with no such work, and runs only host
    /// work.
    fn release_deferred_work(&mut self) -> Result<()> {
        let (start, stopped, plans) = {
            let deferred = self.scheduler_mut().deferred_mut();
            deferred.request = None;
            deferred.turn_note_owed = false;
            (
                deferred.deferred_start.take(),
                core::mem::take(&mut deferred.stopped),
                core::mem::take(&mut deferred.plans),
            )
        };
        if let Some(start) = start
            && let Some(provider) = self.provider()
        {
            provider.abandon_start(start.thread);
        }
        self.scheduler_mut().take_next_thread();
        while self.scheduler_mut().pop_switcher_above(0).is_some() {}
        if let Some(outermost) = plans.first() {
            self.lock_tables()?.tasks.cut_scopes(outermost.base);
        }
        for plan in stopped.into_iter().chain(plans) {
            if let Some(parked) = plan
                .owner
                .and_then(|owner| self.scheduler_mut().parked_mut(owner))
            {
                parked.held = false;
            }
            self.end_marks(
                plan.ends_nested_start,
                plan.restores_may_not_suspend,
                plan.ends_thread_switch,
            )?;
        }
        Ok(())
    }

    /// Act on the stop of the thread a turn resumed, or, with
    /// `deferred_start`, of the thread whose start a frame inside a
    /// guest call left to the store, once the provider has it. It
    /// answers `Some(true)` when it acted, `Some(false)` when there is
    /// no such thread, and `None` while the thread has not stopped.
    ///
    /// A deferred start left the stack with the thread's scopes, which
    /// go back on it before the thread first runs, above the scopes of
    /// the thread that started it, as they were when it started.
    fn take_stop(&mut self, waker: &Waker, deferred_start: bool) -> Result<Option<bool>> {
        let Some(thread) = self
            .scheduler_mut()
            .deferred_mut()
            .in_flight(deferred_start)
            .as_ref()
            .map(|in_flight| in_flight.thread)
        else {
            return Ok(Some(false));
        };
        let restore = self
            .scheduler_mut()
            .deferred_mut()
            .in_flight(deferred_start)
            .as_mut()
            .filter(|in_flight| in_flight.base.is_none())
            .map(|in_flight| {
                (
                    core::mem::take(&mut in_flight.parked.scopes),
                    core::mem::take(&mut in_flight.parked.running),
                )
            });
        if let Some((scopes, running)) = restore {
            let base = {
                let mut guard = self.lock_tables()?;
                let base = guard.tasks.scopes().len();
                guard.tasks.note_returns_to(thread, base);
                guard.tasks.restore_scopes(scopes, running);
                base
            };
            // From here the thread runs as a resumed thread does: the
            // driver runs it, and the frames it runs are its own, with
            // no work left to the store.
            let deferred = self.scheduler_mut().deferred_mut();
            if deferred.resumed.is_some() {
                return Err(Error::internal(
                    "a deferred start ran while a resumed thread's stop is awaited",
                ));
            }
            deferred.resumed = deferred.deferred_start.take().map(|in_flight| InFlight {
                base: Some(base),
                ..in_flight
            });
            return self.take_stop(waker, false);
        }
        let provider = self
            .provider()
            .ok_or_else(|| Error::internal("a thread runs on in a store with no provider"))?;
        let stopped = match provider.poll_stop(self, thread, waker) {
            // Control goes back to the driver now, which runs the
            // thread: the frames it runs see no resume left to them.
            Poll::Pending => {
                self.scheduler_mut().deferred_mut().resume_issued = false;
                return Ok(None);
            }
            Poll::Ready(stopped) => stopped,
        };
        let InFlight {
            thread,
            parked,
            base,
        } = self
            .scheduler_mut()
            .deferred_mut()
            .in_flight(deferred_start)
            .take()
            .ok_or_else(|| Error::internal("a thread in flight went missing"))?;
        let base = base.ok_or_else(|| Error::internal("a thread stopped with no scopes"))?;
        self.thread_stopped(thread, parked, base, stopped)?;
        Ok(Some(true))
    }

    /// Take up `plan`, whose owner suspended for it: the owner's scopes
    /// go back on the stack for as long as the plan runs, as they
    /// would be on it if the trampoline had done the work itself.
    fn activate_plan(&mut self, mut plan: Plan<T>) -> Result<()> {
        let owner = plan
            .owner
            .ok_or_else(|| Error::internal("a plan was taken up with no thread"))?;
        let (scopes, running) = match self.scheduler_mut().parked_mut(owner) {
            Some(parked) => (
                core::mem::take(&mut parked.scopes),
                core::mem::take(&mut parked.running),
            ),
            None => (Vec::new(), Vec::new()),
        };
        {
            let mut guard = self.lock_tables()?;
            plan.base = guard.tasks.scopes().len();
            guard.tasks.restore_scopes(scopes, running);
        }
        self.scheduler_mut().deferred_mut().plans.push(plan);
        Ok(())
    }

    /// Run the innermost plan on, once no work is left above it. The
    /// work the trampoline left is done then: the item that stopped
    /// for it owes its evaluation no longer, and the marks the
    /// trampoline put on the stack come off. Then the plan's wait
    /// runs, from where it stopped. A plan whose wait stops again
    /// stays; one that is done ends, and its owner resumes.
    ///
    /// The plan stays among the plans until it ends, so that a failure
    /// of its work on the way ends it and no other.
    fn step_plan(&mut self) -> Result<()> {
        let (note_owed, nested_start, may_not_suspend, thread_switch, then_wait) = {
            let plan = self.innermost_plan()?;
            (
                core::mem::take(&mut plan.note_owed),
                plan.ends_nested_start.take(),
                plan.restores_may_not_suspend.take(),
                core::mem::take(&mut plan.ends_thread_switch),
                plan.then_wait.take(),
            )
        };
        if note_owed {
            self.note_ready_threads()?;
        }
        self.end_marks(nested_start, may_not_suspend, thread_switch)?;
        if let Some(readiness) = then_wait {
            let wait = if readiness == Readiness::Yielded {
                SeamWait::give_way(self)?
            } else {
                SeamWait::until(self, readiness)?
            };
            self.innermost_plan()?.wait = Some(wait);
        }
        let wait = self.innermost_plan()?.wait.take();
        let outcome = match wait {
            None => Ok(()),
            Some(mut wait) => match wait.run(self) {
                // The wait is over, which ends the thread's part in it.
                Some(outcome) => outcome,
                None => {
                    let owed = core::mem::take(&mut self.scheduler_mut().deferred_mut().note_owed);
                    let plan = self.innermost_plan()?;
                    plan.wait = Some(wait);
                    plan.note_owed = owed;
                    return Ok(());
                }
            },
        };
        let plan = self
            .scheduler_mut()
            .deferred_mut()
            .plans
            .pop()
            .ok_or_else(|| Error::internal("a plan went missing as it ran"))?;
        self.finish_plan(plan, outcome)
    }

    /// The innermost plan the store runs.
    fn innermost_plan(&mut self) -> Result<&mut Plan<T>> {
        self.scheduler_mut()
            .deferred_mut()
            .plans
            .last_mut()
            .ok_or_else(|| Error::internal("a plan went missing as it ran"))
    }

    /// End `plan` with `outcome`, and resume its owner, whose shim
    /// tries the built-in again and reads the outcome.
    ///
    /// The owner's scopes come off the stack as it suspended with
    /// them. A plan of a built-in that suspends its thread records the
    /// built-in's condition on the thread now, as the built-in would
    /// have once its first part returned, and resumes the thread only
    /// when the condition holds: otherwise the thread waits as any
    /// suspended thread does. An owner whose task ended while the plan
    /// ran is not resumed.
    fn finish_plan(&mut self, plan: Plan<T>, outcome: Result<()>) -> Result<()> {
        let owner = plan
            .owner
            .ok_or_else(|| Error::internal("a plan ended with no thread"))?;
        let (scopes, running) = self.lock_tables()?.tasks.cut_scopes(plan.base);
        let Some(parked) = self.scheduler_mut().parked_mut(owner) else {
            return Ok(());
        };
        parked.scopes = scopes;
        parked.running = running;
        parked.held = false;
        let resume = match (plan.suspends, outcome) {
            (true, Ok(())) => self.record_planned_wait(owner)?,
            (_, outcome) => {
                if let Some(block) = self.scheduler_mut().block_mut(plan.blocked) {
                    block.waited = Some(outcome);
                }
                true
            }
        };
        if resume {
            self.resume_thread_once(owner)?;
        }
        Ok(())
    }

    /// Record the condition of the built-in the thread `owner` waits
    /// in, whose first part left a plan, on the thread's record, and
    /// answer whether the thread goes on at once, which is when the
    /// condition holds and is not a yield's.
    fn record_planned_wait(&mut self, owner: ThreadId) -> Result<bool> {
        let Some(readiness) = self.scheduler().block_readiness(owner) else {
            return Ok(true);
        };
        match readiness {
            Readiness::Planned => Ok(true),
            Readiness::Resumed { thread } => {
                Ok(!self.lock_tables()?.tasks.thread_suspended(thread))
            }
            _ => {
                let (previous, holds) = {
                    let mut guard = self.lock_tables()?;
                    let previous = guard.tasks.start_waiting(owner, readiness)?;
                    (previous, guard.tasks.readiness_holds(readiness))
                };
                if let Some(block) = self.scheduler_mut().block_mut(owner) {
                    block.previous = previous;
                }
                Ok(readiness != Readiness::Yielded && holds)
            }
        }
    }

    /// Run the threads that switches named, one after another, which
    /// is the reference's `Thread.resume` loop.
    ///
    /// A thread that suspends in `thread.suspend-then-resume`, or in
    /// one of the three other built-ins that switch, names the thread
    /// to run next, and the frame that started or resumed it runs that
    /// thread before anything else, once the switching thread has left
    /// the real stack. That frame is a turn's item, or a trampoline
    /// that made a nested start. The named thread may switch again as
    /// it suspends, and the loop goes on until a thread suspends
    /// without naming one, or finishes. Each thread runs from this one
    /// frame, so a chain of switches of any length adds no depth to
    /// the real stack.
    ///
    /// A thread of a task that must not block that suspended to
    /// switch, since the frame read `mark`, comes back to this frame
    /// once the chain stops. The frame resumes it at once when it is
    /// ready then, and follows its switches in turn. That is the loop
    /// of the reference's `canon_lift` for a sync-typed task, which
    /// runs the ready threads of the task's instance, and nothing
    /// else, until the task resolves. A switcher that is not ready is
    /// left to the scheduler.
    ///
    /// Under a provider that resumes a thread on a microtask, the loop
    /// stops as soon as a thread it runs leaves work to the store, and
    /// the store goes on with the loop once that work is done.
    fn follow_switches(&mut self, mark: usize) -> Result<()> {
        while !self.defers_work() {
            if let Some(next) = self.scheduler_mut().take_next_thread() {
                self.enter_switched_thread(next)?;
                continue;
            }
            let Some(switcher) = self.scheduler_mut().pop_switcher_above(mark) else {
                break;
            };
            self.take_back_switcher(switcher)?;
        }
        Ok(())
    }

    /// Take back `switcher`, a thread of a task that must not block
    /// that suspended in the provider to switch, once the threads its
    /// switch ran have stopped: resume it when it is ready, and leave
    /// it to the scheduler otherwise.
    fn take_back_switcher(&mut self, switcher: ThreadId) -> Result<()> {
        let ready = self.scheduler().is_parked(switcher)
            && self.lock_tables()?.tasks.thread_ready(switcher);
        if ready {
            self.resume_thread_once(switcher)?;
        }
        Ok(())
    }

    /// Run `thread`, which a switch named, up to the moment it
    /// suspends or finishes, and leave a switch it made as it
    /// suspended for the caller. A thread that has never run starts;
    /// one suspended in the provider resumes, and is suspended no
    /// more, since the switch is the resume that names it. One that
    /// waits on the real stack, in the block of the frame this runs
    /// in, is suspended no more either, and goes on once this returns
    /// to that block.
    fn enter_switched_thread(&mut self, thread: ThreadId) -> Result<()> {
        let start = self.lock_tables()?.tasks.take_thread_start(thread);
        if let Some((task, start)) = start {
            return self.start_explicit_thread(thread, task, start);
        }
        if let Some(record) = self.lock_tables()?.tasks.thread_mut(thread) {
            record.suspended = false;
        }
        self.resume_thread_once(thread)
    }

    /// Run `thread`, which a switch named from a built-in that runs on
    /// the real stack, from inside that built-in: start it when it has
    /// never run, and resume it when it is suspended in the provider,
    /// then run whatever it switches to in turn. It returns once the
    /// chain of threads suspends or finishes. Workspace-internal.
    fn run_switched_thread(&mut self, thread: ThreadId) -> Result<()> {
        let mark = self.scheduler().switcher_mark();
        self.enter_switched_thread(thread)?;
        self.follow_switches(mark)
    }

    /// Start the explicit thread `thread`, which `thread.resume-later`
    /// made ready before it ever ran, and run it until it suspends or
    /// finishes, then run whatever it switches to. A thread a switch
    /// started first has nothing left to start, and this does
    /// nothing. Workspace-internal.
    fn start_ready_thread(&mut self, thread: ThreadId) -> Result<()> {
        let Some((task, start)) = self.lock_tables()?.tasks.take_thread_start(thread) else {
            return Ok(());
        };
        let mark = self.scheduler().switcher_mark();
        self.start_explicit_thread(thread, task, start)?;
        self.follow_switches(mark)
    }

    /// Start the explicit thread `thread` of `task`, whose start
    /// `start` is, as a thread entry, up to the moment it suspends or
    /// finishes.
    ///
    /// The thread runs in its task's scope. Its end is the same
    /// whether its start function returned or trapped: it leaves its
    /// instance's table and its task. A trap poisons the store and
    /// fails whatever resumed or started the thread, which carries it
    /// out to the driver whose turn ran the thread, whichever task the
    /// thread belongs to and whether or not that task's call has
    /// already returned. A thread that returned and was the
    /// last of a task whose implicit thread has exited ends the task,
    /// as [`end_last_thread`](Self::end_last_thread) states.
    fn start_explicit_thread(
        &mut self,
        thread: ThreadId,
        task: TaskId,
        start: ThreadStart,
    ) -> Result<()> {
        let base = self.scope_depth()?;
        self.lock_tables()?
            .tasks
            .enter_thread(thread)
            .ok_or_else(|| Error::internal("a thread started whose record is not in the store"))?;
        let finish = move |store: &mut StoreContext<'_, T>, called: Result<Vec<RuntimeVal>>| {
            {
                let mut guard = store.lock_tables()?;
                guard.leave_thread(thread);
                guard.tasks.end_thread(thread);
            }
            match called {
                Ok(_) => store.end_last_thread(task),
                // A trap of the thread poisons the store, and ends
                // the turn that ran the thread.
                Err(error) => {
                    store.poison();
                    Err(error)
                }
            }
        };
        // A thread's start function takes its context, an `i32` or an
        // `i64` to match its memory, and returns nothing, whatever the
        // table it came from says of it: a reference read out of a
        // table does not always carry its type.
        let context = match start.context {
            RuntimeVal::I64(_) => crate::runtime_layer::ValType::I64,
            _ => crate::runtime_layer::ValType::I32,
        };
        let ty = FuncType::new([context], []);
        self.start_thread_entry(
            thread,
            base,
            &start.function,
            Some(&ty),
            &[start.context],
            Vec::new(),
            finish,
        )
    }

    /// Fail every thread suspended in the provider with the cause an
    /// idle store gives, as though the wait of each had failed there,
    /// and answer whether any was suspended. A driver does this when
    /// its store goes idle: nothing left in the store can resume those
    /// threads.
    ///
    /// The threads fail in the order they last suspended, so a thread
    /// a nested start began fails before the thread that began it, as
    /// a trap unwinds the innermost frame first: the starter was still
    /// running above the start when the started thread suspended, and
    /// it suspended after that, however often either had suspended
    /// before. Each thread's scopes
    /// go back on the stack, and the built-in it waits in finishes
    /// with the failed wait, which gives back what the built-in took
    /// and answers the trap the guest sees. The thread's own finish
    /// then runs with that trap, which is what ends its task and
    /// hands the failure to whoever waits on the call: the caller of
    /// the export, or the thread that started it. That is the failure
    /// the same block raises with no provider, where the wait traps
    /// inside the built-in and the trap unwinds every frame of the
    /// thread. The thread's continuation stays in the switch module's
    /// table, never resumed, until the store drops.
    ///
    /// A finish that fails itself fails the driver: the first such
    /// failure is the answer. Workspace-internal.
    fn fail_parked_threads(&mut self) -> Result<bool> {
        let threads = self.scheduler().parked_in_order();
        if threads.is_empty() {
            return Ok(false);
        }
        let mut failed = None;
        for thread in threads {
            let Some(mut parked) = self.scheduler_mut().take_parked_thread(thread) else {
                continue;
            };
            let block = self.scheduler_mut().end_block(thread);
            let base = {
                let mut guard = self.lock_tables()?;
                guard.tasks.set_own_stack(thread, false);
                let base = guard.tasks.scopes().len();
                let scopes = core::mem::take(&mut parked.scopes);
                let running = core::mem::take(&mut parked.running);
                guard.tasks.restore_scopes(scopes, running);
                base
            };
            // The built-in the thread waits in fails first, with the
            // cause, which gives back what its first part took and
            // answers the trap the guest sees. A thread that waits in
            // none traps with the cause itself.
            let cause = self.idle_cause();
            let trap = match block {
                Some(block) => {
                    self.lock_tables()?
                        .tasks
                        .stop_waiting(thread, block.previous);
                    match block.step.finish(self, Err(Error::Scheduler(cause))) {
                        Err(trap) => call_failure(trap),
                        Ok(_) => Error::Scheduler(self.idle_cause()),
                    }
                }
                None => Error::Scheduler(cause),
            };
            let finished = parked.finish(self, Err(trap));
            // Whatever the finish left above the thread's scopes goes
            // with it, as a trap's unwind takes it.
            self.lock_tables()?.tasks.cut_scopes(base);
            if let Err(error) = finished {
                failed.get_or_insert(error);
            }
        }
        match failed {
            Some(error) => Err(error),
            None => Ok(true),
        }
    }

    /// Queue the resumption of every thread suspended in the provider
    /// whose condition now holds and whose resumption is not queued
    /// yet, in the order the threads became ready.
    ///
    /// A thread that yielded resumes after every other ready item,
    /// behind the low-priority queue, and only once a driver has
    /// returned control to the host executor, as every resumption
    /// after a yield does. Any other thread resumes as fresh
    /// readiness. The resumption belongs to the thread's instance, so
    /// a turn held to that instance runs it, and to its task, so it
    /// goes with the task's record.
    ///
    /// `only`, when it names an instance, queues the resumptions of
    /// that instance's threads alone, which is all a turn held to the
    /// instance may run. The threads of other instances stay as they
    /// are, for the next evaluation of a turn that may run them. It
    /// answers whether it queued any.
    fn queue_resumptions(&mut self, only: Option<InstanceId>) -> Result<bool> {
        if self.scheduler().parked_threads() == 0 {
            return Ok(false);
        }
        let ready = {
            let guard = self.lock_tables()?;
            guard
                .tasks
                .ready_threads()
                .into_iter()
                .map(|thread| {
                    let record = guard.tasks.thread(thread);
                    let yielded =
                        record.and_then(|record| record.readiness) == Some(Readiness::Yielded);
                    let instance = record
                        .and_then(|record| guard.tasks.task(record.task))
                        .and_then(|task| task.instance);
                    (thread, yielded, instance)
                })
                .filter(|(_, _, instance)| only.is_none() || *instance == only)
                .collect::<Vec<_>>()
        };
        let mut queued = false;
        for (thread, yielded, instance) in ready {
            let Some((task, number)) = self.scheduler_mut().queue_resumption(thread) else {
                continue;
            };
            queued = true;
            let mut item = Item::new(
                ItemKind::ThreadResumption,
                move |store: &mut StoreContext<'_, T>| store.run_queued_resumption(thread, number),
            )
            .for_task(task);
            if let Some(instance) = instance {
                item = item.in_instance(instance);
            }
            if yielded {
                self.scheduler_mut().push_low_priority(item);
            } else {
                self.scheduler_mut().push_high_priority(item);
            }
        }
        Ok(queued)
    }

    /// Put the resumption of the parked `thread` in the switch slot,
    /// so that the frame that runs the slot resumes the thread before
    /// anything else. Answers `false`, and changes nothing, when the
    /// thread is not suspended in the provider or a plan it left holds
    /// it. A resumption a turn queued for the thread already is spent
    /// by then, as one a switch overtook is. Workspace-internal.
    fn switch_to_parked_thread(&mut self, thread: ThreadId) -> Result<bool> {
        let Some((task, number)) = self.scheduler().resumption_of(thread) else {
            return Ok(false);
        };
        let instance = self
            .lock_tables()?
            .tasks
            .task(task)
            .and_then(|record| record.instance);
        let mut item = Item::new(
            ItemKind::ThreadResumption,
            move |store: &mut StoreContext<'_, T>| store.run_queued_resumption(thread, number),
        )
        .for_task(task);
        if let Some(instance) = instance {
            item = item.in_instance(instance);
        }
        self.scheduler_mut().switch_to(item);
        Ok(true)
    }

    /// Give an export's task a channel to resolve through and hand
    /// the caller its half.
    ///
    /// A host call into an asynchronous export takes this: the task
    /// outlives the call, so `task.return` sends the result through
    /// the channel and the call's driver takes it out, rather than
    /// the call reading the record of a task that may be gone by
    /// then. Workspace-internal.
    fn attach_result_channel(&self, task: TaskId) -> Result<ResultChannel> {
        self.lock_tables()?
            .tasks
            .attach_result_channel(task)
            .ok_or_else(|| Error::internal("an export's task is not in the store"))
    }

    /// Whether an export's task has resolved: the reference's
    /// `state == RESOLVED`. The exit of a callback task's implicit
    /// thread reads it, because a thread that exits without a result
    /// is the no-result trap. Workspace-internal.
    fn export_task_resolved(&self, task: TaskId) -> Result<bool> {
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
    fn instance_is_held(&self, instance: InstanceId) -> Result<bool> {
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
    fn release_exclusive_thread(&mut self, task: TaskId) -> Result<()> {
        let tables = self.tables_handle();
        let mut guard = Self::lock(&tables)?;
        self.scheduler()
            .release_exclusive_thread(&mut guard.tasks, task);
        Ok(())
    }

    /// Give `instance` to an export task's implicit thread, which a
    /// callback item does before it runs core code.
    /// Workspace-internal.
    fn take_exclusive_thread(&mut self, task: TaskId, instance: InstanceId) -> Result<()> {
        let tables = self.tables_handle();
        let mut guard = Self::lock(&tables)?;
        self.scheduler()
            .take_exclusive_thread(&mut guard.tasks, task, instance);
        Ok(())
    }

    /// Deliver the wait a callback task's status word asked for.
    ///
    /// The set is looked up in `table`, the instance's own handle
    /// table, the task's exclusive hold on the instance is released,
    /// and the item goes where
    /// [`park_callback_on_set`](Self::park_callback_on_set) sends it.
    /// Workspace-internal.
    fn wait_callback_on_set(
        &mut self,
        task: TaskId,
        instance: InstanceId,
        table: TableId,
        set_index: u32,
        slot: EventSlot,
        item: Item<T>,
    ) -> Result<()> {
        let set = {
            let tables = self.tables_handle();
            let mut guard = Self::lock(&tables)?;
            let set = Self::waitable_set_at(&guard, table, set_index)?;
            self.scheduler()
                .release_exclusive_thread(&mut guard.tasks, task);
            set
        };
        self.park_callback_on_set(task, instance, set, slot, item)
    }

    /// Queue or hold the callback item of `task`, whose callback
    /// waits on `set`.
    ///
    /// A set that already holds an event, or a task a cancellation
    /// request waits for, queues `item` at once, with the set in
    /// `slot`: the item takes the request or the set's event as it
    /// runs, the request first. The set counts the task's implicit
    /// thread as a waiter until then. Otherwise the task's implicit thread
    /// is parked on the set and the item waits with it, until a later
    /// turn finds the set filled or `subtask.cancel` wakes it.
    /// Workspace-internal.
    fn park_callback_on_set(
        &mut self,
        task: TaskId,
        instance: InstanceId,
        set: WaitableSetId,
        slot: EventSlot,
        item: Item<T>,
    ) -> Result<()> {
        let tables = self.tables_handle();
        let mut guard = Self::lock(&tables)?;
        let thread = guard
            .tasks
            .task(task)
            .map(|record| record.implicit_thread)
            .ok_or_else(|| Error::internal("an export's task is not in the store"))?;
        let ready =
            guard.tasks.has_pending_cancel(task) || guard.tasks.set_has_pending_event(set)?;
        if ready {
            guard.tasks.queue_wait(set, thread)?;
            slot.fill_from(set);
            self.scheduler_mut().push_high_priority(item);
        } else {
            guard.tasks.begin_wait(set, thread)?;
            self.scheduler_mut()
                .hold_for_event(instance, thread, set, slot, item);
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
    fn enter_export_task(&self, task: TaskId) -> Result<()> {
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
    fn hold_may_not_suspend(&self, task: TaskId) -> Result<()> {
        self.lock_tables()?
            .tasks
            .hold_may_not_suspend(task)
            .ok_or_else(|| Error::internal("an export's task is not in the store"))
    }

    /// Mark an export's task started: its thread is about to run.
    /// Workspace-internal.
    fn start_export_task(&self, task: TaskId) -> Result<()> {
        self.lock_tables()?.tasks.start_task(task);
        Ok(())
    }

    /// Resolve an export's task with the result it returned, which
    /// the caller on the stack takes as the call returns. Every
    /// handle lent for the call comes back with the resolution.
    /// Workspace-internal.
    fn resolve_export_task(&self, task: TaskId, result: Option<Val>) -> Result<()> {
        self.lock_tables()?.resolve_task(task, result)?;
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
    ///
    /// Whatever the task still has queued goes with the record, for
    /// the same reason and in the same breath: an item that named
    /// the task is work the task will never do, and the scheduler's
    /// own documentation states what dropping it gives back.
    ///
    /// The sweep is the second half of the exit, so it runs only
    /// when the first half ended the task: an exit that names a task
    /// whose scope is not on the stack ends nothing, and the items
    /// of a task that is still to run are its pending work, not a
    /// dead task's leavings. The release of the implicit thread
    /// above needs no such guard, because it is keyed on the thread
    /// — it gives back only what this task's own thread holds, and a
    /// task that is still parked holds nothing.
    ///
    /// A task that holds an explicit thread that has not ended does
    /// not end here. Only its implicit thread does: its scope is
    /// popped, and the task goes on with its other threads until the
    /// last of them ends, as
    /// [`end_last_thread`](Self::end_last_thread) states. The borrow
    /// check is the one exception. The reference makes it as the
    /// result is returned, which for a synchronous lift is the moment
    /// its implicit thread returns, so a task that still owes a
    /// borrow here ends here, with all its threads, and the count is
    /// its failure.
    fn exit_export_task(&mut self, task: TaskId) -> Result<core::result::Result<(), u32>> {
        let tables = self.tables_handle();
        let mut guard = Self::lock(&tables)?;
        self.scheduler_mut()
            .exit_implicit_thread(&mut guard.tasks, task);
        let goes_on = guard.tasks.scopes().contains(&Scope::Task(task))
            && guard
                .tasks
                .task(task)
                .is_some_and(|record| record.num_borrows == 0)
            && guard.tasks.has_explicit_threads(task);
        if goes_on {
            guard.leave_task_scope(task);
            guard.leave_implicit_thread(task);
            return Ok(Ok(()));
        }
        let end = guard.exit_task(task);
        let discarded = if end.ended() {
            self.scheduler_mut()
                .discard_task_items(&mut guard.tasks, task)
        } else {
            Vec::new()
        };
        drop(guard);
        self.release_discarded_threads(discarded);
        Ok(end.borrows())
    }

    /// End the implicit thread of an export's task whose scope is
    /// already off the stack, without ending the task, when the task
    /// holds an explicit thread that has not ended. Answers whether it
    /// did; a task with no such thread is untouched, and its caller
    /// ends it as a whole. Workspace-internal.
    ///
    /// This is the reference's `exit_implicit_thread` for a task with
    /// more than one thread. The instance the implicit thread held
    /// exclusively goes back, and the thread leaves its instance's
    /// table and its task. The task's record stays, with everything
    /// it has queued and every thread it holds, because a task ends
    /// only when its last thread does: an explicit thread that runs
    /// later may still call `task.return`.
    /// [`end_last_thread`](Self::end_last_thread) is the rest of the
    /// end.
    fn leave_implicit_thread(&mut self, task: TaskId) -> Result<bool> {
        let tables = self.tables_handle();
        let mut guard = Self::lock(&tables)?;
        if !guard.tasks.has_explicit_threads(task) {
            return Ok(false);
        }
        self.scheduler_mut()
            .exit_implicit_thread(&mut guard.tasks, task);
        Ok(guard.leave_implicit_thread(task))
    }

    /// End `task` when the explicit thread of it that just ended was
    /// its last, after its implicit thread exited. This is the
    /// reference's `unregister_thread` for the last thread of a task:
    /// the task ends, and a task that has not resolved fails with the
    /// no-result cause, as a borrow the guest did not drop fails one
    /// that did. A task that still holds a thread, or whose implicit
    /// thread has not exited, is untouched. Workspace-internal.
    ///
    /// The end is the one a task's implicit thread takes when it is
    /// the last: the record leaves the store, and whatever the task
    /// still has queued goes with it. The failure is the failure of
    /// the task, and goes where a trap of its thread would: it poisons
    /// the store and goes back to the caller, which ends the turn and
    /// reaches the driver that is polling. The
    /// caller's record of a call between two components goes the way
    /// a trap in the callee sends it.
    fn end_last_thread(&mut self, task: TaskId) -> Result<()> {
        let found = {
            let guard = self.lock_tables()?;
            if !guard.tasks.outlived_its_threads(task) {
                return Ok(());
            }
            guard
                .tasks
                .task(task)
                .map(|record| (record.state == TaskState::Resolved, record.subtask))
        };
        let Some((resolved, subtask)) = found else {
            return Ok(());
        };
        let borrows = self.end_export_task(task)?;
        let error = match (resolved, borrows) {
            (false, _) => Error::Task(TaskCause::NoResult),
            (true, Ok(())) => return Ok(()),
            (true, Err(count)) => Error::from(AbiError {
                position: AbiPosition::Result,
                valtype: None,
                cause: AbiCause::OutstandingBorrows {
                    count: count as usize,
                },
            }),
        };
        // The failure is a trap of the task, which poisons the store
        // and ends the turn.
        self.poison();
        if let Some(subtask) = subtask {
            release_subtask(self, subtask, None);
        }
        Err(error)
    }

    /// Pop the scope of an export's task without ending the task, as
    /// the callback loop of an asynchronous export does when core
    /// code returns: the record stays in the store, because the
    /// status word decides what the task does next.
    /// Workspace-internal.
    fn leave_export_task(&self, task: TaskId) -> Result<()> {
        self.lock_tables()?.leave_task_scope(task);
        Ok(())
    }

    /// End an export's task whose scope is already popped, which is
    /// the reference's `exit_implicit_thread` for a callback task:
    /// the instance the task held exclusively goes back and its
    /// record leaves the store, exactly as a synchronous task's does
    /// when its call returns, and whatever the task still has queued
    /// goes with the record under the rule
    /// [`exit_export_task`](Self::exit_export_task) states. This is
    /// the end a callback task parked between events reaches when
    /// the call that started it fails, so it is the end that really
    /// has items to give up. This end has no scope to consult, so it
    /// always ends the task and the sweep always runs. The inner
    /// `Err` carries the count of borrows the guest did not drop.
    /// Workspace-internal.
    fn end_export_task(&mut self, task: TaskId) -> Result<core::result::Result<(), u32>> {
        let tables = self.tables_handle();
        let mut guard = Self::lock(&tables)?;
        self.scheduler()
            .exit_implicit_thread(&mut guard.tasks, task);
        let end = guard.end_task(task);
        let discarded = if end.ended() {
            self.scheduler_mut()
                .discard_task_items(&mut guard.tasks, task)
        } else {
            Vec::new()
        };
        drop(guard);
        self.release_discarded_threads(discarded);
        Ok(end.borrows())
    }

    /// Pop the export's task on its failure path, with no borrow
    /// check. Every scope the failure left above the task — the task
    /// of a callee that trapped, the subtask of a host call that
    /// failed — is popped with it, and the lends of each are given
    /// back. The task's implicit thread ends first, for the reason
    /// [`exit_export_task`](Self::exit_export_task) gives: a call
    /// that failed gives the instance back exactly as one that
    /// returned does, and gives up what the task still has queued
    /// with it — but only when the task ended, under the rule that
    /// exit states. A failure that names a task whose scope is not
    /// on the stack ends nothing: the record stays, so the task is
    /// still to run and the items that name it are still its own.
    /// Workspace-internal.
    fn abandon_export_task(&mut self, task: TaskId) -> Result<()> {
        let tables = self.tables_handle();
        let mut guard = Self::lock(&tables)?;
        self.scheduler_mut()
            .exit_implicit_thread(&mut guard.tasks, task);
        let discarded = if guard.abandon_task(task) {
            self.scheduler_mut()
                .discard_task_items(&mut guard.tasks, task)
        } else {
            Vec::new()
        };
        drop(guard);
        self.release_discarded_threads(discarded);
        Ok(())
    }

    /// Give back what the built-ins of the threads a task's end
    /// discarded were holding, now that the tables are unlocked.
    ///
    /// Each thread was suspended in the provider inside a blocking
    /// built-in whose first part had run: it had raised a set's tally
    /// of waiters, marked an end as waited on synchronously, parked a
    /// host task for a synchronous lower, or started a callee. The
    /// thread will never resume, so the built-in's finish part runs
    /// here with a failed wait, which gives all of that back, as it
    /// does for a wait that fails where the thread stands. The trap it
    /// answers reaches nobody: the task it would end has ended.
    ///
    /// The finish runs with the thread's scopes back on the stack, as
    /// they were when it suspended, because some of what it gives back
    /// is on them: a synchronous lower's subtask gives back the
    /// handles the guest lent for the call only while its scope is on
    /// the stack. Whatever the thread left there goes once the finish
    /// has run, as a trap's unwind would take it. The thread's own
    /// finish, the part its starter left to run once its entry
    /// returned, does not run: it belongs to the task that ended.
    fn release_discarded_threads(
        &mut self,
        discarded: Vec<(ThreadId, ParkedThread<T>, Option<PendingBlock<T>>)>,
    ) {
        for (thread, mut parked, block) in discarded {
            let Ok(base) = self.lock_tables().map(|mut guard| {
                let base = guard.tasks.scopes().len();
                let scopes = core::mem::take(&mut parked.scopes);
                let running = core::mem::take(&mut parked.running);
                guard.tasks.restore_scopes(scopes, running);
                base
            }) else {
                continue;
            };
            if let Some(block) = block {
                if let Ok(mut guard) = self.lock_tables() {
                    guard.tasks.stop_waiting(thread, block.previous);
                }
                let _ = block.step.finish(
                    self,
                    Err(Error::internal(
                        "the task of a thread suspended in a blocking built-in ended",
                    )),
                );
            }
            if let Ok(mut guard) = self.lock_tables() {
                guard.tasks.cut_scopes(base);
            }
        }
    }

    /// Run `body` with an accessor to this store, driving the
    /// store's scheduler until the future `body` returns completes,
    /// and return what that future resolved to.
    ///
    /// This is the body of [`Store::run_concurrent`], which is the
    /// entry a host calls. It takes the context by value because it
    /// owns the store for as long as its own future lives: it runs
    /// its turns against that store directly, and lends it to the
    /// closure only for the length of one poll.
    ///
    /// The accessor it hands `body` is a token. It carries the
    /// store's identity and borrows nothing, so `body`'s future can
    /// hold one across its awaits. What it reaches is the thread's
    /// slot, which this entry fills with the store around each poll
    /// of `body`'s future and empties again before that poll
    /// returns — so a reach made from the closure's future outside
    /// a poll, or from anywhere else, fails with the
    /// store-not-in-poll cause.
    ///
    /// This is a driver: one poll of the returned future is a turn,
    /// and guest code runs only inside a turn.
    ///
    /// Entering it while another driver of the same store is inside
    /// a turn fails with the recursive-driver cause. Dropping the
    /// returned future cancels nothing: whatever the driver queued
    /// stays in the store and runs in the next turn of any driver,
    /// unless a trap poisons the store first and discards it.
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
    async fn run_concurrent<R, F>(self, body: F) -> Result<R>
    where
        F: AsyncFnOnce(&Accessor<T>) -> R,
    {
        // The refusal happens before the accessor exists, so a
        // refused entry leaves the store untouched.
        if self.turn_in_flight() {
            return Err(Error::Scheduler(SchedulerCause::RecursiveDriver));
        }

        let mut store = self;
        let accessor = Accessor::new(store.id());
        let mut future = core::pin::pin!(body(&accessor));
        let mut yield_wake: Option<YieldWake> = None;

        loop {
            let step = core::future::poll_fn(|context| {
                let waker = context.waker();

                // A turn that ended in a yield returns control to the
                // host executor before the item that yielded runs.
                if let Some(wake) = &yield_wake {
                    if !wake.landed() {
                        wake.rewake(waker);
                        return Poll::Pending;
                    }
                    yield_wake = None;
                }

                loop {
                    // The closure waits while a turn is stopped for a thread
                    // that runs on a microtask: the turn is not over.
                    if !store.deferred_busy()
                        && let Poll::Ready(value) = poll_lent(&mut store, future.as_mut(), context)
                    {
                        return Poll::Ready(Some(Ok(value)));
                    }
                    let outcome = match store.turn(waker) {
                        Ok(outcome) => outcome,
                        Err(error) => return Poll::Ready(Some(Err(error))),
                    };
                    match outcome {
                        Outcome::Progress => continue,
                        Outcome::Yield => {
                            yield_wake = Some(YieldWake::after_yield(waker));
                            return Poll::Pending;
                        }
                        // The turn left the store a flight, which the
                        // entry awaits before its next turn.
                        Outcome::Resuming => return Poll::Ready(None),
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
                            if let Poll::Ready(value) =
                                poll_lent(&mut store, future.as_mut(), context)
                            {
                                return Poll::Ready(Some(Ok(value)));
                            }
                            // That poll can have queued an item through
                            // the accessor, and only a turn runs an
                            // item. The re-check asks for a ready item
                            // rather than for pending work of any kind:
                            // a host task is pending under this outcome
                            // by definition, and it is not what the
                            // entry parks on — a host task that never
                            // returns would otherwise strand the item.
                            if store.has_ready_item() {
                                continue;
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
                            if let Poll::Ready(value) =
                                poll_lent(&mut store, future.as_mut(), context)
                            {
                                return Poll::Ready(Some(Ok(value)));
                            }
                            // That poll can have left work in the store:
                            // the closure reaches the store through its
                            // accessor, and what it queues there or
                            // starts there is work only a turn runs.
                            // Parking on it would wait for a wake that
                            // the parked work is what produces.
                            if store.has_pending_work() {
                                continue;
                            }
                            return Poll::Pending;
                        }
                    }
                }
            })
            .await;
            match step {
                Some(done) => return done,
                None => store.fly().await,
            }
        }
    }
}

/// Poll the `run_concurrent` closure's future with `store` in this
/// thread's slot, so that the accessor the closure holds reaches
/// the store for the length of the poll and no longer.
///
/// The store goes back out of the slot before the poll returns,
/// whether the future returned or unwound, which is what makes the
/// entry's own turns — run against the store it owns, between these
/// polls — the only other thing that reaches it.
fn poll_lent<T: 'static, F: Future>(
    store: &mut StoreContext<'_, T>,
    future: Pin<&mut F>,
    context: &mut Context<'_>,
) -> Poll<F::Output> {
    let _poll = PollScope::enter(store, context.waker());
    future.poll(context)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    use wcmp_macros::component;

    use crate::component::Component;
    use crate::concurrency::{
        Driver, Event, HostTask, ItemKind, Readiness, Scope, ThreadId, WaitableId,
    };
    use crate::engine::Engine;
    use crate::error::SchedulerCause;
    use crate::internal::ResourceTypeIdInternal;
    use crate::linker::{HostCall, Linker};
    use crate::resource::HandleKind;
    use crate::store::Store;
    use crate::types::ValueType;

    use super::*;
    use crate::store::StoreInternalExt;

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
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
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
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
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
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
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
                            let subtask = store
                                .lock_tables()
                                .expect("tables")
                                .tasks
                                .insert_subtask()
                                .expect("room under the record cap");
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
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");

        // A host task that never returns, and an item that completes
        // the closure. A turn runs the items that are ready before it
        // polls the host tasks, so the closure is done by the time the
        // turn reports that a host task is still pending — and the
        // entry must not park on a host task it does not wait for.
        let subtask = store
            .internal()
            .lock_tables()
            .expect("tables")
            .tasks
            .insert_subtask()
            .expect("room under the record cap");
        store
            .internal()
            .scheduler_mut()
            .push_host_task(HostTask::from_future(
                subtask,
                |_store: &mut StoreContext<'_, ()>, _outcome: Result<Vec<Val>>| Ok(()),
                core::future::pending::<Result<Vec<Val>>>(),
            ));
        let ran = Arc::new(AtomicUsize::new(0));
        let counted = ran.clone();
        store
            .internal()
            .scheduler_mut()
            .push_high_priority(Item::new(
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
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");

        // A host task that never returns, so every turn reports one
        // pending, and nothing outside the store ever wakes the
        // entry. The item the closure queues is the only thing that
        // resolves it, and only a turn runs an item — so an entry
        // that parked here would wait for ever on work it holds.
        let subtask = store
            .internal()
            .lock_tables()
            .expect("tables")
            .tasks
            .insert_subtask()
            .expect("room under the record cap");
        store
            .internal()
            .scheduler_mut()
            .push_host_task(HostTask::from_future(
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
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");
        store
            .internal()
            .scheduler_mut()
            .push_high_priority(Item::new(
                ItemKind::TaskStart,
                |_store: &mut StoreContext<'_, ()>| -> Result<()> { panic!("the item panicked") },
            ));

        let unwound = unwind(|| store.internal().turn(Waker::noop()));

        assert!(unwound.is_err(), "the item's panic unwound the turn");
        assert!(
            !store.internal().turn_in_flight(),
            "the turn the panic unwound out of is over"
        );
        let mut driver = Box::pin(Driver::run(store.internal().context(), |_store, _waker| {
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
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");
        store
            .internal()
            .scheduler_mut()
            .push_high_priority(Item::new(
                ItemKind::TaskStart,
                |store: &mut StoreContext<'_, ()>| -> Result<()> {
                    let _tables = store.lock_tables().expect("tables");
                    panic!("the item panicked with the tables locked")
                },
            ));

        let unwound = unwind(|| store.internal().turn(Waker::noop()));

        assert!(unwound.is_err(), "the item's panic unwound the turn");
        assert!(
            store.internal().lock_tables().is_ok(),
            "the turn's guard took the tables back from the poison the panic \
             left, so the store is not refusing every later reader"
        );
        let mut driver = Box::pin(Driver::run(store.internal().context(), |_store, _waker| {
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
        let tables = Arc::clone(store.internal_ref().tables());
        let poisoned = unwind(move || {
            let _tables = tables.lock().expect("tables");
            panic!("the host panicked with the tables locked")
        });
        assert!(poisoned.is_err(), "the panic unwound");
        assert!(
            store.internal_ref().tables().is_poisoned(),
            "and poisoned the lock"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn it_enters_a_turn_on_tables_a_panic_outside_the_store_poisoned() {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");

        // A panic taken with the lock held and no turn in flight, so
        // what meets the poison is the guard on its way in rather
        // than on its way out.
        poison_the_tables(&store);

        let outcome = store.internal().turn(Waker::noop());

        assert!(
            outcome.is_ok(),
            "the turn's guard took the tables back from the poison on its way in"
        );
        assert!(
            !store.internal().tables().is_poisoned(),
            "and cleared it, so every later reader of the store reaches them too"
        );
    }

    // Native only, for the reason the tests above are: the browser
    // aborts on a panic, so no panic ever reaches a lock there.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn it_enters_a_driver_on_tables_a_panic_outside_a_turn_poisoned() {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");

        // Both driver entries ask whether a turn is running before
        // either of them has a guard of its own, so the poison is
        // here before anything that could clear it.
        poison_the_tables(&store);

        let mut driver = Box::pin(Driver::run(store.internal().context(), |_store, _waker| {
            Some(Ok(()))
        }));
        assert!(
            matches!(poll_once(&mut driver, Waker::noop()), Poll::Ready(Ok(()))),
            "the driver read the store's turn state past the poison, so a \
             panic outside any turn does not refuse it"
        );
        drop(driver);
        assert!(
            store.internal().tables().is_poisoned(),
            "and the read left the poison where it found it: this driver's \
             condition was met before it ever entered a turn, and entering a \
             turn is where the recovery happens"
        );

        assert!(
            store.internal().turn(Waker::noop()).is_ok(),
            "the turn this driver never needed takes the tables back"
        );
        assert!(
            !store.internal().tables().is_poisoned(),
            "and clears the poison"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn it_enters_the_concurrent_entry_on_tables_a_panic_outside_a_turn_poisoned() {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
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
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");
        let outside = Outside::default();
        let signal = outside.clone();
        let item_runs = Arc::new(AtomicUsize::new(0));
        let counted = item_runs.clone();
        store
            .internal()
            .scheduler_mut()
            .push_low_priority(Item::new(
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
            let mut abandoned = Box::pin(Driver::run(store.internal().context(), never));
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
        let subtask = store
            .internal_ref()
            .lock_tables()
            .expect("tables")
            .tasks
            .push_subtask()
            .expect("room under the record cap");
        (store, TableId::fresh(), subtask)
    }

    #[wcmp_macros::test]
    async fn it_lowers_the_result_at_once_when_the_first_poll_resolves_the_future() {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let (mut store, table, subtask) = host_call(&engine);
        let slot: Lowered = Arc::new(Mutex::new(None));

        let status = store
            .internal()
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
            store.internal().scheduler().host_task_count(),
            0,
            "nothing joined the store's host tasks"
        );
        let guard = store.internal().lock_tables().expect("tables");
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
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let (mut store, table, subtask) = host_call(&engine);
        let slot: Lowered = Arc::new(Mutex::new(None));
        let outside = Outside::default();
        let awaited = outside.clone();

        let status = store
            .internal()
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
            let guard = store.internal().lock_tables().expect("tables");
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
            store.internal().scheduler().host_task_count(),
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
            store.internal().turn(Waker::noop()).expect("a turn"),
            Outcome::Progress,
            "the completed host task left an item ready to run"
        );
        store.internal().turn(Waker::noop()).expect("a turn");

        assert_eq!(
            lowered(&slot),
            vec![Val::U32(9)],
            "the result crossed in the turn that ran the lowering"
        );
        let mut guard = store.internal().lock_tables().expect("tables");
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
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
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
            .internal()
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
            let mut driver = Box::pin(Driver::run(store.internal().context(), never));
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

    /// A host task's future that never resolves on its own: each poll
    /// writes its name to a log and keeps the waker it was handed,
    /// for the test to wake when it chooses.
    struct Parked {
        name: &'static str,
        polls: Log,
        waker: Arc<Mutex<Option<Waker>>>,
    }

    impl Future for Parked {
        type Output = Result<Vec<Val>>;

        fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
            self.polls.lock().expect("log").push(self.name);
            *self.waker.lock().expect("waker") = Some(context.waker().clone());
            Poll::Pending
        }
    }

    /// Give `store` one parked host task per name, in order, and hand
    /// back the slot each one keeps its latest waker in.
    fn parked_host_tasks(
        store: &mut Store<()>,
        names: &[&'static str],
        polls: &Log,
    ) -> Vec<Arc<Mutex<Option<Waker>>>> {
        let slot: Lowered = Arc::new(Mutex::new(None));
        names
            .iter()
            .map(|name| {
                let subtask = store
                    .internal_ref()
                    .lock_tables()
                    .expect("tables")
                    .tasks
                    .insert_subtask()
                    .expect("room under the record cap");
                let waker = Arc::new(Mutex::new(None));
                store
                    .internal()
                    .scheduler_mut()
                    .push_host_task(HostTask::from_future(
                        subtask,
                        recording(&slot),
                        Parked {
                            name,
                            polls: polls.clone(),
                            waker: waker.clone(),
                        },
                    ));
                waker
            })
            .collect()
    }

    /// Wake the waker a parked host task kept from its last poll.
    fn wake(kept: &Arc<Mutex<Option<Waker>>>) {
        kept.lock()
            .expect("waker")
            .as_ref()
            .expect("the task was polled")
            .wake_by_ref();
    }

    #[wcmp_macros::test]
    fn it_polls_only_the_host_tasks_woken_since_the_previous_turn() {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");
        let polls = log();
        let counted = Arc::new(CountingWaker::default());
        let driver_waker = Waker::from(counted.clone());
        let wakers = parked_host_tasks(
            &mut store,
            &["parked 1", "parked 2", "parked 3", "parked 4", "woken"],
            &polls,
        );

        let outcome = store.internal().turn(&driver_waker).expect("a turn");
        assert_eq!(outcome, Outcome::Waiting);
        assert_eq!(
            entries(&polls),
            vec!["parked 1", "parked 2", "parked 3", "parked 4", "woken"],
            "every task that joined counts as woken, so the first turn polled them all"
        );

        polls.lock().expect("log").clear();
        let outcome = store.internal().turn(&driver_waker).expect("a turn");
        assert_eq!(outcome, Outcome::Waiting);
        assert!(
            entries(&polls).is_empty(),
            "nothing was woken, so the turn polled nothing"
        );

        wake(&wakers[4]);
        assert_eq!(
            counted.count(),
            1,
            "the task's wake reached the driver's waker"
        );
        let outcome = store.internal().turn(&driver_waker).expect("a turn");
        assert_eq!(outcome, Outcome::Waiting);
        assert_eq!(
            entries(&polls),
            vec!["woken"],
            "the turn polled the one task that was woken and none of the others"
        );
        assert_eq!(store.internal().scheduler().host_task_count(), 5);
    }

    #[wcmp_macros::test]
    fn it_polls_the_host_tasks_woken_in_one_turn_in_the_order_they_were_woken() {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");
        let polls = log();
        let wakers = parked_host_tasks(&mut store, &["a", "b", "c", "d"], &polls);
        store.internal().turn(Waker::noop()).expect("a turn");
        polls.lock().expect("log").clear();

        wake(&wakers[2]);
        wake(&wakers[0]);
        wake(&wakers[2]);
        wake(&wakers[3]);
        store.internal().turn(Waker::noop()).expect("a turn");

        assert_eq!(
            entries(&polls),
            vec!["c", "a", "d"],
            "the woken tasks were polled in the order of their first wakes, once \
             each, and the task nothing woke was not polled"
        );
    }

    // A browser build has no second thread to send the wake from, so
    // the test is native only.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn it_polls_a_host_task_woken_from_another_thread_in_the_next_turn() {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");
        let polls = log();
        let counted = Arc::new(CountingWaker::default());
        let driver_waker = Waker::from(counted.clone());
        let wakers = parked_host_tasks(&mut store, &["idle", "woken elsewhere"], &polls);
        store.internal().turn(&driver_waker).expect("a turn");
        polls.lock().expect("log").clear();

        let kept = wakers[1].clone();
        std::thread::spawn(move || wake(&kept))
            .join()
            .expect("the waking thread");
        assert_eq!(
            counted.count(),
            1,
            "the wake from the other thread reached the driver's waker"
        );

        let outcome = store.internal().turn(&driver_waker).expect("a turn");
        assert_eq!(outcome, Outcome::Waiting);
        assert_eq!(
            entries(&polls),
            vec!["woken elsewhere"],
            "the next turn polled the task the other thread woke, and only it"
        );
    }

    /// What the items of the entry-gate tests below recorded as they
    /// ran, in order.
    type Log = Arc<Mutex<Vec<&'static str>>>;

    /// A fresh, empty log.
    fn log() -> Log {
        Arc::new(Mutex::new(Vec::new()))
    }

    /// What the items have recorded so far.
    fn entries(log: &Log) -> Vec<&'static str> {
        log.lock().expect("log").clone()
    }

    /// An item that records that it ran.
    fn marker(log: &Log, name: &'static str) -> Item<()> {
        let log = log.clone();
        Item::new(
            ItemKind::TaskStart,
            move |_store: &mut StoreContext<'_, ()>| {
                log.lock().expect("log").push(name);
                Ok(())
            },
        )
    }

    /// An item that runs one export call the way the export path
    /// runs one: the task becomes the current scope, it resolves
    /// with no result, and then it exits. The exit is what ends the
    /// task's implicit thread, so the instance it held goes back and
    /// the next task waiting at the gate can take it.
    fn export_call(log: &Log, name: &'static str, task: TaskId) -> Item<()> {
        let log = log.clone();
        Item::new(
            ItemKind::TaskStart,
            move |store: &mut StoreContext<'_, ()>| {
                log.lock().expect("log").push(name);
                store.enter_export_task(task)?;
                store.resolve_export_task(task, None)?;
                store
                    .exit_export_task(task)?
                    .expect("the call dropped every borrow it took");
                Ok(())
            },
        )
    }

    /// Queue the start of a fresh task of `instance`, with the gate
    /// arguments a call into an export lifted `async` with a
    /// callback carries: the function type is `async`, so the gate
    /// applies, and the task needs the instance exclusively. This is
    /// what `Func::call_concurrent` queues for such an export.
    fn start_callback_task(
        store: &mut StoreContext<'_, ()>,
        instance: InstanceId,
        build: impl FnOnce(TaskId) -> Item<()>,
    ) -> TaskId {
        let task = store
            .tables()
            .lock()
            .expect("tables")
            .tasks
            .create_task(None, None, instance)
            .expect("room under the record cap");
        store
            .start_export_thread(task, instance, true, true, build(task))
            .expect("queue the task's start");
        task
    }

    /// Build the store state the two entry-gate tests share: an
    /// instance a callback task holds, one call into that instance
    /// already waiting at its gate, and a host task whose body
    /// queues a second call there and then gives the instance back.
    ///
    /// The body reaches the store through its accessor, which is the
    /// one thing a host task's body can do to the store, and the two
    /// things it does there are the two a host `async` function's
    /// body really does: the start is what `Func::call_concurrent`
    /// queues, and the release is the call a callback task's own
    /// loop makes between events. It is spelled here rather than run
    /// as a callback, because no guest code runs while a host task's
    /// body is polled: the ready queues are empty by then, and only
    /// the bodies can still change what the store holds.
    fn gate_the_host_tasks_open(store: &mut Store<()>, log: &Log) {
        let instance = store
            .internal()
            .lock_tables()
            .expect("tables")
            .tasks
            .insert_instance();
        let holder = store
            .internal()
            .lock_tables()
            .expect("tables")
            .tasks
            .create_task(None, None, instance)
            .expect("room under the record cap");
        store
            .internal()
            .context()
            .take_exclusive_thread(holder, instance)
            .expect("the callback task takes the instance");

        // An earlier call into the same instance, which the gate
        // holds because the callback task has the instance.
        let waiting = log.clone();
        start_callback_task(&mut store.internal().context(), instance, move |task| {
            export_call(&waiting, "early", task)
        });

        let subtask = store
            .internal()
            .lock_tables()
            .expect("tables")
            .tasks
            .push_subtask()
            .expect("room under the record cap");
        let accessor: Accessor<()> = Accessor::new(store.internal().id());
        let queued = log.clone();
        let mut reached = false;
        store
            .internal()
            .context()
            .push_host_task(HostTask::from_future(
                subtask,
                |_store: &mut StoreContext<'_, ()>, _outcome: Result<Vec<Val>>| Ok(()),
                core::future::poll_fn(move |_context| -> Poll<Result<Vec<Val>>> {
                    if !reached {
                        reached = true;
                        accessor
                            .with(|store: &mut StoreContext<'_, ()>| {
                                let recorded = queued.clone();
                                start_callback_task(store, instance, move |_task| {
                                    marker(&recorded, "queued")
                                });
                                store.release_exclusive_thread(holder)
                            })
                            .expect("reach the store")
                            .expect("give the instance back");
                    }
                    // The body never resolves, so nothing but the gate
                    // can carry the turn that polls it forward.
                    Poll::Pending
                }),
            ));
    }

    #[wcmp_macros::test]
    async fn it_queues_the_starts_its_host_tasks_released_behind_what_their_polls_queued() {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");
        let log = log();
        gate_the_host_tasks_open(&mut store, &log);

        // A second host task, whose body is ready. The turn queues
        // the lowering of what it produced as it polls it, and the
        // starts the gate releases afterwards queue behind that.
        let subtask = store
            .internal()
            .lock_tables()
            .expect("tables")
            .tasks
            .push_subtask()
            .expect("room under the record cap");
        let lowered = log.clone();
        store
            .internal()
            .context()
            .push_host_task(HostTask::from_future(
                subtask,
                move |_store: &mut StoreContext<'_, ()>, _outcome: Result<Vec<Val>>| {
                    lowered.lock().expect("log").push("lowered");
                    Ok(())
                },
                core::future::ready(Ok(vec![Val::U32(1)])),
            ));

        let polled = store
            .internal()
            .turn(Waker::noop())
            .expect("the turn that polls the bodies");

        assert_eq!(
            polled,
            Outcome::Progress,
            "the gate opened after the polls, so the turn has work to run \
             rather than a host task to wait on"
        );
        assert_eq!(
            store.internal().scheduler().waiting_at_gate(),
            1,
            "the gate let the start that was waiting at it through as the \
             turn ended; the one the body queued behind it waits for the \
             instance that start took"
        );
        assert!(
            entries(&log).is_empty(),
            "nothing has run yet: the lowering and the start the gate \
             released are queued for the turn that follows"
        );

        store
            .internal()
            .turn(Waker::noop())
            .expect("the turn that runs them");

        assert_eq!(
            entries(&log),
            vec!["lowered", "early"],
            "the lowering the poll queued ran first, and the start that was \
             waiting at the gate before the turn followed it"
        );

        // The first of the two starts held the instance while it
        // ran and gave it back as it ended, so the gate let the
        // second through only as that turn ended, and it is the
        // next turn that runs it.
        store
            .internal()
            .turn(Waker::noop())
            .expect("the turn that runs the second");

        assert_eq!(
            entries(&log),
            vec!["lowered", "early", "queued"],
            "the two starts ran in the order they arrived at the gate: the \
             one that was waiting there before the turn, then the one the \
             body queued"
        );
    }

    #[wcmp_macros::test]
    async fn it_runs_a_start_its_host_tasks_released_without_another_poll_of_the_driver() {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");
        let log = log();
        gate_the_host_tasks_open(&mut store, &log);

        // Nothing is ready and the one host task never resolves, so
        // the work the gate holds is the only thing the turn can
        // carry forward. A turn that reported `Waiting` here would
        // park the driver on a wake that running that work is what
        // produces.
        let watched = log.clone();
        let mut driver = Box::pin(Driver::run(
            store.internal().context(),
            move |_store, _waker| {
                watched
                    .lock()
                    .expect("log")
                    .contains(&"queued")
                    .then(|| Ok(()))
            },
        ));

        let outcome = poll_once(&mut driver, Waker::noop());

        assert!(
            matches!(outcome, Poll::Ready(Ok(()))),
            "the start the body queued past the gate ran inside the poll the \
             body was polled in, with no second poll of the driver"
        );
        assert_eq!(
            entries(&log),
            vec!["early", "queued"],
            "and it ran behind the start that was already waiting at the \
             gate, which took the instance first and gave it back as its \
             call ended"
        );
    }

    #[wcmp_macros::test]
    async fn it_fails_a_synchronous_lower_of_a_pending_future_with_the_stack_switch_cause() {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let (mut store, table, subtask) = host_call(&engine);
        let slot: Lowered = Arc::new(Mutex::new(None));

        // A synchronous lower whose future resolves at once needs
        // nothing of the suspend seam: the guest gets its result as
        // the call returns.
        store
            .internal()
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

        let subtask = store
            .internal()
            .lock_tables()
            .expect("tables")
            .tasks
            .push_subtask()
            .expect("room under the record cap");
        let error = store
            .internal()
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
            "a synchronous lower of a future that never resolves runs its nested \
             turns until the store is idle, and the store still holding that \
             future is what names the stack-switch cause; it failed with {error} \
             instead"
        );
        assert_eq!(
            store.internal().scheduler().host_task_count(),
            0,
            "the failed call left no host task in the store"
        );
        assert!(
            store
                .internal()
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
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let (mut store, table, subtask) = host_call(&engine);
        let slot: Lowered = Arc::new(Mutex::new(None));

        let error = store
            .internal()
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
                .internal()
                .lock_tables()
                .expect("tables")
                .tasks
                .subtask(subtask)
                .is_none(),
            "the failed call gave the subtask back"
        );
    }

    #[wcmp_macros::test]
    async fn it_ends_the_turn_that_polls_a_failed_host_task_and_poisons_the_store() {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let (mut store, table, subtask) = host_call(&engine);
        let slot: Lowered = Arc::new(Mutex::new(None));
        let outside = Outside::default();
        let awaited = outside.clone();

        let status = store
            .internal()
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

        // The failure is a trap of the guest task that made the call,
        // and it ends the turn that polled the body, with no item
        // queued for a later turn in between.
        let error = store
            .internal()
            .turn(Waker::noop())
            .expect_err("the failure ends the turn that polled the body");

        assert!(
            error.to_string().contains("the host call failed"),
            "the turn ends with what the body failed with, and it ended with \
             {error} instead"
        );
        assert!(
            store.internal().enter_guest().is_err(),
            "the failed host future poisoned the store"
        );
        assert!(
            slot.lock().expect("the lowering's slot").is_none(),
            "a call that never returned has nothing to lower"
        );
        let guard = store.internal().lock_tables().expect("tables");
        assert!(
            guard.tasks.subtask(subtask).is_none(),
            "the subtask did not resolve: its record left the store instead"
        );
        assert!(
            guard.entry(table, index).is_none(),
            "and the caller's entry for it went with the record"
        );
    }

    #[wcmp_macros::test]
    async fn it_ends_the_turn_when_a_failed_crossing_has_no_task_to_trap() {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let (mut store, table, subtask) = host_call(&engine);
        let outside = Outside::default();
        let awaited = outside.clone();

        let status = store
            .internal()
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
            store.internal().turn(Waker::noop()).expect("a turn"),
            Outcome::Progress,
            "the completed host task left its lowering ready to run"
        );

        let error = store
            .internal()
            .turn(Waker::noop())
            .expect_err("the crossing's failure has no task to trap, so it ends the turn");

        assert!(
            error.to_string().contains("the crossing failed"),
            "the turn ends with what the crossing failed with, and it ended \
             with {error} instead"
        );
        let guard = store.internal().lock_tables().expect("tables");
        assert!(
            guard.tasks.subtask(subtask).is_none(),
            "nothing reached the guest, so the subtask did not resolve: its \
             record left the store rather than staying started for ever"
        );
        assert!(
            guard.entry(table, index).is_none(),
            "and the caller's entry for it went with the record"
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

    /// The same shape lifted `canon lift async` with a callback. The
    /// export calls the host function, adds one to what it returns,
    /// hands that to `task.return`, and exits. A call into such an
    /// export is a task the reference allows to block, so a block in
    /// the host function's trampoline waits rather than failing with
    /// the cannot-block cause.
    const CALLBACK_CALLS_THE_HOST: &[u8] = component!(
        r#"
        (component
          (import "probe" (func $probe (param "x" u32) (result u32)))
          (core func $probe' (canon lower (func $probe)))
          (core func $task-return (canon task.return (result u32)))
          (core module $m
            (import "" "probe" (func $probe (param i32) (result i32)))
            (import "" "task.return" (func $task-return (param i32)))
            (func (export "run") (param i32) (result i32)
              (call $task-return
                (i32.add (call $probe (local.get 0)) (i32.const 1)))
              (i32.const 0))
            (func (export "run-callback") (param i32 i32 i32) (result i32) unreachable))
          (core instance $i (instantiate $m
            (with "" (instance
              (export "probe" (func $probe'))
              (export "task.return" (func $task-return))))))
          (func (export "run") async (param "x" u32) (result u32)
            (canon lift (core func $i "run") async
              (callback (core func $i "run-callback")))))
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

    #[wcmp_macros::test]
    async fn it_blocks_a_synchronous_lower_on_the_seams_condition_with_no_provider() {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let component = Component::new(&engine, CALLS_THE_HOST)
            .await
            .expect("component parses");
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");

        // What the host function's call of `start_host_task` came
        // out as, how many host tasks the store held afterwards, and
        // how many items the store ran while the call was blocked.
        let started: Arc<Mutex<Option<(String, usize, u64)>>> = Arc::new(Mutex::new(None));
        let recorded = started.clone();
        let slot: Lowered = Arc::new(Mutex::new(None));
        let filled = slot.clone();

        let mut linker: Linker<()> = Linker::new(&engine);
        linker
            .root()
            .func_wrap(
                "probe",
                move |mut call: HostCall<'_, ()>, (x,): (u32,)| -> Result<u32> {
                    let store = call.store();
                    let subtask = store
                        .lock_tables()?
                        .tasks
                        .push_subtask()
                        .expect("room under the record cap");
                    let lowering = filled.clone();
                    let before = store.scheduler().items_run();
                    let status = store
                        .start_host_task(
                            HostTask::from_future(
                                subtask,
                                move |_store: &mut StoreContext<'_, ()>,
                                      outcome: Result<Vec<Val>>| {
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
                        )
                        .map_or_else(
                            |error| error.to_string(),
                            |status| status.value().to_string(),
                        );
                    *recorded.lock().expect("record") = Some((
                        status,
                        store.scheduler().host_task_count(),
                        store.scheduler().items_run() - before,
                    ));
                    Ok(x)
                },
            )
            .expect("the registration");

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
            Some((CallStatus::returned().value().to_string(), 0, 0)),
            "the block took the seam's fallback, \
             which polled the body again at the first check of its condition \
             and found it ready, so the call returned with no host task left \
             in the store and no item run — the check comes before the first \
             nested turn, and a body ready there never reaches one"
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
    }

    #[wcmp_macros::test]
    async fn it_blocks_a_synchronous_lower_of_a_task_that_may_block_on_the_seams_condition() {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let component = Component::new(&engine, CALLBACK_CALLS_THE_HOST)
            .await
            .expect("component parses");
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");

        // The status word the call reported, and what the lowering
        // was handed.
        let reported: Arc<Mutex<Option<u32>>> = Arc::new(Mutex::new(None));
        let recorded = reported.clone();
        let slot: Lowered = Arc::new(Mutex::new(None));
        let filled = slot.clone();

        let mut linker: Linker<()> = Linker::new(&engine);
        linker
            .root()
            .func_wrap(
                "probe",
                move |mut call: HostCall<'_, ()>, (x,): (u32,)| -> Result<u32> {
                    let store = call.store();
                    let subtask = store
                        .lock_tables()?
                        .tasks
                        .push_subtask()
                        .expect("room under the record cap");
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
            )
            .expect("the registration");

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
            "the seam served the block until the body was ready, so the call \
             returned its result to the guest with no subtask behind it"
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
            "the guest's `task.return` carried what the host function \
             returned plus one"
        );
        assert_eq!(
            store.internal().scheduler().host_task_count(),
            0,
            "the task the call blocked on was parked in the store, and the \
             turn that completed it took it out again"
        );
    }

    /// A host task's body that is still running on its first two
    /// polls and completes on its third. It asks for the next poll
    /// every time it answers pending, as a future that waits on
    /// something outside the store does once that thing moves.
    struct ReadyOnThirdPoll {
        polls: Arc<AtomicUsize>,
        value: u32,
    }

    impl Future for ReadyOnThirdPoll {
        type Output = Result<Vec<Val>>;

        fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
            let this = self.get_mut();
            if this.polls.fetch_add(1, AtomicOrdering::Relaxed) + 1 >= 3 {
                return Poll::Ready(Ok(vec![Val::U32(this.value)]));
            }
            context.waker().wake_by_ref();
            Poll::Pending
        }
    }

    /// What the store held while a synchronous lower waited: how
    /// many host tasks, whether the call's own task was among them,
    /// and the readiness conditions of the waiting threads.
    type WhileWaiting = Arc<Mutex<Option<(usize, bool, Vec<Readiness>)>>>;

    #[wcmp_macros::test]
    async fn it_parks_the_future_of_a_synchronous_lower_among_the_stores_host_tasks() {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let component = Component::new(&engine, CALLBACK_CALLS_THE_HOST)
            .await
            .expect("component parses");
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");

        let seen: WhileWaiting = Arc::new(Mutex::new(None));
        let polls = Arc::new(AtomicUsize::new(0));
        let reported: Arc<Mutex<Option<u32>>> = Arc::new(Mutex::new(None));
        let slot: Lowered = Arc::new(Mutex::new(None));

        let mut linker: Linker<()> = Linker::new(&engine);
        {
            let (seen, polls, reported, filled) =
                (seen.clone(), polls.clone(), reported.clone(), slot.clone());
            linker
                .root()
                .func_wrap(
                    "probe",
                    move |mut call: HostCall<'_, ()>, (x,): (u32,)| -> Result<u32> {
                        let store = call.store();
                        let subtask = store
                            .lock_tables()?
                            .tasks
                            .push_subtask()
                            .expect("room under the record cap");

                        // Ready work of the store, which the block's
                        // nested turn runs while the call waits. It
                        // reads the store as it finds it then.
                        let recorded = seen.clone();
                        store.scheduler_mut().push_high_priority(Item::new(
                            ItemKind::TaskStart,
                            move |store: &mut StoreContext<'_, ()>| {
                                let held = store.scheduler().host_task_count();
                                let parked = store.scheduler().holds_host_task(subtask);
                                let guard = store.lock_tables()?;
                                let waiting = guard
                                    .tasks
                                    .waiting_threads()
                                    .iter()
                                    .filter_map(|thread| guard.tasks.thread(*thread))
                                    .filter_map(|record| record.readiness)
                                    .collect();
                                *recorded.lock().expect("record") = Some((held, parked, waiting));
                                Ok(())
                            },
                        ));

                        let lowering = filled.clone();
                        let status = store.start_host_task(
                            HostTask::from_future(
                                subtask,
                                move |_store: &mut StoreContext<'_, ()>,
                                      outcome: Result<Vec<Val>>| {
                                    *lowering.lock().expect("the lowering's slot") = Some(outcome);
                                    Ok(())
                                },
                                ReadyOnThirdPoll {
                                    polls: polls.clone(),
                                    value: x * 2,
                                },
                            ),
                            TableId::fresh(),
                            LowerKind::Sync,
                        )?;
                        *reported.lock().expect("record") = Some(status.value());
                        Ok(x)
                    },
                )
                .expect("the registration");
        }

        let instance = linker
            .instantiate(&mut store, &component)
            .await
            .expect("instantiate");
        let run = instance.get_func("run").expect("run export");
        let result = run
            .call(&mut store, &[Val::U32(20)])
            .await
            .expect("call run");

        let (held, parked, waiting) = seen
            .lock()
            .expect("record")
            .clone()
            .expect("the nested turn ran the item while the call waited");
        assert_eq!(
            held, 1,
            "while the call waited, the store held one host task"
        );
        assert!(
            parked,
            "and it was the call's own future, which the lower parked there"
        );
        assert_eq!(
            waiting.len(),
            1,
            "one thread waited, on the resolution of the call: {waiting:?}"
        );
        assert!(
            matches!(waiting[0], Readiness::Subtask { .. }),
            "the waiting thread's condition names the call's subtask: {waiting:?}"
        );
        assert_eq!(
            polls.load(AtomicOrdering::Relaxed),
            3,
            "the future was pending for two polls and ready on the third"
        );
        assert_eq!(
            *reported.lock().expect("record"),
            Some(CallStatus::returned().value()),
            "the call returned its result to the guest"
        );
        assert_eq!(lowered(&slot), vec![Val::U32(40)]);
        assert_eq!(result.first(), Some(&Val::U32(21)));
        assert_eq!(
            store.internal().scheduler().host_task_count(),
            0,
            "the poll that completed the future took it out of the store"
        );
    }

    /// Make a task of a fresh instance the current scope, and push
    /// the subtask of a call it makes, as a synchronous lower finds
    /// them. The instance may block. Answers the task's thread and
    /// the call's subtask.
    fn a_call_of_the_current_task(store: &StoreContext<'_, ()>) -> (ThreadId, SubtaskId) {
        let mut guard = store.lock_tables().expect("tables");
        let instance = guard.tasks.insert_instance();
        let task = guard
            .tasks
            .create_task(None, None, instance)
            .expect("room under the record cap");
        guard.tasks.push_task_scope(task);
        let thread = guard.tasks.current_thread().expect("the task's thread");
        (
            thread,
            guard
                .tasks
                .push_subtask()
                .expect("room under the record cap"),
        )
    }

    /// Lower the call `subtask` records synchronously, with a body
    /// that stays pending and never asks for a wake.
    fn lower_a_call_that_never_resolves(
        store: &mut StoreContext<'_, ()>,
        subtask: SubtaskId,
    ) -> Result<CallStatus> {
        store.start_host_task(
            HostTask::from_future(
                subtask,
                |_store: &mut StoreContext<'_, ()>, _outcome: Result<Vec<Val>>| Ok(()),
                core::future::pending::<Result<Vec<Val>>>(),
            ),
            TableId::fresh(),
            LowerKind::Sync,
        )
    }

    /// The condition `thread` waits on, and the threads the store
    /// lists as waiting.
    fn waits(store: &StoreContext<'_, ()>, thread: ThreadId) -> (Option<Readiness>, Vec<ThreadId>) {
        let guard = store.lock_tables().expect("tables");
        (
            guard
                .tasks
                .thread(thread)
                .and_then(|record| record.readiness),
            guard.tasks.waiting_threads().to_vec(),
        )
    }

    /// What a later block that nothing can serve fails with. The
    /// store is idle, so the cause says whether it still holds a
    /// host future that can resolve.
    fn a_later_failed_block(store: &mut StoreContext<'_, ()>) -> String {
        let outcome = store
            .run_in_turn(Waker::noop(), |store| {
                SuspendSeam::suspend(store, |_| false)
            })
            .expect("the outer turn runs");
        match outcome {
            Err(error) => error.to_string(),
            Ok(()) => "the later block returned".to_owned(),
        }
    }

    #[wcmp_macros::test]
    fn it_withdraws_the_parked_call_of_a_synchronous_lower_whose_wait_failed() {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut owner: Store<()> = Store::new(&engine, ()).expect("store");
        let mut store = owner.internal().context();
        let (thread, subtask) = a_call_of_the_current_task(&store);

        let lowered = store
            .run_in_turn(Waker::noop(), move |store| {
                lower_a_call_that_never_resolves(store, subtask)
            })
            .expect("the outer turn runs");

        assert_eq!(
            lowered
                .map(|status| status.value())
                .map_err(|error| error.to_string()),
            Err(Error::Scheduler(SchedulerCause::StackSwitchNeeded).to_string()),
            "the call's own future was parked and still pending when the \
             store went idle, so the wait failed with the stack-switch cause"
        );
        assert_eq!(
            store.scheduler().host_task_count(),
            0,
            "the lower withdrew the parked task when its wait failed"
        );
        assert!(!store.scheduler().is_parked_call(subtask));
        assert_eq!(
            waits(&store, thread),
            (None, Vec::new()),
            "and the thread's wait ended"
        );
        assert_eq!(
            a_later_failed_block(&mut store),
            Error::Scheduler(SchedulerCause::Deadlock).to_string(),
            "no host future is left pending, so a later block of an idle \
             store names the deadlock cause"
        );
    }

    /// Lower a call that never resolves, with an item queued that
    /// panics when the block's nested turn runs it, and catch the
    /// unwind. With `tables_locked`, the item panics while it holds
    /// the store's handle tables, which poisons their lock.
    #[cfg(not(target_arch = "wasm32"))]
    fn panic_inside_a_blocked_lower(
        store: &mut StoreContext<'_, ()>,
        subtask: SubtaskId,
        tables_locked: bool,
    ) {
        store.scheduler_mut().push_high_priority(Item::new(
            ItemKind::TaskStart,
            move |store: &mut StoreContext<'_, ()>| -> Result<()> {
                let _tables = tables_locked.then(|| store.lock_tables().expect("tables"));
                panic!("the item panicked")
            },
        ));

        let unwound = unwind(|| {
            store.run_in_turn(Waker::noop(), move |store| {
                lower_a_call_that_never_resolves(store, subtask)
            })
        });

        assert!(
            unwound.is_err(),
            "the item's panic unwound through the block"
        );
    }

    // The two tests below are native only: the browser aborts on a
    // panic instead of unwinding, so there is nothing to catch there
    // and nothing the block could be left holding.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn it_leaves_no_parked_call_and_no_waiting_thread_when_a_blocked_lower_panicked() {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut owner: Store<()> = Store::new(&engine, ()).expect("store");
        let mut store = owner.internal().context();
        let (thread, subtask) = a_call_of_the_current_task(&store);

        panic_inside_a_blocked_lower(&mut store, subtask, false);

        assert_eq!(
            store.scheduler().host_task_count(),
            0,
            "the lower withdrew the task it parked as the panic unwound \
             through it"
        );
        assert!(!store.scheduler().is_parked_call(subtask));
        assert_eq!(
            waits(&store, thread),
            (None, Vec::new()),
            "the thread's wait ended as the panic unwound through the seam"
        );
        assert_eq!(
            a_later_failed_block(&mut store),
            Error::Scheduler(SchedulerCause::Deadlock).to_string(),
            "the store is idle and no host future is pending, so a parked \
             task the panic left behind would have named the stack-switch \
             cause here"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn it_ends_the_wait_of_a_blocked_lower_whose_panic_poisoned_the_tables() {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut owner: Store<()> = Store::new(&engine, ()).expect("store");
        let mut store = owner.internal().context();
        let (thread, subtask) = a_call_of_the_current_task(&store);

        panic_inside_a_blocked_lower(&mut store, subtask, true);

        assert_eq!(
            waits(&store, thread),
            (None, Vec::new()),
            "the seam read past the poison to end the thread's wait, and \
             the turn took the tables back from it"
        );
        assert_eq!(store.scheduler().host_task_count(), 0);
        assert_eq!(
            a_later_failed_block(&mut store),
            Error::Scheduler(SchedulerCause::Deadlock).to_string()
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
        let guard = store.internal_ref().tables().lock().expect("handle tables");
        (
            guard.tasks.scopes().len(),
            guard.tasks.task_count(),
            guard.tasks.thread_count(),
        )
    }

    #[wcmp_macros::test]
    fn it_runs_a_host_resource_destructor_as_a_task_with_one_thread() {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");
        let tables = store.internal().tables_handle();
        let seen: Arc<Mutex<Option<SeenByDestructor>>> = Arc::new(Mutex::new(None));
        let recorded = seen.clone();

        let type_id = ResourceTypeId::fresh();
        store.internal().context().register_resource(
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
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");
        let tables = store.internal().tables_handle();
        let seen: Arc<Mutex<Option<SeenByDestructor>>> = Arc::new(Mutex::new(None));
        let recorded = seen.clone();

        let type_id = ResourceTypeId::fresh();
        store.internal().context().register_resource(
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
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");

        // One identity under two labels, which is what a host
        // resource registered against two interfaces is. The store
        // keeps the first of them and renders that one; it does not
        // carry the set.
        let type_id = ResourceTypeId::fresh();
        store.internal().context().register_resource(
            type_id,
            Some(ResourceType::new("first")),
            ResourceDestructor::Host(Arc::new(|_data: &mut (), _rep: u32| Ok(()))),
        );
        store
            .internal()
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
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");

        // A fallback is what the sweep of a linker's registrations
        // leaves behind for an identity no component brought in. A
        // component that names the same identity afterwards is the
        // better name, so it takes over; a second fallback after it
        // does not take it back.
        let type_id = ResourceTypeId::fresh();
        store
            .internal()
            .context()
            .fallback_resource_name(type_id, ResourceType::new("swept"));
        store
            .internal()
            .context()
            .name_resource(type_id, ResourceType::new("imported"));
        store
            .internal()
            .context()
            .fallback_resource_name(type_id, ResourceType::new("swept-again"));
        store.internal().context().register_resource(
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
