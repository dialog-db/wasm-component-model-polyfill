//! The `task.cancel` and `subtask.cancel` built-ins.
//!
//! Cancellation is cooperative. A caller asks a subtask to stop with
//! `subtask.cancel`, and the callee decides when to stop: it confirms
//! the request with `task.cancel`, or returns a result all the same.
//! Nothing ends a guest task by force.
//!
//! `subtask.cancel` follows the reference's `canon_subtask_cancel`.
//! It traps, in this order, when the instance's may-leave flag is
//! clear, when the index names no subtask, when the subtask's
//! resolution was already delivered, when cancellation was already
//! requested, and when the subtask is in a waitable set. It then asks
//! the callee to stop:
//!
//! - A callee its entry gate still holds never runs. Its task is
//!   given up where it waits, and the subtask resolves to
//!   `CANCELLED_BEFORE_STARTED`. The records leave the store when the
//!   caller drops the subtask.
//! - A started callee's task becomes pending-cancel, and learns of
//!   the request once, at the next point its callback loop consults
//!   it, as [`CallbackTask`](crate::executor::CallbackTask) states. A
//!   callback task waiting in its loop takes the request at once: the
//!   built-in wakes it and gives way to it, from inside its own frame,
//!   as Wasmtime wakes the first thread that can take the request. A
//!   stackful task is never told, and the cancel waits until it
//!   resolves on its own.
//!
//! When the callee has not resolved by then, an asynchronous cancel
//! gives way once, as `thread.yield` does, unless it already gave way
//! to the callee it woke, and answers `BLOCKED` when the callee still
//! has not resolved; the caller then waits for the subtask event. A
//! synchronous cancel blocks until the callee resolves, as every
//! blocking built-in blocks, and a block that cannot progress fails
//! with the cause the suspend seam names. The subtask is waited on
//! synchronously for as long as the built-in runs, so the callee
//! cannot add it to a waitable set meanwhile.
//!
//! A cancel that finds the callee resolved delivers the resolution,
//! which gives back the handles the caller lent, and answers the
//! subtask's state: `RETURNED` when the callee returned first,
//! `CANCELLED_BEFORE_RETURNED` when it confirmed, and
//! `CANCELLED_BEFORE_STARTED` for a callee the gate held.
//!
//! The cancellation of a host callee is not built. A subtask of a
//! call into a host function passes the traps above and then fails
//! with [`Error::Unsupported`].
//!
//! `task.cancel` resolves the current task as cancelled, following
//! the reference's `Task.cancel` with Wasmtime's messages. The
//! instance must be leavable, a cancellation request must have been
//! delivered to the task, the task must not have resolved, and it
//! must owe no borrow. Its threads go on after it, and the task lives
//! until its last thread ends.
//!
//! Every failure travels as the structured error itself rather than
//! as its message, so the call the guest is inside fails with a
//! substrate failure a host can read the cause back out of.

use std::sync::{Arc, Mutex};

use anyhow::anyhow;
use wasm_runtime_layer::{AsContextMut, Func as RuntimeFunc, Val as RuntimeVal};

use crate::abi::runtime_state::AbiRuntimeState;
use crate::concurrency::{
    BlockStep, BlockingBuiltin, InstanceId, LowerKind, Readiness, SubtaskId, SuspendSeam, TaskId,
    TaskState,
};
use crate::error::{AbiCause, AbiError, AbiPosition, Error, TaskCause, WaitableCause};
use crate::executor::intrinsics::core_func_type;
use crate::executor::ir::CoreSignature;
use crate::internal::ErrorInternal;
use crate::resource::{HandleTables, TableId};
use crate::store::StoreContext;
use crate::store::StoreContextInternalExt;

use super::start_call::{lock, run_nested_start};

/// What `subtask.cancel` answers when the callee has not resolved by
/// the time it returns: the reference's `BLOCKED`.
const BLOCKED: u32 = 0xffff_ffff;

/// Build the `task.cancel` built-in for `instance`, the translator's
/// per-instantiation index of the component instance that calls it.
pub fn build_task_cancel<T: 'static>(
    store: &mut StoreContext<'_, T>,
    instance: usize,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> RuntimeFunc {
    let tables = store.internal().tables_handle();
    RuntimeFunc::new(
        store.internal().runtime_mut(),
        core_func_type(signature),
        move |mut store_ctx, _args, _results| {
            let (id, _table) = calling_instance(&abi_state, instance)?;
            trap_if_cannot_leave(&abi_state, id, &mut store_ctx)?;
            Ok(task_cancel(&tables)?)
        },
    )
}

