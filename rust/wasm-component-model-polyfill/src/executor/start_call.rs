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
//! - An asynchronously lifted callee returns a status word, which
//!   goes to the callback loop of [`crate::executor::CallbackTask`].
//!   A synchronously lifted one returns its flat results, which
//!   cross into the caller through the return function at that
//!   moment; its `post-return` runs afterwards, inside its own task,
//!   and the task then ends.
//!
//! A trap in the callee, or a failure of either generated function,
//! ends the call: the subtask's resolution is a cancellation, which
//! gives back every handle the caller lent, and the failure travels
//! to whichever of the two intrinsics is waiting for it.

use std::sync::{Arc, Mutex};

use wasm_runtime_layer::{Func as RuntimeFunc, Val as RuntimeVal};

use crate::abi::boundary_call::BoundaryCall;
use crate::abi::instance::BoundaryInstance;
use crate::abi::runtime_state::AbiRuntimeState;
use crate::concurrency::{InstanceId, Item, ItemKind, SubtaskId, TaskId};
use crate::error::{AbiCause, AbiError, AbiPosition, Error, InstantiationError, Result};
use crate::executor::{CallbackTask, status_word};
use crate::resource::{HandleTables, TableId};
use crate::store::StoreContext;

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
    /// How many flat results it returns. An asynchronously lifted
    /// callee returns the status word, which the adapter counts as
    /// one.
    result_count: usize,
    /// The callback loop of an asynchronously lifted callee, which
    /// the status word its core function returned is handed to.
    /// `None` for a synchronously lifted callee, whose results cross
    /// into the caller as its core function returns.
    loop_: Option<CallbackTask>,
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
    /// callback loop of an asynchronously lifted callee, and the
    /// `post-return` of a synchronously lifted one.
    pub fn with_callee(
        mut self,
        function: RuntimeFunc,
        counts: (usize, usize),
        loop_: Option<CallbackTask>,
        post_return: Option<(RuntimeFunc, BoundaryInstance)>,
    ) -> Self {
        let (param_count, result_count) = counts;
        self.callee = Some(Callee {
            function,
            param_count,
            result_count,
            loop_,
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

    /// The callee's component instance.
    pub fn instance(&self) -> InstanceId {
        self.instance
    }

    /// The callee's component instance by the translator's
    /// per-instantiation index, which is what the ABI state is keyed
    /// on.
    pub fn instance_index(&self) -> usize {
        self.instance_index
    }

    /// Whether the callee's function type carries the `async`
    /// effect, which decides whether its task waits at the gate.
    pub fn callee_async_typed(&self) -> bool {
        self.callee_async_typed
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

    /// Whether the call has settled: the callee resolved, or the
    /// start left a failure behind.
    pub fn settled(&self, tables: &Arc<Mutex<HandleTables>>, failure: &StartFailure) -> bool {
        if failure.lock().map(|slot| slot.is_some()).unwrap_or(false) {
            return true;
        }
        self.resolved(tables).unwrap_or(true)
    }

    /// Whether the call resolved, or `None` when its record is gone.
    pub fn resolved(&self, tables: &Arc<Mutex<HandleTables>>) -> Option<bool> {
        tables.lock().ok().and_then(|guard| {
            guard
                .tasks
                .subtask(self.subtask)
                .map(|record| record.state.resolved())
        })
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
        Ok(Item::new(
            ItemKind::TaskStart,
            move |store: &mut StoreContext<'_, T>| {
                let started = start_call(store, subtask, task, &callee);
                let Err(error) = started else {
                    return Ok(());
                };
                abandon(store, subtask);
                let Some(failure) = failure else {
                    return Err(error);
                };
                if let Ok(mut slot) = failure.lock() {
                    *slot = Some(error);
                }
                Ok(())
            },
        )
        .for_task(task))
    }
}

/// Run the callee: lower the arguments through the start function,
/// call the core function, and act on what it produced.
fn start_call<T: 'static>(
    store: &mut StoreContext<'_, T>,
    subtask: SubtaskId,
    task: TaskId,
    callee: &Callee,
) -> Result<()> {
    // The callee's task is the current scope for the whole of the
    // start: the arguments the start function lowers are the
    // callee's, and a borrow the adapter transfers in is owed to it.
    store.enter_export_task(task)?;
    let core_arguments = match call_start_function(store, subtask, callee.param_count) {
        Ok(arguments) => arguments,
        Err(error) => {
            store.abandon_export_task(task)?;
            return Err(error);
        }
    };
    {
        let tables = store.tables_handle();
        lock(&tables)?.tasks.start_subtask(subtask);
    }
    store.start_export_task(task)?;

    // The callee's flat result types are the adapter's own, and the
    // adapter names only how many there are. A status word is an
    // `i32`; a synchronously lifted callee's one flat result can be
    // of any type, so its slot is filled with the widest flat value,
    // which every backend overwrites with the value and the type the
    // core function returned.
    let placeholder = match callee.loop_ {
        Some(_) => RuntimeVal::I32(0),
        None => RuntimeVal::F64(0.0),
    };
    let mut core_results = vec![placeholder; callee.result_count];
    let called = callee
        .function
        .call(store.runtime_mut(), &core_arguments, &mut core_results)
        .map_err(|err| Error::from(InstantiationError::SubstrateFailure(err)));
    let Ok(()) = called else {
        store.abandon_export_task(task)?;
        return called;
    };
    match &callee.loop_ {
        Some(loop_) => {
            store.leave_export_task(task)?;
            loop_.handle_status_word(store, status_word(&core_results)?)
        }
        None => match resolve_sync_lift(store, subtask, task, callee, &core_results) {
            Ok(()) => Ok(()),
            Err(error) => {
                store.abandon_export_task(task)?;
                Err(error)
            }
        },
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
    let tables = store.tables_handle();
    cross_result_into_caller(
        store.runtime_mut(),
        &tables,
        task,
        subtask,
        None,
        core_results,
    )?;
    if let Some((post_return, boundary)) = &callee.post_return {
        let _call = BoundaryCall::post_return(boundary)?;
        let mut empty: [RuntimeVal; 0] = [];
        post_return
            .call(store.runtime_mut(), core_results, &mut empty)
            .map_err(|err| Error::from(InstantiationError::SubstrateFailure(err)))?;
    }
    match store.exit_export_task(task)? {
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
    let tables = store.tables_handle();
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
        .call(store.runtime_mut(), &arguments, &mut results)
        .map_err(|err| Error::from(InstantiationError::SubstrateFailure(err)))?;
    Ok(results)
}

/// End a prepared call whose start failed: the subtask's resolution
/// is a cancellation, and the handles the caller lent for the call
/// are given back with it.
pub fn abandon<T: 'static>(store: &mut StoreContext<'_, T>, subtask: SubtaskId) {
    let tables = store.tables_handle();
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
/// A prepared call's subtask is never a scope of its own — the
/// caller's task holds the stack while the callee runs, and an
/// asynchronous lower hands the record back to the caller to wait on
/// — so the scope stack has nothing to unwind for it and
/// `HandleTables::abandon_subtask` would find nothing to do. This is
/// what abandoning one means instead.
///
/// The entry goes with the record because the two are the caller's
/// one handle on the call: a record removed while an entry still
/// named it would leave the caller an index that resolves to
/// nothing. A call that failed before the lower returned was never
/// given an entry, and then there is only the record to remove.
///
/// The cancellation is recorded even for a subtask that had already
/// returned, where cancelling is not the state the resolution would
/// otherwise reach. Nothing reads the difference: the record and the
/// caller's entry for it leave the store in the same breath, so the
/// state it was moved to has no one left to observe it.
pub fn release_subtask<T: 'static>(store: &mut StoreContext<'_, T>, subtask: SubtaskId) {
    abandon(store, subtask);
    let tables = store.tables_handle();
    let Ok(mut guard) = tables.lock() else {
        return;
    };
    let entry = guard.tasks.subtask(subtask).and_then(|record| {
        let table = record.bridge.as_ref()?.caller_table;
        Some((table, record.handle?))
    });
    if let Some((table, index)) = entry {
        guard.remove(table, index);
    }
    guard.tasks.remove_subtask(subtask);
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
pub fn callee_callback(
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
