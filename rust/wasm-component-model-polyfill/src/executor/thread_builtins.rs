//! The thread built-ins that never switch stacks: `thread.index`,
//! `thread.new-indirect`, and `thread.resume-later`.
//!
//! Each component instance keeps a table of its threads, and these
//! three built-ins work on the table of the instance that calls them.
//! A task's implicit thread takes its index as the task starts. An
//! explicit thread takes one as `thread.new-indirect` creates it, and
//! gives it back when its start function returns.
//!
//! - `thread.index` returns the current thread's index.
//! - `thread.new-indirect` reads a start function out of a table,
//!   creates a suspended thread of the current task that will call it
//!   with the context value the guest passed, and returns the new
//!   thread's index. The start function takes one `i32`, or one `i64`
//!   in a 64-bit memory, and returns nothing; the context value is of
//!   the same type. The built-in fails with Wasmtime's message when
//!   the index is out of the table's bounds, when the entry holds no
//!   function, and when the function is of another type.
//! - `thread.resume-later` makes a suspended thread ready. The thread
//!   runs in a later turn: the built-in queues the thread's start as
//!   a resumption after a yield, which is where Wasmtime queues it
//!   too. A thread that is not suspended fails with Wasmtime's message
//!   "cannot resume thread which is not suspended", and an index that
//!   names no thread fails as Wasmtime's handle table fails it.
//!
//! Each built-in first traps with the cannot-leave cause when the
//! instance's may-leave flag is clear, which is the case while a
//! `realloc` or a `post-return` of that instance runs.
//!
//! An explicit thread runs in its task's scope, so a borrow it takes
//! counts against the task and `thread.index` answers the thread's
//! own index while it runs. It does not take the instance's exclusive
//! thread: only the implicit thread of a task lifted synchronously or
//! with a callback does. When its start function returns, the thread
//! leaves the instance's table and its task's list of threads. A
//! start function that traps ends the thread the same way, and the
//! trap is the failure of the thread's task.
//!
//! A thread's start is its task's pending work. A task that ends
//! before a turn runs the start takes the start with it, under the
//! store's rule for the items of a task that ends.

use std::sync::{Arc, Mutex};

use anyhow::anyhow;
use wasm_runtime_layer::{AsContextMut, Func as RuntimeFunc, Val as RuntimeVal};

use crate::abi::layout::FlatType;
use crate::abi::runtime_state::AbiRuntimeState;
use crate::backend::substrate_failure;
use crate::concurrency::{InstanceId, Item, ItemKind, TaskId, ThreadId, ThreadStart};
use crate::error::{Error, Result, TaskCause, ThreadCause};
use crate::executor::intrinsics::core_func_type;
use crate::executor::ir::{CoreParameter, CoreSignature};
use crate::internal::ErrorInternal;
use crate::resource::HandleTables;
use crate::store::StoreContext;
use crate::store::StoreContextInternalExt;

/// Build the `thread.index` built-in for `instance`: it returns the
/// current thread's index in the instance's thread table.
pub fn build_thread_index<T: 'static>(
    store: &mut StoreContext<'_, T>,
    instance: usize,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> RuntimeFunc {
    let tables = store.internal().tables_handle();
    RuntimeFunc::new(
        store.internal().runtime_mut(),
        core_func_type(signature),
        move |mut store_ctx, _args, results| {
            let id = calling_instance(&abi_state, instance)?;
            trap_if_cannot_leave(&abi_state, id, &mut store_ctx)?;
            let mut guard = lock_tables(&tables)?;
            let thread = guard
                .tasks
                .current_thread()
                .ok_or_else(|| anyhow!("`thread.index` ran with no thread on the stack"))?;
            // A thread that has no index yet is one whose task began
            // without the start that registers it; it takes its index
            // on first asking, as the reference's lazy allocation of
            // thread state allows.
            let index = guard.tasks.register_thread(thread).ok_or_else(|| {
                anyhow!("`thread.index` ran on a thread with no thread table to join")
            })?;
            results[0] = RuntimeVal::I32(index as i32);
            Ok(())
        },
    )
}

/// Build the `thread.new-indirect` built-in for `instance`, reading
/// its start function out of the table in runtime-table slot
/// `table`. The type of the second parameter of `signature`, the
/// context value, is the type the start function takes.
pub fn build_thread_new_indirect<T: 'static>(
    store: &mut StoreContext<'_, T>,
    instance: usize,
    table: usize,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> RuntimeFunc {
    let tables = store.internal().tables_handle();
    let context_type = match signature.params.get(1) {
        Some(CoreParameter::Value(FlatType::I64)) => FlatType::I64,
        _ => FlatType::I32,
    };
    RuntimeFunc::new(
        store.internal().runtime_mut(),
        core_func_type(signature),
        move |mut store_ctx, args, results| {
            let id = calling_instance(&abi_state, instance)?;
            trap_if_cannot_leave(&abi_state, id, &mut store_ctx)?;
            let function_index = arg_u32(args, 0)?;
            let context = args
                .get(1)
                .cloned()
                .ok_or_else(|| anyhow!("`thread.new-indirect` took no context value"))?;
            let start_table = abi_state
                .lock()
                .map_err(|_| anyhow!("ABI runtime state lock poisoned"))?
                .thread_start_tables
                .get(table)
                .cloned()
                .flatten()
                .ok_or_else(|| {
                    anyhow!("`thread.new-indirect` named a table the instantiation did not extract")
                })?;
            let function = start_table
                .start_function(&mut store_ctx, function_index, context_type)?
                .map_err(trap)?;
            let mut guard = lock_tables(&tables)?;
            let task = guard
                .tasks
                .current_task()
                .ok_or_else(|| anyhow!("`thread.new-indirect` ran with no task on the stack"))?;
            let (_, index) = guard
                .tasks
                .create_thread(task, ThreadStart { function, context })
                .ok_or_else(|| {
                    anyhow!("`thread.new-indirect` found no thread table for the current task")
                })?;
            results[0] = RuntimeVal::I32(index as i32);
            Ok(())
        },
    )
}