/// Resolve the current task as cancelled, which is what one call of
/// `task.cancel` does once the may-leave check has passed.
fn task_cancel(tables: &Arc<Mutex<HandleTables>>) -> crate::error::Result<()> {
    let mut guard = lock(tables)?;
    let task = guard
        .tasks
        .current_task()
        .ok_or_else(|| Error::internal("`task.cancel` ran with no task on the stack"))?;
    let record = guard
        .tasks
        .task(task)
        .ok_or_else(|| Error::internal("the current task has no record"))?;
    // A task whose lift is not `async` is never asked to cancel, so
    // the one check covers it too.
    if !record.cancel_delivered {
        return Err(Error::Task(TaskCause::CancelNotDelivered));
    }
    if record.state == TaskState::Resolved {
        return Err(Error::Task(TaskCause::ReturnedTwice));
    }
    if record.num_borrows > 0 {
        return Err(Error::from(AbiError {
            position: AbiPosition::Result,
            valtype: None,
            cause: AbiCause::OutstandingBorrows {
                count: record.num_borrows as usize,
            },
        }));
    }
    if let Some(subtask) = record.subtask {
        guard.tasks.subtask_cancelled_by_callee(subtask)?;
    }
    if !guard.resolve_task(task, None)? {
        return Err(Error::internal("the current task has no record"));
    }
    Ok(())
}

/// Build the `subtask.cancel` built-in for `instance`, the
/// translator's per-instantiation index of the component instance
/// that calls it. `async_` is the built-in's `async` option.
pub fn build_subtask_cancel<T: 'static>(
    _store: &mut StoreContext<'_, T>,
    instance: usize,
    async_: bool,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> BlockingBuiltin<T> {
    BlockingBuiltin::new(
        core_func_type(signature),
        move |store: &mut StoreContext<'_, T>, args: &[RuntimeVal]| {
            begin_subtask_cancel(store, &abi_state, instance, async_, args)
        },
    )
}

/// The first part of `subtask.cancel`: the traps, the request, and
/// the give-way to a callee the request woke. What is left is the
/// wait the module documentation states.
fn begin_subtask_cancel<T: 'static>(
    store: &mut StoreContext<'_, T>,
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    instance: usize,
    async_: bool,
    args: &[RuntimeVal],
) -> anyhow::Result<BlockStep<T>> {
    let index = match args.first() {
        Some(RuntimeVal::I32(index)) => *index as u32,
        _ => return Err(anyhow!("`subtask.cancel` expected an i32 argument")),
    };
    let (id, table) = calling_instance(abi_state, instance)?;
    trap_if_cannot_leave(abi_state, id, store.internal().runtime_mut())?;
    let tables = store.internal().tables_handle();
    let (subtask, callee) = claim(&tables, table, index)?;

    // From here the subtask is waited on synchronously, and a failure
    // ends that wait before it travels out.
    let woken = resolved(&tables, subtask).and_then(|resolved| {
        if resolved {
            Ok(None)
        } else {
            request_cancellation(store, subtask, callee)
        }
    });
    let woken = ending_wait_on_failure(&tables, subtask, woken)?;
    let Some(callee_instance) = woken else {
        let step = after_request(&tables, subtask, async_, false);
        return Ok(ending_wait_on_failure(&tables, subtask, step)?);
    };

    // The callee took the request at once, so the cancel gives way to
    // it here: the woken callback runs above this frame, and the
    // cancel goes on once it returns or suspends.
    let lower = if async_ {
        LowerKind::Async
    } else {
        LowerKind::Sync
    };
    if let Err(error) = run_nested_start(store, subtask, callee_instance, lower) {
        end_synchronous_wait(&tables, subtask);
        return Err(error.into());
    }
    if store.internal().defers_work() {
        // Under a provider that resumes a thread only where the store
        // runs no guest code, the callee's run is the store's to
        // finish, and the rest of the cancel follows it. A woken
        // callback that traps before it first suspends comes here in
        // the browser, whose provider hands the failure over on a
        // microtask.
        return Ok(BlockStep::wait(
            Readiness::Planned,
            move |store: &mut StoreContext<'_, T>, waited| {
                if let Err(error) = waited {
                    end_synchronous_wait(&tables, subtask);
                    return Err(error.into());
                }
                if !async_ && !resolved(&tables, subtask)? {
                    let waited =
                        SuspendSeam::<T>::wait_until(store, Readiness::Subtask { subtask });
                    if let Err(error) = waited {
                        end_synchronous_wait(&tables, subtask);
                        return Err(error.into());
                    }
                }
                Ok(vec![status(&tables, subtask)?])
            },
        ));
    }
    let step = after_request(&tables, subtask, async_, true);
    Ok(ending_wait_on_failure(&tables, subtask, step)?)
}

