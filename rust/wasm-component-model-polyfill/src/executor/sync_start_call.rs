//! The `sync-start-call` intrinsic of a fused adapter.
//!
//! A synchronous lower of an asynchronously lifted export reaches
//! this intrinsic right after the prepare intrinsic of
//! [`super::prepare_call`]. It runs the call the preparation set up
//! and returns the caller's flat results, so the caller sees the
//! call as an ordinary one that returned.
//!
//! The adapter passes the callee's core function as a `funcref` and
//! the number of flat parameters that function takes. What the
//! intrinsic does with them:
//!
//! - It builds the item that starts the callee's implicit thread.
//!   The item calls the start function with the caller's flat
//!   arguments, which lifts them in the caller and lowers them into
//!   the callee; marks the subtask started; calls the callee's core
//!   function; and hands the status word it returned to the callback
//!   loop of [`crate::executor::CallbackTask`].
//! - It places that item in the scheduler's switch slot and enters
//!   the callee's implicit thread through the entry gate. A callee
//!   the gate holds leaves the slot empty and waits there in arrival
//!   order; a callee the gate lets through keeps the slot, and the
//!   intrinsic runs it at once. That is the reference resuming the
//!   callee's thread before the lower returns.
//! - It then blocks on the subtask's resolution through the suspend
//!   seam. A callee that parked — a callback export that returned
//!   the yield or the wait word — leaves its caller free to give
//!   way to the rest of the store; a callee that resolved as it ran
//!   does not block the caller at all. A caller that must not block
//!   fails with the cannot-block cause, and only after the callee
//!   did not resolve at once, which is the lazy rule of the
//!   reference and of Wasmtime 49.
//! - It delivers the resolution, which releases every handle the
//!   caller lent for the call, and returns the flat results the
//!   return function produced.
//!
//! A trap in the callee, or a failure of either generated function,
//! unwinds through the start item into the slot the item leaves it
//! in, and the intrinsic fails the caller's call with it. That is
//! the message the synchronous baseline gives the same trap.

use std::sync::{Arc, Mutex};

use wasm_runtime_layer::{Func as RuntimeFunc, Val as RuntimeVal};

use crate::abi::layout::FlatType;
use crate::abi::runtime_state::AbiRuntimeState;
use crate::concurrency::{InstanceId, Item, ItemKind, SubtaskId, SuspendSeam, TaskId};
use crate::error::{Error, InstantiationError, Result};
use crate::executor::intrinsics::core_func_type;
use crate::executor::ir::CoreSignature;
use crate::executor::{CallbackTask, status_word};
use crate::resource::{HandleTables, TableId};
use crate::store::StoreContext;

use super::prepare_call::{take_prepared_call, u32_argument};

/// Where the start item leaves the failure that belongs to the
/// caller: the lowering of the arguments, a trap in the callee's
/// core function, or the status word that function returned. The
/// item outlives the trampoline's frame when the gate holds it, so
/// both sides hold the slot.
type StartFailure = Arc<Mutex<Option<Error>>>;

/// Build the `sync-start-call` intrinsic of one fused adapter.
/// `callback` is the runtime callback slot of the callee's lift.
pub fn build_sync_start_call<T: 'static>(
    store: &mut StoreContext<'_, T>,
    callback: usize,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> RuntimeFunc {
    // The caller's flat results are this intrinsic's own results,
    // and they are the return function's too. The types travel with
    // the call, because the function is named by reference and
    // nothing else says what it produces.
    let caller_results = signature.results.clone();
    RuntimeFunc::new(
        store.runtime_mut(),
        core_func_type(signature),
        move |store_ctx, args, results| {
            let mut store = StoreContext::new(store_ctx);
            Ok(sync_start_call(
                &mut store,
                callback,
                &caller_results,
                &abi_state,
                args,
                results,
            )?)
        },
    )
}

/// The body of one call of the intrinsic.
fn sync_start_call<T: 'static>(
    store: &mut StoreContext<'_, T>,
    callback: usize,
    caller_results: &[FlatType],
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    args: &[RuntimeVal],
    results: &mut [RuntimeVal],
) -> Result<()> {
    let callee_function = funcref_argument(args, 0)?;
    let param_count = u32_argument(args, 1)? as usize;

    let tables = store.tables_handle();
    let subtask = take_prepared_call(&tables)?;
    let prepared = Prepared::read(&tables, subtask)?;

    // The callee's lift is asynchronous with a callback: a
    // synchronous start reaches no other shape, because the
    // stackful form is refused at translation and a synchronously
    // lifted callee takes the enter and exit intrinsics instead.
    // The task learns that here, where the adapter says it, so that
    // the callee's `task.return` finds the `async` option set.
    let loop_ = {
        let (function, table) = callee_callback(abi_state, prepared.instance_index, callback)?;
        let mut guard = lock(&tables)?;
        if let Some(options) = guard
            .tasks
            .task_mut(prepared.task)
            .and_then(|record| record.options.as_mut())
        {
            options.async_ = true;
            options.callback = Some(callback);
        }
        if let Some(bridge) = guard
            .tasks
            .subtask_mut(subtask)
            .and_then(|record| record.bridge.as_mut())
        {
            bridge.caller_results = caller_results.to_vec();
        }
        drop(guard);
        CallbackTask::new(prepared.task, prepared.instance, table, function)
    };

    let failure: StartFailure = Arc::new(Mutex::new(None));
    let item = start_item(
        subtask,
        prepared.task,
        callee_function,
        param_count,
        loop_,
        failure.clone(),
    );

    // The callee runs next: the item goes in the switch slot, the
    // gate decides whether it stays there, and the slot is run from
    // inside this frame.
    store.scheduler_mut().switch_to(item);
    store.start_switched_export_thread(
        prepared.task,
        prepared.instance,
        prepared.callee_async_typed,
        true,
    )?;
    store.run_switch_slot()?;

    // The caller waits for the callee's result. A call that resolved
    // while the slot ran does not wait at all, which is what makes
    // the cannot-block failure of a sync-typed caller lazy.
    let watched = tables.clone();
    let blocked = SuspendSeam::suspend(store, |_store| settled(&watched, subtask, &failure));
    if let Some(error) = failure.lock().ok().and_then(|mut slot| slot.take()) {
        remove_subtask(&tables, subtask);
        return Err(error);
    }
    blocked?;

    // The resolution is delivered as the lower returns, which gives
    // back every handle the caller lent for the call. The subtask
    // record then leaves the store: a call that resolves before the
    // lower returns leaves the caller no entry to wait on.
    let flat_results = {
        let mut guard = lock(&tables)?;
        guard.deliver_subtask_resolution(subtask)?;
        let results = guard
            .tasks
            .subtask_mut(subtask)
            .and_then(|record| record.bridge.as_mut())
            .map(|bridge| std::mem::take(&mut bridge.flat_results))
            .unwrap_or_default();
        guard.tasks.remove_subtask(subtask);
        results
    };
    if flat_results.len() != results.len() {
        return Err(Error::internal(format!(
            "the return function of a prepared call produced {} flat results for a caller with {}",
            flat_results.len(),
            results.len()
        )));
    }
    for (slot, value) in results.iter_mut().zip(flat_results) {
        *slot = value;
    }
    Ok(())
}

