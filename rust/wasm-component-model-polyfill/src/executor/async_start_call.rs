//! The `async-start-call` intrinsic of a fused adapter.
//!
//! An asynchronous lower reaches this intrinsic right after the
//! prepare intrinsic of [`super::prepare_call`], whether the callee
//! is lifted synchronously or with a callback. It runs the call the
//! preparation set up and answers with the status word, so the
//! caller gets control back before the callee necessarily returns.
//!
//! The adapter passes the callee's core function as a `funcref`, the
//! number of flat parameters and of flat results that function has,
//! and one flag word that says whether the callee's lift is
//! asynchronous. What the intrinsic does with them:
//!
//! - It builds the item that starts the callee's implicit thread,
//!   which is the item of [`super::start_call`], places it in the
//!   scheduler's switch slot, and enters the thread through the
//!   entry gate. A callee the gate lets through keeps the slot and
//!   runs from inside this frame; a callee the gate holds leaves the
//!   slot empty and waits there in arrival order.
//! - It then reads the subtask and answers with the status word of
//!   the design. A call that resolved while the slot ran delivers
//!   its resolution, leaves the caller no entry, and answers
//!   `RETURNED` with no index. A call that did not resolve enters
//!   the caller's handle table, and the word is the subtask's state
//!   in its low four bits with that index above them: `STARTED` when
//!   the callee has read its parameters and parked or is still
//!   running, `STARTING` while the gate holds it.
//!
//! The subtask then delivers the rest of the call as events. A
//! callee the gate held later starts, and the start fills the
//! subtask's pending event because the caller holds an entry by
//! then; the callee's resolution fills it again. A callback export
//! that read `STARTED`, joined the subtask to one of its waitable
//! sets, and returned the wait word receives the event when its
//! sibling calls `task.return`, with the result already crossed into
//! its memory, because the crossing runs at the callee's
//! `task.return` and the event is delivered afterwards.
//!
//! A trap in the callee unwinds through the start item to this
//! trampoline and fails the caller's call, as it does for the
//! synchronous start. Once the trampoline has answered, a callee the
//! gate held is a task of its own: it starts in a later turn, and a
//! trap it raises fails that turn's driver rather than the call that
//! started it. That is Wasmtime's rule for a task that keeps running
//! after its call returned, and it is why the turn that opens the
//! gate ends there rather than running what it released: the call
//! whose callee the gate was holding gets its answer first.

use std::sync::{Arc, Mutex};

use wasm_runtime_layer::{Func as RuntimeFunc, Val as RuntimeVal};
use wasmtime_environ::component::START_FLAG_ASYNC_CALLEE;

use crate::abi::instance::BoundaryInstance;
use crate::abi::runtime_state::AbiRuntimeState;
use crate::concurrency::{CallStatus, LowerKind, SubtaskId};
use crate::error::{Error, Result};
use crate::executor::CallbackTask;
use crate::executor::intrinsics::core_func_type;
use crate::executor::ir::CoreSignature;
use crate::internal::ErrorInternal;
use crate::resource::{HandleTables, TableId};
use crate::store::StoreContext;
use crate::store::StoreContextInternalExt;

use super::prepare_call::u32_argument;
use super::start_call::{Prepared, callee_callback, funcref_argument, lock, post_return_at};

/// Build the `async-start-call` intrinsic of one fused adapter.
/// `callback` is the runtime callback slot of an asynchronously
/// lifted callee, and `post_return` the runtime post-return slot of
/// a synchronously lifted one. The adapter names at most one of the
/// two, and neither when the callee's lift declares neither.
pub fn build_async_start_call<T: 'static>(
    store: &mut StoreContext<'_, T>,
    callback: Option<usize>,
    post_return: Option<usize>,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> RuntimeFunc {
    RuntimeFunc::new(
        store.internal().runtime_mut(),
        core_func_type(signature),
        move |store_ctx, args, results| {
            let mut store = StoreContext::new(store_ctx);
            Ok(async_start_call(
                &mut store,
                callback,
                post_return,
                &abi_state,
                args,
                results,
            )?)
        },
    )
}

