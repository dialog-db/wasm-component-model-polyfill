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
//! - It builds the item that starts the callee's implicit thread,
//!   which is the item of [`super::start_call`]: it lifts and lowers
//!   the arguments, marks the subtask started, calls the callee's
//!   core function, and hands the status word it returned to the
//!   callback loop of [`crate::executor::CallbackTask`].
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
//!
//! A wait that fails is the caller's failure rather than the
//! callee's, and it gives back what the trap path gives back: the
//! subtask resolves as a cancellation and leaves the store, and the
//! callee's task ends, so the instance its thread held exclusively
//! goes back and the next call finds the store as this one did.
//!
//! A callee this reaches has usually left an item behind. It is the
//! one caller of the callback loop that can fail while its callee
//! is parked: a callee that gave way left a callback item on the
//! low-priority queue, one that waited left a held callback item,
//! and one the gate never let through left the start item at the
//! gate. Every one of them names the callee's task, and the store's
//! rule for a dead task's pending work is that it goes with the
//! record: ending the task drops them. No item is left to run the
//! callee's callback for a task the store no longer holds, which is
//! what would otherwise reach the polyfill's own invariant cause
//! from a component that did nothing wrong.

use std::sync::{Arc, Mutex};

use wasm_runtime_layer::{Func as RuntimeFunc, Val as RuntimeVal};

use crate::abi::layout::FlatType;
use crate::abi::runtime_state::AbiRuntimeState;
use crate::concurrency::{LowerKind, Readiness, SubtaskId, SuspendSeam, TaskId};
use crate::error::{Error, Result};
use crate::executor::CallbackTask;
use crate::executor::intrinsics::core_func_type;
use crate::executor::ir::CoreSignature;
use crate::internal::ErrorInternal;
use crate::store::StoreContext;
use crate::store::StoreContextInternalExt;

use super::prepare_call::u32_argument;
use super::start_call::{Prepared, callee_callback, funcref_argument, lock, release_subtask};
use super::start_failure::StartFailure;

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
        store.internal().runtime_mut(),
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

    let tables = store.internal().tables_handle();
    let prepared = Prepared::take(&tables)?;
    let subtask = prepared.subtask();

    // The callee's lift is asynchronous with a callback: a
    // synchronous start reaches no other shape, because the
    // stackful form is refused at translation and a synchronously
    // lifted callee takes the enter and exit intrinsics instead.
    // The task learns that here, where the adapter says it, so that
    // the callee's `task.return` finds the `async` option set.
    let loop_ = {
        let (function, table) = callee_callback(abi_state, prepared.instance_index(), callback)?;
        let mut guard = lock(&tables)?;
        if let Some(options) = guard
            .tasks
            .task_mut(prepared.task())
            .and_then(|record| record.options.as_mut().map(Arc::make_mut))
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
        CallbackTask::new(prepared.task(), prepared.instance(), table, function)
    };

    let failure: StartFailure = Arc::new(Mutex::new(None));
    // The status word of an asynchronously lifted callee is its one
    // flat result, which the callback loop reads.
    let prepared = prepared.with_callee(callee_function, (param_count, 1), Some(loop_), None);
    let item = prepared.item_reporting_to(failure.clone())?;

    // The callee runs next: the item goes in the switch slot, the
    // gate decides whether it stays there, and the slot is run from
    // inside this frame.
    prepared.run_start(store, item, LowerKind::Sync)?;

    // The caller waits for the callee's result: its readiness
    // condition is the resolution of the call's subtask. A start
    // that fails resolves the subtask as a cancellation before it
    // leaves its failure in the slot, so the condition covers that
    // too. A call that resolved while the slot ran does not wait at
    // all, which is what makes the cannot-block failure of a
    // sync-typed caller lazy.
    let blocked = SuspendSeam::wait_until(store, Readiness::Subtask { subtask });
    if let Some(error) = failure.lock().ok().and_then(|mut slot| slot.take()) {
        prepared.remove(&tables);
        return Err(error);
    }
    if let Err(error) = blocked {
        release_wait(store, subtask, prepared.task());
        return Err(error);
    }

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

/// Give back what a call whose wait failed still holds.
///
/// The wait fails with a scheduler cause — the caller must not
/// block, the store is idle, or the target has no stack switch to
/// serve the suspension — and each of them says the callee will
/// never resolve for this caller. The store is therefore left as it
/// was before the call, which is what the trap path leaves it as,
/// reached the other way: the subtask resolves as a cancellation,
/// which gives back every handle the caller lent, its record goes,
/// and the callee's task ends, which gives back the instance that
/// task's implicit thread held exclusively.
///
/// The task ends rather than being abandoned because its scope is
/// already off the stack by the time a wait can fail: the callee
/// either parked between events or never ran at all, and an
/// abandon finds no scope to unwind and leaves the record where it
/// is. The borrows the callee did not drop go with the record, for
/// the reason a callback task's own failure path gives: the call
/// has already failed, so there is nothing left to report them to.
///
/// The item the callee parked with goes with the record too. The
/// module documentation says which item each way of parking leaves,
/// and the scheduler's says what dropping one gives back.
fn release_wait<T: 'static>(store: &mut StoreContext<'_, T>, subtask: SubtaskId, task: TaskId) {
    release_subtask(store, subtask);
    let _ = store.internal().end_export_task(task);
}
