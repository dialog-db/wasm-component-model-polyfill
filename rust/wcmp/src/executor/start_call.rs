//! What the two start intrinsics of a fused adapter share.
//!
//! The prepare intrinsic of [`super::prepare_call`] leaves one
//! prepared call in the store. A start intrinsic takes it out,
//! builds the item that starts the callee's implicit thread, puts
//! that item in the scheduler's switch slot, and enters the thread
//! through the entry gate. A callee the gate lets through keeps the
//! slot, and the intrinsic runs it from inside its own frame; a
//! callee the gate holds leaves the slot empty and waits there in
//! arrival order. That is the reference resuming the callee's thread
//! before the lower returns.
//!
//! [`Prepared`] is that call, from the moment a start intrinsic
//! takes it to the moment its item is queued. The two intrinsics
//! differ in what they do afterwards — a synchronous lower blocks
//! for the callee's result, an asynchronous one answers with the
//! status word — and in how the item reports a failure, which is
//! what [`Prepared::item`] and
//! [`Prepared::item_reporting_to`] name.
//!
//! What the item does is the same for both:
//!
//! - It calls the start function with the caller's flat arguments,
//!   which lifts them in the caller and lowers them into the callee.
//! - It marks the subtask started, which fills the subtask's pending
//!   event when the caller already holds a handle for it — the
//!   callee the gate held and later let through.
//! - It calls the callee's core function.
//! - A callee lifted with a callback returns a status word, which
//!   goes to the callback loop of [`crate::executor::CallbackTask`].
//!   A stackful callee returns nothing, and its return ends its
//!   implicit thread, which fails the call with the no-result cause
//!   when the callee has not called `task.return` by then.
//!   [`crate::executor::AsyncLift`] names the two forms. A
//!   synchronously lifted callee returns its flat results, which
//!   cross into the caller through the return function at that
//!   moment; its `post-return` runs afterwards, inside its own task,
//!   and the task then ends.
//!
//! A trap in the callee, or a failure of either generated function,
//! ends the call: the subtask's resolution is a cancellation, which
//! gives back every handle the caller lent, and the failure travels
//! to whichever of the two intrinsics is waiting for it.
//!
//! The handles come back with the cancellation because the lends
//! the start function recorded are on the subtask, which is the
//! record of the call, rather than on the callee's task.
//! [`HandleTables::lend_to`] states that rule.

use std::sync::{Arc, Mutex};

use crate::abi::boundary_call::BoundaryCall;
use crate::abi::instance::BoundaryInstance;
use crate::abi::runtime_state::AbiRuntimeState;
use crate::concurrency::{InstanceId, Item, ItemKind, LowerKind, SubtaskId, TaskId};
use crate::error::{AbiCause, AbiError, AbiPosition, Error, Result};
use crate::executor::{AsyncLift, CallbackTask};
use crate::internal::ErrorInternal;
use crate::resource::{HandleTables, TableId};
use crate::runtime_layer::{Func as RuntimeFunc, Val as RuntimeVal, substrate_failure};
use crate::store::StoreContext;
use crate::store::StoreContextInternalExt;

use super::prepare_call::take_prepared_call;
use super::start_failure::StartFailure;
use super::task_return::cross_result_into_caller;

/// One prepared call, taken out of the store by a start intrinsic.
///
/// The identities come off the records the prepare intrinsic made.
/// The callee — its core function, how many flat parameters and
/// results that function has, and what happens when it returns —
/// comes off the arguments the start intrinsic was called with, so
/// it is named by [`Prepared::with_callee`] rather than read here.
pub struct Prepared {
    /// The caller's subtask, the record of the call.
    subtask: SubtaskId,
    /// The callee's task.
    task: TaskId,
    /// The callee's component instance.
    instance: InstanceId,
    /// The same instance, by the translator's per-instantiation
    /// index, which is what the ABI state is keyed on.
    instance_index: usize,
    /// Whether the callee's function type carries the `async`
    /// effect, which decides whether its task waits at the gate.
    callee_async_typed: bool,
    /// The callee, once the start intrinsic has named it.
    callee: Option<Callee>,
}