/// Build the `thread.resume-later` built-in for `instance`: the named
/// suspended thread becomes ready, and a later turn starts it.
pub fn build_thread_resume_later<T: 'static>(
    store: &mut StoreContext<'_, T>,
    instance: usize,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> RuntimeFunc {
    let tables = store.internal().tables_handle();
    RuntimeFunc::new(
        store.internal().runtime_mut(),
        core_func_type(signature),
        move |mut store_ctx, args, _results| {
            let id = calling_instance(&abi_state, instance)?;
            trap_if_cannot_leave(&abi_state, id, &mut store_ctx)?;
            let index = arg_u32(args, 0)?;
            let (thread, task, start) = {
                let mut guard = lock_tables(&tables)?;
                make_ready(&mut guard, id, index).map_err(trap)?
            };
            let item = Item::new(
                ItemKind::ThreadStart,
                move |store: &mut StoreContext<'_, T>| run_thread(store, task, thread, start),
            )
            .in_instance(id)
            .for_task(task);
            let mut store = StoreContext::new(store_ctx.as_context_mut());
            store.internal().scheduler_mut().push_low_priority(item);
            Ok(())
        },
    )
}

/// Make the thread at `index` of `instance`'s thread table ready, and
/// hand back what its start needs: the thread, its task, and what it
/// runs. The reference's `canon_thread_resume_later` traps on an
/// index that names no thread and on a thread that is not suspended.
fn make_ready(
    tables: &mut HandleTables,
    instance: InstanceId,
    index: u32,
) -> Result<(ThreadId, TaskId, ThreadStart)> {
    let thread = tables
        .tasks
        .thread_at(instance, index)
        .ok_or(Error::Thread(ThreadCause::UnknownThread { index }))?;
    let record = tables
        .tasks
        .thread_mut(thread)
        .ok_or_else(|| Error::internal("a thread table names a thread the store does not hold"))?;
    if !record.suspended {
        return Err(Error::Thread(ThreadCause::NotSuspended));
    }
    let start = record
        .start
        .take()
        .ok_or_else(|| Error::internal("a suspended thread has nothing to start"))?;
    record.suspended = false;
    Ok((thread, record.task, start))
}

/// Run an explicit thread from its start to its end, which is what
/// the item `thread.resume-later` queued does.
///
/// The thread runs in its task's scope. Its end is the same whether
/// its start function returned or trapped: the scope it pushed is
/// popped, with whatever a failed call left above it, and the thread
/// leaves its instance's table and its task. A trap is then the
/// failure of the thread's task, which reaches the call that started
/// the task when that call is waiting on it and ends the turn
/// otherwise.
fn run_thread<T: 'static>(
    store: &mut StoreContext<'_, T>,
    task: TaskId,
    thread: ThreadId,
    start: ThreadStart,
) -> Result<()> {
    store
        .internal()
        .lock_tables()?
        .tasks
        .enter_thread(thread)
        .ok_or_else(|| Error::internal("a thread started whose record is not in the store"))?;
    let outcome = start
        .function
        .call(store.internal().runtime_mut(), &[start.context], &mut [])
        .map_err(substrate_failure);
    {
        let mut guard = store.internal().lock_tables()?;
        guard.leave_thread(thread);
        guard.tasks.end_thread(thread);
    }
    match outcome {
        Ok(()) => Ok(()),
        Err(error) => store.internal().fail_export_task(Some(task), error),
    }
}

/// The store-wide identity of the component instance the translator
/// named for a built-in.
fn calling_instance(
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    instance: usize,
) -> anyhow::Result<InstanceId> {
    let state = abi_state
        .lock()
        .map_err(|_| anyhow!("ABI runtime state lock poisoned"))?;
    state
        .component_instances
        .get(instance)
        .copied()
        .ok_or_else(|| {
            anyhow!(
                "a built-in named component instance {instance}, which this instantiation does not hold"
            )
        })
}

/// Refuse the built-in when the instance may not be left, which is
/// the case while a `realloc` or a `post-return` of that instance
/// runs. The flag is the core global the instance's adapters compile
/// against, so this reads what the generated code reads.
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
    Err(trap(Error::Task(TaskCause::CannotLeave)))
}

/// Lock the store's handle tables and record state.
fn lock_tables(
    tables: &Arc<Mutex<HandleTables>>,
) -> anyhow::Result<std::sync::MutexGuard<'_, HandleTables>> {
    tables
        .lock()
        .map_err(|_| anyhow!("resource handle tables lock poisoned"))
}

/// The trap a structured error becomes on its way to the guest. The
/// message is the error's own, which the conformance corpora match
/// by substring.
fn trap(error: Error) -> anyhow::Error {
    anyhow!("{error}")
}

fn arg_u32(args: &[RuntimeVal], index: usize) -> anyhow::Result<u32> {
    match args.get(index) {
        Some(RuntimeVal::I32(value)) => Ok(*value as u32),
        _ => Err(anyhow!("a thread built-in expected an i32 argument")),
    }
}