/// The body of one call of the intrinsic.
fn async_start_call<T: 'static>(
    store: &mut StoreContext<'_, T>,
    callback: Option<usize>,
    post_return: Option<usize>,
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    args: &[RuntimeVal],
    results: &mut [RuntimeVal],
) -> Result<()> {
    let callee_function = funcref_argument(args, 0)?;
    let param_count = u32_argument(args, 1)? as usize;
    let result_count = u32_argument(args, 2)? as usize;
    let async_lifted = u32_argument(args, 3)? & (START_FLAG_ASYNC_CALLEE as u32) != 0;

    let tables = store.internal().tables_handle();
    let prepared = Prepared::take(&tables)?;
    let subtask = prepared.subtask();
    let caller_table = caller_table(&tables, subtask)?;

    let loop_ = if async_lifted {
        Some(callback_loop(&tables, abi_state, &prepared, callback)?)
    } else {
        None
    };
    // A synchronously lifted callee's `post-return` is the one canon
    // option of the callee's lift that the preparation could not
    // record: the adapter names it here, with the start, rather than
    // there. An asynchronously lifted callee has none — its task
    // ends at the exit code of its callback loop, and the loop runs
    // the reference's `post-return` step itself.
    let post = match (async_lifted, post_return) {
        (false, Some(slot)) => Some((
            post_return_at(abi_state, slot)?,
            callee_boundary(&tables, abi_state, &prepared)?,
        )),
        _ => None,
    };

    let prepared = prepared.with_callee(callee_function, (param_count, result_count), loop_, post);
    let item = prepared.item()?;

    // The callee runs next: the item goes in the switch slot, the
    // gate decides whether it stays there, and the slot is run from
    // inside this frame. Both shapes the polyfill serves need the
    // exclusive thread of the callee's instance — the reference's
    // `not opts.async or opts.callback` — and a sync-typed callee
    // ignores the gate either way.
    if let Err(error) = prepared.run_start(store, item, LowerKind::Async) {
        prepared.remove(&tables);
        return Err(error);
    }

    let status = status_word(&tables, subtask, caller_table)?;
    let slot = results
        .first_mut()
        .ok_or_else(|| Error::internal("the async start intrinsic returns one status word"))?;
    *slot = RuntimeVal::I32(status.value() as i32);
    Ok(())
}

/// The status word the lower answers with.
///
/// A subtask that resolved before the lower returns delivers its
/// resolution here, which gives back every handle the caller lent
/// for the call, and its record leaves the store: the caller is
/// given no entry, so there is nothing for it to wait on or to drop.
/// A subtask that did not resolve enters the caller's handle table,
/// and the word carries the state it is in with that index.
fn status_word(
    tables: &Arc<Mutex<HandleTables>>,
    subtask: SubtaskId,
    caller_table: TableId,
) -> Result<CallStatus> {
    let mut guard = lock(tables)?;
    let state = guard
        .tasks
        .subtask(subtask)
        .map(|record| record.state)
        .ok_or_else(|| Error::internal("a started call has no subtask record"))?;
    if state.resolved() {
        guard.deliver_subtask_resolution(subtask)?;
        guard.tasks.remove_subtask(subtask);
        return Ok(CallStatus::returned());
    }
    let index = guard.insert_subtask(caller_table, subtask);
    Ok(CallStatus::in_progress(state, index))
}

/// The handle table of the caller's component instance, where the
/// subtask's entry goes. The preparation recorded it, because the
/// adapter names the caller only there.
fn caller_table(tables: &Arc<Mutex<HandleTables>>, subtask: SubtaskId) -> Result<TableId> {
    lock(tables)?
        .tasks
        .subtask(subtask)
        .and_then(|record| record.bridge.as_ref())
        .map(|bridge| bridge.caller_table)
        .ok_or_else(|| Error::internal("a prepared call carries no generated functions"))
}

/// The callback loop of an asynchronously lifted callee, with the
/// `async` option recorded on its task so that the callee's
/// `task.return` finds it set.
///
/// A lift that named no callback is the stackful form, which the
/// polyfill refuses. Translation refuses it where a component
/// declares it; the refusal is repeated here because the adapter
/// says which lift it is only at the call, and the two forms share
/// this intrinsic.
fn callback_loop(
    tables: &Arc<Mutex<HandleTables>>,
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    prepared: &Prepared,
    callback: Option<usize>,
) -> Result<CallbackTask> {
    let Some(callback) = callback else {
        return Err(Error::unsupported(
            "stackful asynchronous lifts (`canon lift async` without a callback)",
        ));
    };
    let (function, table) = callee_callback(abi_state, prepared.instance_index(), callback)?;
    if let Some(options) = lock(tables)?
        .tasks
        .task_mut(prepared.task())
        .and_then(|record| record.options.as_mut().map(Arc::make_mut))
    {
        options.async_ = true;
        options.callback = Some(callback);
    }
    Ok(CallbackTask::new(
        prepared.task(),
        prepared.instance(),
        table,
        function,
    ))
}

/// The callee's component instance as a crossing names it, which is
/// what holds its may-leave flag clear around its `post-return`. The
/// options come off the callee's task, where the preparation put
/// them.
fn callee_boundary(
    tables: &Arc<Mutex<HandleTables>>,
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    prepared: &Prepared,
) -> Result<BoundaryInstance> {
    let options = lock(tables)?
        .tasks
        .task(prepared.task())
        .and_then(|record| record.options.clone())
        .ok_or_else(|| Error::internal("a prepared call's callee task carries no options"))?;
    let (_options, instance) = BoundaryInstance::resolve(&options, abi_state, tables)?;
    Ok(instance)
}
