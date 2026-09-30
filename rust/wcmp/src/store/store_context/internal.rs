//! The workspace-internal face of [`StoreContext`].
//!
//! [`StoreContext`] is re-exported by `lib.rs`, so every `pub` item
//! on it is public API even when the type it hands back cannot be
//! named outside the crate: a method is reachable by method syntax
//! whatever its return type is. The entries a turn, an item, and a
//! trampoline need therefore live here instead, on a wrapper the
//! crate builds over a borrow of the context. The wrapper is never
//! re-exported, and no `pub` method on [`StoreContext`] returns one,
//! so safe host code cannot reach the scheduler, the handle tables,
//! or the runtime-layer store through the context it is handed.
//!
//! Each entry keeps the signature it had on the context. The
//! wrapper holds the borrow, so a borrow an entry returns is tied to
//! that borrow exactly as it was tied to `&mut self` before; the
//! entries take `self` by value for that reason.

use core::task::Waker;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::abi::signature::Signature;
use crate::concurrency::{
    Accessor, CallStatus, EntryFinish, EventSlot, HostTask, InstanceId, Item, LowerKind, Outcome,
    Plan, ResultChannel, Scheduler, StoreProvider, SubtaskId, TaskId, ThreadId, WaitableSetId,
};
use crate::error::{Result, SchedulerCause};
use crate::executor::ResourceDestructor;
use crate::executor::ir::CanonOptions;
use crate::resource::{HandleTables, ResourceHandle, ResourceTypeId, TableId};
use crate::runtime_layer::{
    Func as RuntimeFunc, StoreContextMut as RuntimeContextMut, Val as RuntimeVal,
};
use crate::types::ResourceType;
use crate::value::Val;

use super::super::resource_record::ResourceRecord;
use super::super::store_data::StoreData;
use super::super::store_id::StoreId;
use super::StoreContext;

/// The workspace-internal entries of a [`StoreContext`], over a
/// borrow of one.
///
/// `'b` is the borrow of the context the wrapper holds, and `'a` is
/// the borrow of the core store the context itself holds.
pub struct StoreContextInternal<'b, 'a, T: 'static> {
    context: &'b mut StoreContext<'a, T>,
}

