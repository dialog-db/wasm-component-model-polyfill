// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The `start-call` intrinsic of a fused adapter.
//!
//! An adapter reaches this intrinsic right after the prepare intrinsic
//! of [`super::prepare_call`] whenever the call is not sync on every
//! side: the lower is async, the lift is async, or the function type
//! carries the `async` effect. Only an adapter that is sync on all
//! three calls the callee itself. The intrinsic runs the call that the
//! preparation set up.
//!
//! The adapter passes four values: the callee's core function as a
//! `funcref`, the number of flat parameters and of flat results that
//! function has, and a flag word. The flags say two things, and the
//! intrinsic reads them each time it runs, as Wasmtime does:
//!
//! - `START_FLAG_ASYNC_CALLER`: the caller lowered the import with
//!   `async`. The intrinsic answers with the status word. Without the
//!   flag, the caller is sync, and the intrinsic blocks until the call
//!   resolves and then returns the caller's flat results.
//! - `START_FLAG_ASYNC_CALLEE`: the callee lifted the export with
//!   `async`, with a callback or stackful. Without the flag, the
//!   callee's lift is sync, and its results cross into the caller as
//!   its core function returns. Its `post-return` runs after that.
//!
//! What the intrinsic does for every call:
//!
//! - It builds the item that starts the callee's implicit thread,
//!   which is the item of [`super::start_call`], places it in the
//!   scheduler's switch slot, and enters the thread through the entry
//!   gate. A callee that the gate lets through keeps the slot and runs
//!   from inside this frame. A callee that the gate holds leaves the
//!   slot empty and waits there in arrival order. That is the
//!   reference resuming the callee's thread before the lower returns.
//!   The gate applies to every call that reaches this intrinsic, so a
//!   sync caller of an async-typed callee waits for backpressure and
//!   the exclusive lock too, as the reference's
//!   `Task.enter_implicit_thread` says.
//!
//! What an async caller gets:
//!
//! - The status word of the design. A call that resolved while the
//!   slot ran delivers its resolution, leaves the caller no entry, and
//!   answers `RETURNED` with no index. A call that did not resolve
//!   enters the caller's handle table, and the word is the subtask's
//!   state in its low four bits with that index above them: `STARTED`
//!   when the callee has read its parameters, `STARTING` while the
//!   gate holds it.
//! - The rest of the call as events. A callee that the gate held
//!   starts later, and its start fills the subtask's pending event,
//!   because the caller holds an entry by then. The callee's
//!   resolution fills it again.
//! - Once the intrinsic has answered, a callee that the gate held is a
//!   task of its own. It starts in a later turn, and a trap it raises
//!   fails that turn's driver, not the call that started it. That is
//!   Wasmtime's rule for a task that keeps running after its call
//!   returned. It is also why the turn that opens the gate ends there:
//!   the call whose callee the gate was holding gets its answer first.
//!
//! What a sync caller gets:
//!
//! - It blocks on the subtask's resolution through the suspend seam.
//!   So the intrinsic is a blocking built-in, and it reaches the guest
//!   as the switch module's shim under a provider. A callee that
//!   parked leaves its caller free to give way to the rest of the
//!   store. A callee that resolved as it ran does not block the caller
//!   at all. A caller that must not block fails with the cannot-block
//!   cause, and only after the callee did not resolve at once. That is
//!   the lazy rule of the reference and of Wasmtime.
//! - Once the call resolves, the intrinsic delivers the resolution,
//!   which gives back every handle the caller lent for the call, and
//!   returns the flat results that the return function produced.
//!
//! A trap in the callee, or a failure of either generated function,
//! unwinds through the start item. An async caller's start fails at
//! once. A sync caller's item leaves the failure in a slot, and the
//! intrinsic fails the call with it, which is the message that the
//! synchronous baseline gives the same trap.
//!
//! A sync caller's wait that fails is the caller's failure, not the
//! callee's. It gives back what the trap path gives back: the subtask
//! resolves as a cancellation and leaves the store, and the callee's
//! task ends, so the instance that its thread held exclusively goes
//! back. A callee that this reaches has usually left an item behind: a
//! callback item on the low-priority queue, a held callback item, or
//! the start item at the gate. Each of them names the callee's task,
//! and the store's rule for the pending work of a dead task is that it
//! goes with the record. Ending the task drops them, so no item is
//! left to run the callee's callback for a task the store no longer
//! holds.

use std::sync::{Arc, Mutex};

use wasmtime_environ::component::{START_FLAG_ASYNC_CALLEE, START_FLAG_ASYNC_CALLER};

use crate::abi::instance::BoundaryInstance;
use crate::abi::layout::FlatType;
use crate::abi::runtime_state::AbiRuntimeState;
use crate::concurrency::{
    BlockStep, BlockingBuiltin, CallStatus, LowerKind, Readiness, SubtaskId, TaskId,
};
use crate::error::{Error, Result};
use crate::executor::intrinsics::core_func_type;
use crate::executor::ir::CoreSignature;
use crate::internal::ErrorInternal;
use crate::resource::{HandleTables, TableId};
use crate::runtime_layer::Val as RuntimeVal;
use crate::store::StoreContext;
use crate::store::StoreContextInternalExt;