/// The callee of a prepared call: what the start item runs, and what
/// it does with what that run produced. Every field is a handle the
/// runtime layer clones cheaply, so the item owns a copy of the whole
/// of it.
#[derive(Clone)]
struct Callee {
    /// The callee's core function.
    function: RuntimeFunc,
    /// How many flat parameters that function takes, which the
    /// adapter names because the types are its own.
    param_count: usize,
    /// How many flat results it returns. A callee lifted with a
    /// callback returns the status word, which the adapter counts as
    /// one, and a stackful callee returns nothing.
    result_count: usize,
    /// The form of an asynchronously lifted callee's lift, which
    /// says what its core function's return does. `None` for a
    /// synchronously lifted callee, whose results cross into the
    /// caller as its core function returns.
    lift: Option<AsyncLift>,
    /// The `post-return` of a synchronously lifted callee, which
    /// runs once its results have crossed, with the callee's
    /// component instance as a crossing names it: that is what holds
    /// the may-leave flag clear around the call.
    post_return: Option<(RuntimeFunc, BoundaryInstance)>,
}

impl Prepared {
    /// Take the call the prepare intrinsic left in the store and
    /// read the records it made.
    pub fn take(tables: &Arc<Mutex<HandleTables>>) -> Result<Self> {
        let subtask = take_prepared_call(tables)?;
        let guard = lock(tables)?;
        let record = guard
            .tasks
            .subtask(subtask)
            .ok_or_else(|| Error::internal("a prepared call has no subtask record"))?;
        let task = record
            .callee
            .ok_or_else(|| Error::internal("a prepared call names no callee task"))?;
        let callee_async_typed = record
            .bridge
            .as_ref()
            .ok_or_else(|| Error::internal("a prepared call carries no generated functions"))?
            .callee_async_typed;
        let task_record = guard
            .tasks
            .task(task)
            .ok_or_else(|| Error::internal("a prepared call's callee task is not in the store"))?;
        let instance = task_record
            .instance
            .ok_or_else(|| Error::internal("a prepared call's callee belongs to no instance"))?;
        let instance_index = task_record
            .options
            .as_ref()
            .map(|options| options.instance)
            .ok_or_else(|| Error::internal("a prepared call's callee task carries no options"))?;
        Ok(Self {
            subtask,
            task,
            instance,
            instance_index,
            callee_async_typed,
            callee: None,
        })
    }

    /// Name the callee the start intrinsic was called with: its core
    /// function, the count of its flat parameters and results, the
    /// form of an asynchronously lifted callee's lift, and the
    /// `post-return` of a synchronously lifted one.
    pub fn with_callee(
        mut self,
        function: RuntimeFunc,
        counts: (usize, usize),
        lift: Option<AsyncLift>,
        post_return: Option<(RuntimeFunc, BoundaryInstance)>,
    ) -> Self {
        let (param_count, result_count) = counts;
        self.callee = Some(Callee {
            function,
            param_count,
            result_count,
            lift,
            post_return,
        });
        self
    }

    /// The caller's subtask, the record of the call.
    pub fn subtask(&self) -> SubtaskId {
        self.subtask
    }

    /// The callee's task.
    pub fn task(&self) -> TaskId {
        self.task
    }

    /// The form of an asynchronously lifted callee's lift, with the
    /// `async` option and the callback recorded on its task so that
    /// the callee's `task.return` finds them set. `callback` is the
    /// runtime callback slot the adapter named, and a lift that named
    /// none is the stackful form.
    pub fn async_lift(
        &self,
        tables: &Arc<Mutex<HandleTables>>,
        abi_state: &Arc<Mutex<AbiRuntimeState>>,
        callback: Option<usize>,
    ) -> Result<AsyncLift> {
        if let Some(options) = lock(tables)?
            .tasks
            .task_mut(self.task)
            .and_then(|record| record.options.as_mut().map(Arc::make_mut))
        {
            options.async_ = true;
            options.callback = callback;
        }
        let Some(callback) = callback else {
            return Ok(AsyncLift::Stackful(self.task));
        };
        let (function, table) = callee_callback(abi_state, self.instance_index, callback)?;
        Ok(AsyncLift::Callback(CallbackTask::new(
            self.task,
            self.instance,
            table,
            function,
        )))
    }