impl<'b, 'a, T: 'static> StoreContextInternal<'b, 'a, T> {
    /// The internal face of `context`.
    pub fn of(context: &'b mut StoreContext<'a, T>) -> Self {
        Self { context }
    }

    /// Borrow the context again, for the length of the wrapper's own
    /// borrow.
    pub fn reborrow(self) -> StoreContext<'b, T> {
        self.context.reborrow()
    }

    /// Borrow the core store's context, which is what every crossing
    /// of the canonical ABI and every call into the guest runs
    /// against.
    pub fn runtime(self) -> &'b RuntimeContextMut<'a, StoreData<T>> {
        self.context.runtime()
    }

    /// Mutably borrow the core store's context.
    pub fn runtime_mut(self) -> &'b mut RuntimeContextMut<'a, StoreData<T>> {
        self.context.runtime_mut()
    }

    /// The store's process-unique identity.
    pub fn id(self) -> StoreId {
        self.context.id()
    }

    /// The store's handle tables.
    pub fn tables(self) -> &'b Arc<Mutex<HandleTables>> {
        self.context.tables()
    }

    /// Clone the handle for the per-store handle-tables ledger.
    pub fn tables_handle(self) -> Arc<Mutex<HandleTables>> {
        self.context.tables_handle()
    }

    /// Lock the store's handle tables and record state.
    pub fn lock_tables(self) -> Result<MutexGuard<'b, HandleTables>> {
        self.context.lock_tables()
    }

    /// Set the most records the store holds live before a new one
    /// fails. A cap below the records live now removes none of them;
    /// only the records created after it fail. Nothing public sets
    /// the cap: this entry is the only way to it, and only the
    /// crate's tests and the conformance harness's entry call it, so
    /// it is compiled for them alone.
    #[cfg(any(test, feature = "wast-runner"))]
    pub fn set_max_records(self, max: usize) -> Result<()> {
        self.context.lock_tables()?.tasks.set_max_records(max);
        Ok(())
    }

    /// The store's cooperative scheduler.
    pub fn scheduler(self) -> &'b Scheduler<T> {
        self.context.scheduler()
    }

    /// The store's cooperative scheduler, mutably.
    pub fn scheduler_mut(self) -> &'b mut Scheduler<T> {
        self.context.scheduler_mut()
    }

    /// Record what the store knows about a resource type an
    /// instantiation introduced.
    pub fn register_resource(
        self,
        type_id: ResourceTypeId,
        name: Option<ResourceType>,
        destructor: ResourceDestructor<T>,
    ) {
        self.context.register_resource(type_id, name, destructor);
    }

    /// Record a label to fall back on for `type_id` while no
    /// component in this store has named it.
    pub fn fallback_resource_name(self, type_id: ResourceTypeId, name: ResourceType) {
        self.context.fallback_resource_name(type_id, name);
    }

    /// What the store knows about `type_id` at this moment, as a
    /// record an instantiation whose plan fails hands back.
    pub fn resource_record(self, type_id: ResourceTypeId) -> ResourceRecord {
        self.context.resource_record(type_id)
    }

    /// Put back what the store knew about one resource type before
    /// an instantiation registered it.
    pub fn restore_resource(self, record: ResourceRecord) {
        self.context.restore_resource(record);
    }

    /// Release a handle the host holds.
    pub fn resource_drop(self, handle: ResourceHandle) -> Result<()> {
        self.context.resource_drop(handle)
    }

    /// Run one turn of the store's scheduler.
    pub fn turn(self, waker: &Waker) -> Result<Outcome> {
        self.context.turn(waker)
    }

    /// Whether a turn of this store is running.
    pub fn turn_in_flight(self) -> bool {
        self.context.turn_in_flight()
    }

    /// Whether the store holds work only a turn can carry forward.
    pub fn has_pending_work(self) -> bool {
        self.context.has_pending_work()
    }

    /// Run `body` with the store's turn state raised, so that what
    /// it does runs as one turn does.
    pub fn run_in_turn<R>(
        self,
        waker: &Waker,
        body: impl FnOnce(&mut StoreContext<'a, T>) -> R,
    ) -> Result<R> {
        self.context.run_in_turn(waker, body)
    }

    /// Run one nested turn, or go on with the one that last stopped
    /// for work it left to the store.
    pub fn continue_nested_turn(
        self,
        waker: &Waker,
        only: Option<InstanceId>,
        resume: bool,
    ) -> Result<Outcome> {
        self.context.continue_nested_turn(waker, only, resume)
    }

    /// Whether the frame that runs now left work to the store that it
    /// must not go past.
    pub fn defers_work(self) -> bool {
        self.context.defers_work()
    }

    /// Whether the store is inside work frames left to it.
    pub fn deferred_busy(self) -> bool {
        self.context.deferred_busy()
    }

    /// Leave `plan` for the thread the running trampoline runs in.
    pub fn leave_plan(self, plan: Plan<T>) -> Result<()> {
        self.context.leave_plan(plan)
    }

    /// Poll the parked host task of the synchronous lower of the
    /// call `subtask` records, once, and settle it if it completed.
    pub fn poll_parked_call(self, subtask: SubtaskId) -> Result<()> {
        self.context.poll_parked_call(subtask)
    }

    /// The instance a nested turn must not block, when one is set.
    pub fn must_not_block_instance(self) -> Option<InstanceId> {
        self.context.must_not_block_instance()
    }

    /// The waker of the turn that is running, or a no-op waker.
    pub fn active_waker(self) -> Waker {
        self.context.active_waker()
    }

    /// The cause an idle store reports for `task`.
    pub fn idle_cause(self, task: Option<TaskId>) -> SchedulerCause {
        self.context.idle_cause(task)
    }

    /// The cause a refused suspend reports.
    pub fn suspend_cause(self) -> SchedulerCause {
        self.context.suspend_cause()
    }

    /// The cause a refused suspend of a thread whose instance,
    /// `instance`, must not suspend reports.
    pub fn suspend_cause_in(self, instance: InstanceId) -> SchedulerCause {
        self.context.suspend_cause_in(instance)
    }

    /// Start a host task, polling it once before it is queued.
    pub fn start_host_task(
        self,
        task: HostTask<T>,
        caller: TableId,
        lower: LowerKind,
    ) -> Result<CallStatus> {
        self.context.start_host_task(task, caller, lower)
    }

    /// Queue a host task the store will poll on its next turn.
    pub fn push_host_task(self, task: HostTask<T>) {
        self.context.push_host_task(task);
    }

    /// Create the task record for a call of an export.
    pub fn create_export_task(
        self,
        function: Arc<Signature>,
        options: Arc<CanonOptions>,
        instance: InstanceId,
    ) -> Result<TaskId> {
        self.context.create_export_task(function, options, instance)
    }

    /// Queue `item` as the start of `task`'s implicit thread, past
    /// the entry gate of `instance`.
    pub fn start_export_thread(
        self,
        task: TaskId,
        instance: InstanceId,
        async_function: bool,
        needs_exclusive: bool,
        item: Item<T>,
    ) -> Result<()> {
        self.context
            .start_export_thread(task, instance, async_function, needs_exclusive, item)
    }

    /// Enter `task`'s implicit thread through the switch slot.
    pub fn start_switched_export_thread(
        self,
        task: TaskId,
        instance: InstanceId,
        async_function: bool,
        needs_exclusive: bool,
    ) -> Result<()> {
        self.context
            .start_switched_export_thread(task, instance, async_function, needs_exclusive)
    }

    /// Poll the body of a synchronously lowered host call once, and
    /// park it when it is still running.
    pub fn begin_blocking_host_task(self, task: HostTask<T>) -> Result<Option<SubtaskId>> {
        self.context.begin_blocking_host_task(task)
    }

    /// Finish a synchronously lowered host call whose body was
    /// parked.
    pub fn finish_blocking_host_task(self, subtask: SubtaskId, waited: Result<()>) -> Result<()> {
        self.context.finish_blocking_host_task(subtask, waited)
    }

    /// Run whatever the switch slot holds.
    pub fn run_switch_slot(self) -> Result<()> {
        self.context.run_switch_slot()
    }

    /// Put the resumption of the parked `thread` in the switch slot.
    pub fn switch_to_parked_thread(self, thread: ThreadId) -> Result<bool> {
        self.context.switch_to_parked_thread(thread)
    }

    /// How deep the stack of current scopes is.
    pub fn scope_depth(self) -> Result<usize> {
        self.context.scope_depth()
    }

    /// The implicit thread of `task`.
    pub fn implicit_thread(self, task: TaskId) -> Result<ThreadId> {
        self.context.implicit_thread(task)
    }

    /// Fail every thread suspended in the provider with the cause an
    /// idle store gives for `task`.
    pub fn fail_parked_threads(self, task: Option<TaskId>) -> Result<bool> {
        self.context.fail_parked_threads(task)
    }

    /// The provider the store keeps, when the engine selected one.
    pub fn provider(self) -> Option<StoreProvider> {
        self.context.provider()
    }

    /// Run the store's flight, the start or the resume of a thread a
    /// turn left for the driver, until the thread stops. A driver awaits
    /// this whenever a turn ends in [`Outcome::Resuming`].
    ///
    /// The flight is the rest of the turn that left it, so a turn runs
    /// for as long as the thread does, with the waker of the driver
    /// that awaits it: a trampoline the thread calls finds the turn and
    /// its waker, as it would inside the turn itself.
    pub async fn fly(self) {
        self.context.fly().await;
    }

    /// Whether the store's owner dropped it while a thread the
    /// provider resumed had yet to run.
    pub fn dropped(self) -> bool {
        self.context.dropped()
    }

    /// Record that a trap happened in the store, and discard the
    /// work it holds.
    pub fn poison(self) {
        self.context.poison();
    }

    /// Refuse a host entry into a guest of a poisoned store, with the
    /// cannot-enter cause.
    pub fn enter_guest(self) -> Result<()> {
        self.context.enter_guest()
    }

    /// Run a thread entry, through the provider when the store has
    /// one, and hand what it produced to `finish`.
    pub fn run_thread_entry(
        self,
        thread: ThreadId,
        base: usize,
        entry: &RuntimeFunc,
        args: &[RuntimeVal],
        results: Vec<RuntimeVal>,
        finish: impl EntryFinish<T>,
    ) -> Result<()> {
        self.context
            .run_thread_entry(thread, base, entry, args, results, finish)
    }

    /// Start an explicit thread `thread.resume-later` made ready
    /// before it ever ran.
    pub fn start_ready_thread(self, thread: ThreadId) -> Result<()> {
        self.context.start_ready_thread(thread)
    }

    /// Run a thread a switch named from a built-in on the real stack,
    /// through the provider.
    pub fn run_switched_thread(self, thread: ThreadId) -> Result<()> {
        self.context.run_switched_thread(thread)
    }

    /// Attach a result channel to `task`.
    pub fn attach_result_channel(self, task: TaskId) -> Result<ResultChannel> {
        self.context.attach_result_channel(task)
    }

    /// Whether `task` has resolved.
    pub fn export_task_resolved(self, task: TaskId) -> Result<bool> {
        self.context.export_task_resolved(task)
    }

    /// Whether `instance` is held by an exclusive thread.
    pub fn instance_is_held(self, instance: InstanceId) -> Result<bool> {
        self.context.instance_is_held(instance)
    }

    /// Release the exclusive hold `task` has on its instance.
    pub fn release_exclusive_thread(self, task: TaskId) -> Result<()> {
        self.context.release_exclusive_thread(task)
    }

    /// Take an exclusive hold on `instance` for `task`.
    pub fn take_exclusive_thread(self, task: TaskId, instance: InstanceId) -> Result<()> {
        self.context.take_exclusive_thread(task, instance)
    }

    /// Park `task`'s callback thread on a waitable set.
    pub fn wait_callback_on_set(
        self,
        task: TaskId,
        instance: InstanceId,
        table: TableId,
        set_index: u32,
        slot: EventSlot,
        item: Item<T>,
    ) -> Result<()> {
        self.context
            .wait_callback_on_set(task, instance, table, set_index, slot, item)
    }

    /// Queue or hold `task`'s callback item, whose callback waits on
    /// `set`.
    pub fn park_callback_on_set(
        self,
        task: TaskId,
        instance: InstanceId,
        set: WaitableSetId,
        slot: EventSlot,
        item: Item<T>,
    ) -> Result<()> {
        self.context
            .park_callback_on_set(task, instance, set, slot, item)
    }

    /// Enter `task`, which is what a call of an export does.
    pub fn enter_export_task(self, task: TaskId) -> Result<()> {
        self.context.enter_export_task(task)
    }

    /// Hold `task`'s current thread to the may-not-suspend rule.
    pub fn hold_may_not_suspend(self, task: TaskId) -> Result<()> {
        self.context.hold_may_not_suspend(task)
    }

    /// Move `task` into its started state.
    pub fn start_export_task(self, task: TaskId) -> Result<()> {
        self.context.start_export_task(task)
    }

    /// Resolve `task` with `result`.
    pub fn resolve_export_task(self, task: TaskId, result: Option<Val>) -> Result<()> {
        self.context.resolve_export_task(task, result)
    }

    /// Exit `task`'s current thread.
    pub fn exit_export_task(self, task: TaskId) -> Result<core::result::Result<(), u32>> {
        self.context.exit_export_task(task)
    }

    /// Leave `task`, which is what a return from the guest does.
    pub fn leave_export_task(self, task: TaskId) -> Result<()> {
        self.context.leave_export_task(task)
    }

    /// End `task`'s implicit thread without ending the task, when the
    /// task holds an explicit thread, and answer whether it did.
    pub fn leave_implicit_thread(self, task: TaskId) -> Result<bool> {
        self.context.leave_implicit_thread(task)
    }

    /// End `task` when the thread of it that just ended was its last.
    pub fn end_last_thread(self, task: TaskId) -> Result<()> {
        self.context.end_last_thread(task)
    }

    /// End `task`.
    pub fn end_export_task(self, task: TaskId) -> Result<core::result::Result<(), u32>> {
        self.context.end_export_task(task)
    }

    /// Abandon `task`, dropping whatever it still holds.
    pub fn abandon_export_task(self, task: TaskId) -> Result<()> {
        self.context.abandon_export_task(task)
    }

    /// Run `body` with an accessor to this store, driving the
    /// store's scheduler until the future `body` returns completes.
    pub async fn run_concurrent<R, F>(self, body: F) -> Result<R>
    where
        F: AsyncFnOnce(&Accessor<T>) -> R,
    {
        self.context.reborrow().run_concurrent(body).await
    }
}