use super::prepare_call::u32_argument;
use super::start_call::{Prepared, funcref_argument, lock, post_return_at, release_subtask};
use super::start_failure::StartFailure;

/// Build the `start-call` intrinsic of one fused adapter.
///
/// `callback` is the runtime callback slot of an async-lifted callee,
/// and `post_return` the runtime post-return slot of a sync-lifted
/// one. The adapter names whichever of the two the callee's lift
/// declares. `signature` is the intrinsic's own core signature: its
/// results are the status word for an async caller and the caller's
/// flat results for a sync one.
///
/// The intrinsic is a blocking built-in for both callers. A sync
/// caller waits on the call's resolution. An async caller never waits
/// on a condition, but a callee that it starts can switch to a
/// suspended thread before it first suspends. Under the
/// host-suspension provider the intrinsic cannot resume that thread
/// from inside the caller's call. It then leaves the rest of the start
/// to the scheduler as a plan, the caller's thread suspends in the
/// intrinsic's shim, and the status word is read once the scheduler
/// resumes it.
pub fn build_start_call<T: 'static>(
    _store: &mut StoreContext<'_, T>,
    callback: Option<usize>,
    post_return: Option<usize>,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> BlockingBuiltin<T> {
    // A sync caller's flat results are this intrinsic's own results,
    // and they are the return function's too. The types travel with
    // the call, because the function is named by reference and
    // nothing else says what it produces.
    let caller_results = signature.results.clone();
    BlockingBuiltin::new(
        core_func_type(signature),
        move |store: &mut StoreContext<'_, T>, args: &[RuntimeVal]| {
            let call = StartCall {
                callback,
                post_return,
                caller_results: &caller_results,
                abi_state: &abi_state,
            };
            Ok(call.begin(store, args)?)
        },
    )
}

/// What one fused adapter's intrinsic knows before it is called.
struct StartCall<'a> {
    callback: Option<usize>,
    post_return: Option<usize>,
    caller_results: &'a [FlatType],
    abi_state: &'a Arc<Mutex<AbiRuntimeState>>,
}

impl StartCall<'_> {
    /// The first part of one call of the intrinsic: name the callee,
    /// start it, and answer or wait as the caller's lower says.
    fn begin<T: 'static>(
        &self,
        store: &mut StoreContext<'_, T>,
        args: &[RuntimeVal],
    ) -> Result<BlockStep<T>> {
        let callee_function = funcref_argument(args, 0)?;
        let param_count = u32_argument(args, 1)? as usize;
        let result_count = u32_argument(args, 2)? as usize;
        let flags = u32_argument(args, 3)?;
        let async_caller = flags & (START_FLAG_ASYNC_CALLER as u32) != 0;
        let async_lifted = flags & (START_FLAG_ASYNC_CALLEE as u32) != 0;

        let tables = store.internal().tables_handle();
        let prepared = Prepared::take(&tables)?;

        // The task learns the form of the callee's lift here, where the
        // adapter says it, so that the callee's `task.return` finds the
        // `async` option set.
        let lift = if async_lifted {
            Some(prepared.async_lift(&tables, self.abi_state, self.callback)?)
        } else {
            None
        };
        // A sync-lifted callee's `post-return` is the one canon option
        // of its lift that the preparation could not record: the
        // adapter names it here, with the start. An async-lifted
        // callee has none: its task ends at the exit code of its
        // callback loop, or when its stackful core function returns,
        // and the reference calls a `post-return` only on the sync
        // path.
        let post = match (async_lifted, self.post_return) {
            (false, Some(slot)) => Some((
                post_return_at(self.abi_state, slot)?,
                callee_boundary(&tables, self.abi_state, &prepared)?,
            )),
            _ => None,
        };
        let prepared =
            prepared.with_callee(callee_function, (param_count, result_count), lift, post);

        if async_caller {
            begin_async_caller(store, prepared)
        } else {
            self.begin_sync_caller(store, prepared)
        }
    }

    /// Start the callee for a sync caller, then wait for the call to
    /// resolve.
    fn begin_sync_caller<T: 'static>(
        &self,
        store: &mut StoreContext<'_, T>,
        prepared: Prepared,
    ) -> Result<BlockStep<T>> {
        let tables = store.internal().tables_handle();
        let subtask = prepared.subtask();
        if let Some(bridge) = lock(&tables)?
            .tasks
            .subtask_mut(subtask)
            .and_then(|record| record.bridge.as_mut())
        {
            bridge.caller_results = self.caller_results.to_vec();
        }

        let failure: StartFailure = Arc::new(Mutex::new(None));
        let item = prepared.item_reporting_to(failure.clone())?;

        // The callee runs next: the item goes in the switch slot, the
        // gate decides whether it stays there, and the slot is run from
        // inside this frame. Under a provider the callee runs on a
        // stack of its own, and the start returns here once it
        // suspends or finishes.
        prepared.run_start(store, item, LowerKind::Sync)?;

        // The caller waits for the callee's result: its readiness
        // condition is the resolution of the call's subtask. A start
        // that fails resolves the subtask as a cancellation before it
        // leaves its failure in the slot, so the condition covers that
        // too. A call that resolved while the slot ran does not wait at
        // all, which is what makes the cannot-block failure of a
        // sync-typed caller lazy.
        let slots = self.caller_results.len();
        Ok(BlockStep::wait(
            Readiness::Subtask { subtask },
            move |store: &mut StoreContext<'_, T>, waited| {
                Ok(finish_sync_caller(
                    store, &prepared, &failure, waited, slots,
                )?)
            },
        ))
    }
}