    /// The item that starts the callee's implicit thread and fails
    /// the turn that ran it when the call fails.
    ///
    /// This is what an asynchronous lower takes. Its trampoline has
    /// the item's run under it while the switch slot runs, so a
    /// failure there travels straight out to the caller's call; once
    /// the trampoline has answered with the status word, a callee
    /// the gate held later fails a driver's turn instead, which is
    /// Wasmtime's rule for a task that keeps running after its call
    /// returned.
    pub fn item<T: 'static>(&self) -> Result<Item<T>> {
        self.build_item(None)
    }

    /// The item that starts the callee's implicit thread and leaves
    /// what it fails with in `failure`.
    ///
    /// This is what a synchronous lower takes. Its trampoline blocks
    /// for the callee's result, so a failure must not end the turn
    /// that ran the item — the turn can be a nested one the block
    /// itself is running — and the trampoline reads the slot when
    /// the block returns.
    pub fn item_reporting_to<T: 'static>(&self, failure: StartFailure) -> Result<Item<T>> {
        self.build_item(Some(failure))
    }

    /// Start the callee's implicit thread with `item`, from inside
    /// the start intrinsic's frame, for a caller that lowered the
    /// call through `lower`.
    ///
    /// The item goes in the switch slot and the thread enters the
    /// gate. A callee the gate lets through keeps the slot and runs
    /// here. A callee the gate holds leaves the slot empty and waits
    /// in arrival order, so nothing runs.
    ///
    /// An `async`-typed callee that runs here is a nested start. The
    /// reference runs it on a stack of its own and returns to this
    /// frame when it blocks, and so does the store's provider: the
    /// callee's thread starts through it, and the start returns here
    /// once the thread suspends or finishes. Without a provider the
    /// callee runs on the real stack above this frame, and the frames
    /// below stay where they are until it returns. The stack of
    /// current scopes carries a mark with the call's subtask and
    /// `lower` for as long as the start runs. A block that fails above
    /// the mark names the stack-switch cause when the caller would go
    /// on once a provider returned control to it, and could release
    /// the block: always after an asynchronous lower, and after a
    /// synchronous lower once the callee has resolved. A sync-typed
    /// callee is not a nested start. Its task must not block, so it
    /// never suspends, and it runs on the caller's stack, except under
    /// a provider that resumes a thread only where the store runs no
    /// guest code: its thread starts through that provider then, on a
    /// stack of its own, as the reference runs every callee's thread.
    ///
    /// The callee's task takes the exclusive thread of its instance
    /// unless it is stackful, which is the reference's
    /// `needs_exclusive`: `not opts.async or opts.callback`.
    pub fn run_start<T: 'static>(
        &self,
        store: &mut StoreContext<'_, T>,
        item: Item<T>,
        lower: LowerKind,
    ) -> Result<()> {
        let needs_exclusive = self
            .callee
            .as_ref()
            .and_then(|callee| callee.lift.as_ref())
            .is_none_or(AsyncLift::needs_exclusive);
        store.internal().scheduler_mut().switch_to(item);
        store.internal().start_switched_export_thread(
            self.task,
            self.instance,
            self.callee_async_typed,
            needs_exclusive,
        )?;
        if !self.callee_async_typed {
            return store.internal().run_switch_slot();
        }
        run_nested_start(store, self.subtask, self.instance, lower)
    }

    /// Remove the subtask record of a call that failed, once the
    /// trampoline has taken its failure.
    pub fn remove(&self, tables: &Arc<Mutex<HandleTables>>) {
        if let Ok(mut guard) = tables.lock() {
            guard.tasks.remove_subtask(self.subtask);
        }
    }

    /// Build the item. `failure` is the slot a failure goes to, and
    /// `None` fails the turn that ran the item instead.
    ///
    /// The item names the callee's task, so a gate that is still
    /// holding it when the call fails gives it up with the task's
    /// record rather than starting a callee the caller has given up on.
    fn build_item<T: 'static>(&self, failure: Option<StartFailure>) -> Result<Item<T>> {
        let callee = self
            .callee
            .as_ref()
            .ok_or_else(|| Error::internal("a prepared call was started with no callee"))?
            .clone();
        let subtask = self.subtask;
        let task = self.task;
        let nested = self.callee_async_typed;
        Ok(Item::new(
            ItemKind::TaskStart,
            move |store: &mut StoreContext<'_, T>| {
                let report = StartReport { subtask, failure };
                start_call(store, subtask, task, callee, nested, report)
            },
        )
        .for_task(task))
    }
}