/// The workspace-internal entries of a [`StoreContext`] that need
/// no mutable access, over a shared borrow of one.
///
/// A host call reaches the store through a shared borrow while it
/// mints a handle, so the entries that reading needs live here as
/// well as on [`StoreContextInternal`].
pub struct StoreContextRefInternal<'b, 'a, T: 'static> {
    context: &'b StoreContext<'a, T>,
}

impl<'b, 'a, T: 'static> StoreContextRefInternal<'b, 'a, T> {
    /// The internal face of `context`.
    pub fn of(context: &'b StoreContext<'a, T>) -> Self {
        Self { context }
    }

    /// The store's handle tables.
    pub fn tables(self) -> &'b Arc<Mutex<HandleTables>> {
        self.context.tables()
    }

    /// Lock the store's handle tables and record state.
    pub fn lock_tables(self) -> Result<MutexGuard<'b, HandleTables>> {
        self.context.lock_tables()
    }

    /// The name the store renders for the resource type `type_id`,
    /// when it learned one.
    pub fn resource_type(self, type_id: ResourceTypeId) -> Option<ResourceType> {
        self.context.resource_type(type_id)
    }

    /// How many resource types the store has a destructor
    /// registered for.
    pub fn registered_destructors(self) -> usize {
        self.context.registered_destructors()
    }

    /// How many resource types the store has learned a name for.
    pub fn learned_resource_names(self) -> usize {
        self.context.learned_resource_names()
    }
}