/// Start the callee for an async caller, and answer with the status
/// word, at once or once the work that the start left to the store is
/// done.
fn begin_async_caller<T: 'static>(
    store: &mut StoreContext<'_, T>,
    prepared: Prepared,
) -> Result<BlockStep<T>> {
    let tables = store.internal().tables_handle();
    let subtask = prepared.subtask();
    let caller_table = caller_table(&tables, subtask)?;
    let item = prepared.item()?;

    // The callee runs next: the item goes in the switch slot, the
    // gate decides whether it stays there, and the slot is run from
    // inside this frame.
    if let Err(error) = prepared.run_start(store, item, LowerKind::Async) {
        prepared.remove(&tables);
        return Err(error);
    }

    // A start that left work to the store answers once that work is
    // done: the callee may resolve in it, which the word says.
    if store.internal().defers_work() {
        return Ok(BlockStep::wait(
            Readiness::Planned,
            move |store: &mut StoreContext<'_, T>, waited| {
                let tables = store.internal().tables_handle();
                if let Err(error) = waited {
                    prepared.remove(&tables);
                    return Err(error.into());
                }
                let status = status_word(&tables, subtask, caller_table)?;
                Ok(vec![RuntimeVal::I32(status.value() as i32)])
            },
        ));
    }
    let status = status_word(&tables, subtask, caller_table)?;
    Ok(BlockStep::Ready(vec![RuntimeVal::I32(
        status.value() as i32
    )]))
}

/// The finish part of one call for a sync caller: the caller's flat
/// results, once the call resolved.
fn finish_sync_caller<T: 'static>(
    store: &mut StoreContext<'_, T>,
    prepared: &Prepared,
    failure: &StartFailure,
    waited: Result<()>,
    slots: usize,
) -> Result<Vec<RuntimeVal>> {
    let tables = store.internal().tables_handle();
    let subtask = prepared.subtask();
    if let Some(error) = failure.lock().ok().and_then(|mut slot| slot.take()) {
        prepared.remove(&tables);
        return Err(error);
    }
    if let Err(error) = waited {
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
    if flat_results.len() != slots {
        return Err(Error::internal(format!(
            "the return function of a prepared call produced {} flat results for a caller with {}",
            flat_results.len(),
            slots
        )));
    }
    Ok(flat_results)
}

/// Give back what a sync caller's call whose wait failed still holds.
///
/// The wait fails with a scheduler cause: the caller must not block,
/// the store is idle, or the target has no stack switch to serve the
/// suspension. Each of them says that the callee will never resolve
/// for this caller. So the store is left as it was before the call,
/// which is what the trap path leaves it as, reached the other way:
/// the subtask resolves as a cancellation, which gives back every
/// handle the caller lent, its record goes, and the callee's task
/// ends, which gives back the instance that the task's implicit thread
/// held exclusively.
///
/// The task ends and is not abandoned, because its scope is already
/// off the stack by the time a wait can fail: the callee either parked
/// between events or never ran at all, and an abandon finds no scope
/// to unwind. The borrows that the callee did not drop go with the
/// record, because the call has already failed and nothing is left to
/// report them to. The item that the callee parked with goes with the
/// record too.
fn release_wait<T: 'static>(store: &mut StoreContext<'_, T>, subtask: SubtaskId, task: TaskId) {
    release_subtask(store, subtask, None);
    let _ = store.internal().end_export_task(task);
}

/// The status word that an async caller's lower answers with.
///
/// A subtask that resolved before the lower returns delivers its
/// resolution here, which gives back every handle the caller lent for
/// the call, and its record leaves the store: the caller is given no
/// entry, so there is nothing for it to wait on or to drop. A subtask
/// that did not resolve enters the caller's handle table, and the word
/// carries the state it is in with that index.
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
    let index = guard
        .insert_subtask(caller_table, subtask)
        .map_err(|e| Error::Trap(anyhow::Error::from(e)))?;
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