/// Where the failure of a prepared call goes: the slot a synchronous
/// lower reads, or, with no slot, the turn that ran the start.
struct StartReport {
    /// The caller's record of the call.
    subtask: SubtaskId,
    /// The slot a synchronous lower reads, or `None` for an
    /// asynchronous lower.
    failure: Option<StartFailure>,
}

impl StartReport {
    /// End the call with `error`: its subtask resolves as a
    /// cancellation, and the failure goes to the slot, or fails the
    /// turn when there is none.
    fn fail<T: 'static>(self, store: &mut StoreContext<'_, T>, error: Error) -> Result<()> {
        abandon(store, self.subtask);
        let Some(failure) = self.failure else {
            return Err(error);
        };
        if let Ok(mut slot) = failure.lock() {
            *slot = Some(error);
        }
        Ok(())
    }
}

/// Run the callee: lower the arguments through the start function,
/// call the core function, and act on what it produced. A failure
/// anywhere in that goes to `report`.
///
/// The core function of an `async`-typed callee is the entry of the
/// callee's implicit thread, which starts through the store's
/// provider when there is one: from inside the start intrinsic, that
/// is the nested start the reference makes, on a stack of its own,
/// and the intrinsic goes on once the thread suspends or finishes.
/// What the start does once the core function returns runs when the
/// entry finishes. A sync-typed callee's task must not block, so its
/// thread never suspends, and its core function is a direct call on
/// the caller's stack. Under a provider that resumes a thread only
/// where the store runs no guest code, it starts through the provider
/// on a stack of its own instead, as the reference runs every
/// callee's thread: the ready threads its task gives way to while it
/// waits can then run with its stack set aside.
fn start_call<T: 'static>(
    store: &mut StoreContext<'_, T>,
    subtask: SubtaskId,
    task: TaskId,
    callee: Callee,
    nested: bool,
    report: StartReport,
) -> Result<()> {
    let base = store.internal().scope_depth()?;
    // The callee's task is the current scope for the whole of the
    // start: the arguments the start function lowers are the
    // callee's, and a borrow the adapter transfers in is owed to it.
    if let Err(error) = store.internal().enter_export_task(task) {
        return report.fail(store, error);
    }
    let core_arguments = match call_start_function(store, subtask, callee.param_count) {
        Ok(arguments) => arguments,
        Err(error) => {
            let error = abandoned(store, task, error);
            return report.fail(store, error);
        }
    };
    {
        let tables = store.internal().tables_handle();
        lock(&tables)?.tasks.start_subtask(subtask);
    }
    if let Err(error) = store.internal().start_export_task(task) {
        return report.fail(store, error);
    }

    // The callee's flat result types are the adapter's own, and the
    // adapter names only how many there are. A status word is an
    // `i32`, and a stackful callee returns nothing. A synchronously
    // lifted callee's one flat result can be of any type, so its
    // slot is filled with the widest flat value, which every backend
    // overwrites with the value and the type the core function
    // returned.
    let placeholder = match callee.lift {
        Some(_) => RuntimeVal::I32(0),
        None => RuntimeVal::F64(0.0),
    };
    let slots = vec![placeholder; callee.result_count];
    let function = callee.function.clone();
    let finish = move |store: &mut StoreContext<'_, T>, called: Result<Vec<RuntimeVal>>| {
        let outcome = match called {
            Err(error) => Err(abandoned(store, task, error)),
            Ok(core_results) => match &callee.lift {
                Some(lift) => store
                    .internal()
                    .leave_export_task(task)
                    .and_then(|()| lift.returned(store, &core_results)),
                None => resolve_sync_lift(store, subtask, task, &callee, &core_results)
                    .map_err(|error| abandoned(store, task, error)),
            },
        };
        match outcome {
            Ok(()) => Ok(()),
            Err(error) => report.fail(store, error),
        }
    };
    // Under a provider that resumes a thread only where the store runs
    // no guest code, a sync-typed callee starts through the provider
    // too, on a stack of its own. It must not block, and its task gives
    // way to the ready threads of its own instance when it waits, which
    // such a provider can resume only with the callee's stack set
    // aside: a callee called from this frame would have a host frame
    // below it that no suspension may cross.
    let own_stack = store
        .internal()
        .provider()
        .is_some_and(|provider| provider.resumes_later());
    if !nested && !own_stack {
        let mut core_results = slots;
        let called = function
            .call(
                store.internal().runtime_mut(),
                &core_arguments,
                &mut core_results,
            )
            .map_err(substrate_failure)
            .map(|()| core_results);
        return finish(store, called);
    }
    let thread = store.internal().implicit_thread(task)?;
    store
        .internal()
        .run_thread_entry(thread, base, &function, &core_arguments, slots, finish)
}