/// The seam crate code reaches [`StoreContextInternal`] and
/// [`StoreContextRefInternal`] through.
///
/// The trait lives in a private module, so it cannot be imported
/// outside the crate and `context.internal()` resolves only inside
/// it. It also carries the context's constructor, which a trampoline
/// calls with the borrow the runtime layer handed it.
pub trait StoreContextInternalExt<'a, T: 'static> {
    /// The store a borrow of the core store reaches.
    fn new(runtime: RuntimeContextMut<'a, StoreData<T>>) -> Self;

    /// The workspace-internal entries of this context.
    fn internal(&mut self) -> StoreContextInternal<'_, 'a, T>;

    /// The workspace-internal entries of this context that need no
    /// mutable access.
    fn internal_ref(&self) -> StoreContextRefInternal<'_, 'a, T>;
}

impl<'a, T: 'static> StoreContextInternalExt<'a, T> for StoreContext<'a, T> {
    fn new(runtime: RuntimeContextMut<'a, StoreData<T>>) -> Self {
        StoreContext::from_runtime(runtime)
    }

    fn internal(&mut self) -> StoreContextInternal<'_, 'a, T> {
        StoreContextInternal::of(self)
    }

    fn internal_ref(&self) -> StoreContextRefInternal<'_, 'a, T> {
        StoreContextRefInternal::of(self)
    }
}