/// Look up the subtask at `index` of `table` and make the traps that
/// come before the request, in the reference's order. A subtask that
/// passes them is waited on synchronously from here on and marked as
/// asked to cancel, and its callee's task comes back with it.
fn claim(
    tables: &Arc<Mutex<HandleTables>>,
    table: TableId,
    index: u32,
) -> crate::error::Result<(SubtaskId, TaskId)> {
    let mut guard = lock(tables)?;
    let subtask = guard.subtask_from_handle(table, index).map_err(|err| {
        Error::from(AbiError {
            position: AbiPosition::Argument(0),
            valtype: None,
            cause: AbiCause::InvalidHandle {
                reason: err.to_string(),
            },
        })
    })?;
    let waitable = guard.tasks.subtask_waitable(subtask);
    let record = guard
        .tasks
        .subtask(subtask)
        .ok_or_else(|| Error::internal("a subtask entry names no record"))?;
    if record.resolve_delivered {
        return Err(Error::Waitable(WaitableCause::SubtaskCancelAfterTerminal));
    }
    if record.cancel_requested {
        return Err(Error::Waitable(WaitableCause::SubtaskCancelledTwice));
    }
    let callee = record.callee;
    if guard.tasks.waitable_set_of(waitable)?.is_some() {
        return Err(Error::Waitable(WaitableCause::SyncAndAsync));
    }
    let Some(callee) = callee else {
        return Err(Error::unsupported(
            "cancellation of a call into a host function (`subtask.cancel`)",
        ));
    };
    guard.tasks.begin_synchronous_wait(waitable)?;
    if let Some(record) = guard.tasks.subtask_mut(subtask) {
        record.cancel_requested = true;
    }
    Ok((subtask, callee))
}

/// Ask the callee's task to stop, which is the reference's
/// `request_cancellation`. Answers the callee's component instance
/// when the request woke a callback task waiting in its loop, which
/// the cancel then gives way to.
///
/// A task that has not started is given up before it runs, and its
/// subtask resolves to `CANCELLED_BEFORE_STARTED`. A started task
/// becomes pending-cancel. A task that resolved is left alone.
fn request_cancellation<T: 'static>(
    store: &mut StoreContext<'_, T>,
    subtask: SubtaskId,
    task: TaskId,
) -> crate::error::Result<Option<InstanceId>> {
    let tables = store.internal().tables_handle();
    let (state, thread, instance) = {
        let mut guard = lock(&tables)?;
        let state = guard.tasks.request_cancellation(task);
        let record = guard
            .tasks
            .task(task)
            .ok_or_else(|| Error::internal("a subtask's callee task is not in the store"))?;
        (state, record.implicit_thread, record.instance)
    };
    match state {
        Some(TaskState::Initial) => {
            cancel_before_start(store, subtask, task)?;
            Ok(None)
        }
        Some(TaskState::Started) => {
            // A callback task waiting in its loop can take the request
            // at once: its held item runs next, where the request is
            // delivered before any event of the set. The thread stops
            // waiting, and the set counts it as a waiter until the item
            // runs.
            let Some((set, slot, item)) = store
                .internal()
                .scheduler_mut()
                .take_held_callback_of(thread)
            else {
                return Ok(None);
            };
            {
                let mut guard = lock(&tables)?;
                guard.tasks.end_wait(set, thread)?;
                guard.tasks.queue_wait(set, thread)?;
            }
            slot.fill_from(set);
            store.internal().scheduler_mut().switch_to(item);
            Ok(instance)
        }
        _ => Ok(None),
    }
}

/// Give up a callee the entry gate still holds, which is the
/// reference's delivery of a request at `enter_implicit_thread`: the
/// subtask resolves to `CANCELLED_BEFORE_STARTED`, and the task ends
/// where it waits, never having run. Its record stays while the
/// caller's entry names it, as a finished callee's does, and leaves
/// with the entry.
fn cancel_before_start<T: 'static>(
    store: &mut StoreContext<'_, T>,
    subtask: SubtaskId,
    task: TaskId,
) -> crate::error::Result<()> {
    {
        let tables = store.internal().tables_handle();
        let mut guard = lock(&tables)?;
        guard.tasks.subtask_cancelled(subtask)?;
        if let Some(record) = guard.tasks.task_mut(task) {
            record.state = TaskState::Resolved;
        }
    }
    // The end drops the start the gate or a ready queue still holds,
    // lowers the count of the tasks waiting to enter, and gives back
    // the instance a start the gate already let through had claimed.
    let _borrows = store.internal().end_export_task(task)?;
    Ok(())
}