/// End `task` on its failure path, and answer the failure the call
/// reports: `error`, or the failure the end itself met.
fn abandoned<T: 'static>(store: &mut StoreContext<'_, T>, task: TaskId, error: Error) -> Error {
    match store.internal().abandon_export_task(task) {
        Ok(()) => error,
        Err(failure) => failure,
    }
}

/// End a synchronously lifted callee's call: its flat results cross
/// into the caller through the return function, its `post-return`
/// runs, and its task ends.
///
/// The crossing is the reference's `on_resolve`, at the moment the
/// reference runs it for such a callee: the core function has
/// returned, so the result is there, and the caller's memory is
/// written before anything else of the callee runs. The
/// `post-return` follows, inside the callee's own task and with the
/// callee's instance unleavable, as it does for a synchronous call.
fn resolve_sync_lift<T: 'static>(
    store: &mut StoreContext<'_, T>,
    subtask: SubtaskId,
    task: TaskId,
    callee: &Callee,
    core_results: &[RuntimeVal],
) -> Result<()> {
    let tables = store.internal().tables_handle();
    cross_result_into_caller(
        store.internal().runtime_mut(),
        &tables,
        task,
        subtask,
        None,
        core_results,
    )?;
    if let Some((post_return, boundary)) = &callee.post_return {
        let call = BoundaryCall::post_return(boundary, store.internal().runtime_mut())?;
        let mut empty: [RuntimeVal; 0] = [];
        let ran = post_return
            .call(store.internal().runtime_mut(), core_results, &mut empty)
            .map_err(substrate_failure);
        call.end(store.internal().runtime_mut())?;
        ran?;
    }
    match store.internal().exit_export_task(task)? {
        Ok(()) => Ok(()),
        Err(count) => Err(outstanding_borrows(count)),
    }
}