/// What the start intrinsic reads off the prepared call.
struct Prepared {
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
}

impl Prepared {
    fn read(tables: &Arc<Mutex<HandleTables>>, subtask: SubtaskId) -> Result<Self> {
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
            task,
            instance,
            instance_index,
            callee_async_typed,
        })
    }
}

/// The item that starts the callee's implicit thread.
///
/// It runs once, whether from the switch slot inside the caller's
/// trampoline or from a later turn when the gate held it. What it
/// fails with belongs to the caller, so it leaves it in `failure`
/// rather than failing the turn that ran it.
fn start_item<T: 'static>(
    subtask: SubtaskId,
    task: TaskId,
    callee: RuntimeFunc,
    param_count: usize,
    loop_: CallbackTask,
    failure: StartFailure,
) -> Item<T> {
    Item::new(
        ItemKind::TaskStart,
        move |store: &mut StoreContext<'_, T>| {
            let started = start_call(store, subtask, task, &callee, param_count, &loop_);
            if let Err(error) = started {
                abandon(store, subtask);
                if let Ok(mut slot) = failure.lock() {
                    *slot = Some(error);
                }
            }
            Ok(())
        },
    )
}

/// Run the callee: lower the arguments through the start function,
/// call the core function, and act on the status word it returned.
fn start_call<T: 'static>(
    store: &mut StoreContext<'_, T>,
    subtask: SubtaskId,
    task: TaskId,
    callee: &RuntimeFunc,
    param_count: usize,
    loop_: &CallbackTask,
) -> Result<()> {
    // The callee's task is the current scope for the whole of the
    // start: the arguments the start function lowers are the
    // callee's, and a borrow the adapter transfers in is owed to it.
    store.enter_export_task(task)?;
    let core_arguments = match call_start_function(store, subtask, param_count) {
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

    let mut core_results = [RuntimeVal::I32(0)];
    let called = callee
        .call(store.runtime_mut(), &core_arguments, &mut core_results)
        .map_err(|err| Error::from(InstantiationError::SubstrateFailure(err)));
    match called {
        Ok(()) => {
            store.leave_export_task(task)?;
            loop_.handle_status_word(store, status_word(&core_results)?)
        }
        Err(error) => {
            store.abandon_export_task(task)?;
            Err(error)
        }
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

/// Whether the call has settled: the callee resolved, or the start
/// left a failure behind.
fn settled(tables: &Arc<Mutex<HandleTables>>, subtask: SubtaskId, failure: &StartFailure) -> bool {
    if failure.lock().map(|slot| slot.is_some()).unwrap_or(false) {
        return true;
    }
    tables
        .lock()
        .ok()
        .and_then(|guard| {
            guard
                .tasks
                .subtask(subtask)
                .map(|record| record.state.resolved())
        })
        .unwrap_or(true)
}

/// End a prepared call whose start failed: the subtask's resolution
/// is a cancellation, and the handles the caller lent for the call
/// are given back with it.
fn abandon<T: 'static>(store: &mut StoreContext<'_, T>, subtask: SubtaskId) {
    let tables = store.tables_handle();
    let Ok(mut guard) = tables.lock() else {
        return;
    };
    let _ = guard.tasks.subtask_cancelled(subtask);
    let _ = guard.deliver_subtask_resolution(subtask);
}

/// Remove the subtask record of a call that failed, once the
/// trampoline has taken its failure.
fn remove_subtask(tables: &Arc<Mutex<HandleTables>>, subtask: SubtaskId) {
    if let Ok(mut guard) = tables.lock() {
        guard.tasks.remove_subtask(subtask);
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

/// One `funcref` argument, which an adapter never passes as null.
fn funcref_argument(args: &[RuntimeVal], index: usize) -> Result<RuntimeFunc> {
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

fn lock(tables: &Arc<Mutex<HandleTables>>) -> Result<std::sync::MutexGuard<'_, HandleTables>> {
    tables
        .lock()
        .map_err(|_| Error::internal("resource handle tables lock poisoned"))
}