/// The rest of `subtask.cancel` once the request was made: answer at
/// once when the callee resolved, and otherwise give way once or
/// block, as the module documentation states. `gave_way` says the
/// cancel already gave way to a callee it woke.
fn after_request<T: 'static>(
    tables: &Arc<Mutex<HandleTables>>,
    subtask: SubtaskId,
    async_: bool,
    gave_way: bool,
) -> crate::error::Result<BlockStep<T>> {
    if resolved(tables, subtask)? || (async_ && gave_way) {
        return Ok(BlockStep::Ready(vec![status(tables, subtask)?]));
    }
    let readiness = if async_ {
        Readiness::Yielded
    } else {
        Readiness::Subtask { subtask }
    };
    let tables = tables.clone();
    Ok(BlockStep::wait(
        readiness,
        move |_store: &mut StoreContext<'_, T>, waited| {
            if let Err(error) = waited {
                end_synchronous_wait(&tables, subtask);
                return Err(error.into());
            }
            Ok(vec![status(&tables, subtask)?])
        },
    ))
}

/// What the built-in answers now: the subtask's state, once its
/// resolution is delivered here, or `BLOCKED` when it has not
/// resolved. The synchronous wait on the subtask ends either way.
///
/// Delivery gives back the handles the caller lent for the call and
/// takes the subtask event the resolution left, since the state this
/// answers is that event's news.
fn status(
    tables: &Arc<Mutex<HandleTables>>,
    subtask: SubtaskId,
) -> crate::error::Result<RuntimeVal> {
    let mut guard = lock(tables)?;
    let waitable = guard.tasks.subtask_waitable(subtask);
    let state = guard
        .tasks
        .subtask(subtask)
        .map(|record| record.state)
        .ok_or_else(|| Error::internal("a cancelled subtask's record left the store"))?;
    guard.tasks.end_synchronous_wait(waitable)?;
    if !state.resolved() {
        return Ok(RuntimeVal::I32(BLOCKED as i32));
    }
    guard.deliver_subtask_resolution(subtask)?;
    guard.tasks.take_pending_event(waitable)?;
    Ok(RuntimeVal::I32(state.value() as i32))
}

/// Whether the call `subtask` records has resolved. A record that is
/// gone counts as resolved, because a call that failed takes its
/// record away.
fn resolved(tables: &Arc<Mutex<HandleTables>>, subtask: SubtaskId) -> crate::error::Result<bool> {
    Ok(lock(tables)?
        .tasks
        .subtask(subtask)
        .is_none_or(|record| record.state.resolved()))
}

/// End the synchronous wait the cancel holds on `subtask`, on a path
/// that answers nothing. A record that is gone has no wait to end.
fn end_synchronous_wait(tables: &Arc<Mutex<HandleTables>>, subtask: SubtaskId) {
    if let Ok(mut guard) = tables.lock()
        && guard.tasks.subtask(subtask).is_some()
    {
        let waitable = guard.tasks.subtask_waitable(subtask);
        let _ = guard.tasks.end_synchronous_wait(waitable);
    }
}

/// The store-wide identity of the component instance the translator
/// named for the built-in, with the handle table that instance keeps.
fn calling_instance(
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    instance: usize,
) -> anyhow::Result<(InstanceId, TableId)> {
    let state = abi_state
        .lock()
        .map_err(|_| anyhow!("ABI runtime state lock poisoned"))?;
    let id = state.component_instances.get(instance).copied();
    let table = state.handle_tables.get(instance).copied();
    match (id, table) {
        (Some(id), Some(table)) => Ok((id, table)),
        _ => Err(anyhow!(
            "a built-in named component instance {instance}, which this instantiation does not hold"
        )),
    }
}

/// Pass `result` through, ending the synchronous wait the cancel
/// holds on `subtask` first when it is a failure.
fn ending_wait_on_failure<R>(
    tables: &Arc<Mutex<HandleTables>>,
    subtask: SubtaskId,
    result: crate::error::Result<R>,
) -> crate::error::Result<R> {
    if result.is_err() {
        end_synchronous_wait(tables, subtask);
    }
    result
}

/// Refuse the built-in when the instance may not be left. The flag
/// is the core global the instance's adapters compile against, so
/// this reads what the generated code reads.
fn trap_if_cannot_leave(
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    instance: InstanceId,
    store: impl AsContextMut,
) -> anyhow::Result<()> {
    let flags = {
        let state = abi_state
            .lock()
            .map_err(|_| anyhow!("ABI runtime state lock poisoned"))?;
        state.flags_of(instance).cloned().ok_or_else(|| {
            anyhow!("a built-in named an instance with no may-leave flag of its own")
        })?
    };
    if flags.may_leave(store).map_err(|err| anyhow!("{err}"))? {
        return Ok(());
    }
    Err(Error::Task(TaskCause::CannotLeave).into())
}