/// Call the start function of the prepared call with the caller's
/// flat arguments, and hand back the callee's flat parameters.
fn call_start_function<T: 'static>(
    store: &mut StoreContext<'_, T>,
    subtask: SubtaskId,
    param_count: usize,
) -> Result<Vec<RuntimeVal>> {
    let tables = store.internal().tables_handle();
    let (start, arguments) = {
        let guard = lock(&tables)?;
        let bridge = guard
            .tasks
            .subtask(subtask)
            .and_then(|record| record.bridge.as_ref())
            .ok_or_else(|| Error::internal("a prepared call carries no generated functions"))?;
        // The start function takes the caller's flat arguments and
        // nothing else. A caller that takes its result through a
        // return pointer passed that pointer as its last flat
        // argument, and the pointer belongs to the return function
        // rather than to this one.
        let mut arguments = bridge.arguments.clone();
        if bridge.caller.has_return_pointer() {
            arguments.pop();
        }
        (bridge.start.clone(), arguments)
    };
    // The callee's flat parameter types are the adapter's own, and
    // the adapter names only how many there are. The slots are
    // filled with the widest flat value, which every backend
    // overwrites with the value and the type the start function
    // returned.
    let mut results = vec![RuntimeVal::F64(0.0); param_count];
    start
        .call(store.internal().runtime_mut(), &arguments, &mut results)
        .map_err(substrate_failure)?;
    Ok(results)
}

/// End a prepared call whose start failed: the subtask's resolution
/// is a cancellation, and the handles the caller lent for the call
/// are given back with it. The lends are on the subtask, so the
/// delivery of the cancellation is what gives them back, under the
/// rule [`HandleTables::lend_to`] states. A callee that `task.return`s
/// and keeps running holds none of them past that delivery.
pub fn abandon<T: 'static>(store: &mut StoreContext<'_, T>, subtask: SubtaskId) {
    let tables = store.internal().tables_handle();
    let Ok(mut guard) = tables.lock() else {
        return;
    };
    let _ = guard.tasks.subtask_cancelled(subtask);
    let _ = guard.deliver_subtask_resolution(subtask);
}

/// Take the caller's record of a failed call out of the store: the
/// resolution is a cancellation, which gives back every handle the
/// caller lent, the entry the caller holds for the subtask leaves
/// that caller's table, and the record itself is removed.
///
/// A prepared call's subtask is never on the stack of scopes — the
/// caller's task holds the stack while the callee runs, and an
/// asynchronous lower hands the record back to the caller to wait on
/// — so the scope stack has nothing to unwind for it and
/// `HandleTables::abandon_subtask` would find nothing to do. This is
/// what abandoning one means instead. The record still carries the
/// call's lends, which the start function named it for rather than
/// reading the stack, so the delivery here is what gives them back.
///
/// The entry goes with the record because the two are the caller's
/// one handle on the call: a record removed while an entry still
/// named it would leave the caller an index that resolves to
/// nothing. A call that failed before the lower returned was never
/// given an entry, and then there is only the record to remove.
///
/// A call into a host function has no generated functions to name
/// the caller's table, so its caller passes that table as `table`.
/// A prepared call names its own, and `table` is `None` there.
///
/// The cancellation is recorded even for a subtask that had already
/// returned, where cancelling is not the state the resolution would
/// otherwise reach. Nothing reads the difference: the record and the
/// caller's entry for it leave the store in the same breath, so the
/// state it was moved to has no one left to observe it.
pub fn release_subtask<T: 'static>(
    store: &mut StoreContext<'_, T>,
    subtask: SubtaskId,
    table: Option<TableId>,
) {
    abandon(store, subtask);
    let tables = store.internal().tables_handle();
    let Ok(mut guard) = tables.lock() else {
        return;
    };
    let entry = guard.tasks.subtask(subtask).and_then(|record| {
        let table = record
            .bridge
            .as_ref()
            .map(|bridge| bridge.caller_table)
            .or(table)?;
        Some((table, record.handle?))
    });
    if let Some((table, index)) = entry {
        guard.remove(table, index);
    }
    guard.tasks.remove_subtask(subtask);
}

/// Run the item the switch slot holds from inside the current frame,
/// as a nested start: the thread it starts or resumes, of a task of
/// `instance`, runs above a mark that names `subtask`, the caller's
/// record of the call, and `lower`, how the caller would go on once
/// control came back to it.
///
/// This is how a start intrinsic runs an `async`-typed callee, and
/// how `subtask.cancel` gives way to the callee it woke. The
/// documentation of [`Prepared::run_start`] states the rules.
pub fn run_nested_start<T: 'static>(
    store: &mut StoreContext<'_, T>,
    subtask: SubtaskId,
    instance: InstanceId,
    lower: LowerKind,
) -> Result<()> {
    // The mark comes back off through an unwind too. One a panic
    // left on the stack would turn every later deadlock under this
    // caller into a stack switch.
    //
    // The thread may suspend while it runs from here, whatever call
    // into its instance is in progress below, because its suspension
    // hands control back to this frame and the caller goes on
    // without a block. Its instance's may-not-suspend flag is clear
    // until the thread returns or suspends, and then goes back to
    // what it was, as Wasmtime's start intrinsic does.
    let tables = store.internal().tables_handle();
    let may_not_suspend = {
        let mut guard = lock(&tables)?;
        guard.tasks.begin_nested_start(subtask, lower);
        guard.tasks.set_may_not_suspend(instance, false)
    };
    let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        store.internal().run_switch_slot()
    }));
    if let (Some(old), Ok(mut guard)) = (may_not_suspend, tables.lock()) {
        guard.tasks.set_may_not_suspend(instance, old);
    }
    // A thread that switched to a thread this frame cannot resume
    // left that to the store, and the frame goes on once the store
    // has done it. The mark comes off then.
    if matches!(ran, Ok(Ok(()))) && store.internal().defers_work() {
        store
            .internal()
            .scheduler_mut()
            .deferred_mut()
            .ends_nested_start = true;
        return Ok(());
    }
    if let Ok(mut guard) = tables.lock() {
        guard.tasks.end_nested_start();
    }
    ran.unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

/// One `funcref` argument, which an adapter never passes as null.
pub fn funcref_argument(args: &[RuntimeVal], index: usize) -> Result<RuntimeFunc> {
    match args.get(index) {
        Some(RuntimeVal::FuncRef(Some(func))) => Ok(func.clone()),
        Some(RuntimeVal::FuncRef(None)) => Err(Error::internal(
            "an adapter started a call with a null function reference",
        )),
        _ => Err(Error::internal(
            "the start intrinsic expected a `funcref` argument",
        )),
    }
}

/// The callback the callee's lift named and the handle table of the
/// callee's instance, by the translator's per-instantiation index.
fn callee_callback(
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    instance_index: usize,
    callback: usize,
) -> Result<(RuntimeFunc, TableId)> {
    let state = abi_state
        .lock()
        .map_err(|_| Error::internal("ABI runtime state lock poisoned"))?;
    let function = state
        .callbacks
        .get(callback)
        .cloned()
        .flatten()
        .ok_or_else(|| Error::internal("a prepared call names a callback slot with no callback"))?;
    let table = state
        .handle_tables
        .get(instance_index)
        .copied()
        .ok_or_else(|| Error::internal("a prepared call names an instance with no handle table"))?;
    Ok((function, table))
}

/// The `post-return` the callee's lift named, by runtime slot.
pub fn post_return_at(abi_state: &Arc<Mutex<AbiRuntimeState>>, slot: usize) -> Result<RuntimeFunc> {
    abi_state
        .lock()
        .map_err(|_| Error::internal("ABI runtime state lock poisoned"))?
        .post_returns
        .get(slot)
        .cloned()
        .flatten()
        .ok_or_else(|| {
            Error::internal("a prepared call names a post-return slot with no post-return")
        })
}

/// The store's handle tables, or the internal error when a panic
/// poisoned their lock.
pub fn lock(tables: &Arc<Mutex<HandleTables>>) -> Result<std::sync::MutexGuard<'_, HandleTables>> {
    tables
        .lock()
        .map_err(|_| Error::internal("resource handle tables lock poisoned"))
}

/// The outstanding-borrows failure of a callee whose task ended
/// while the guest still owed a borrow.
fn outstanding_borrows(count: u32) -> Error {
    Error::from(AbiError {
        position: AbiPosition::Result,
        valtype: None,
        cause: AbiCause::OutstandingBorrows {
            count: count as usize,
        },
    })
}
