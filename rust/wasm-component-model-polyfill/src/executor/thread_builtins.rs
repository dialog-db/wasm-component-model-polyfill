//! The thread built-ins other than `thread.yield`: `thread.index`,
//! `thread.new-indirect`, and `thread.resume-later`, which never
//! switch stacks, and the five that suspend or switch.
//!
//! Each component instance keeps a table of its threads, and these
//! built-ins work on the table of the instance that calls them.
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
//! - `thread.resume-later` makes a suspended thread ready. A thread
//!   that has never run runs in a later turn: the built-in queues the
//!   thread's start as a resumption after a yield, which is where
//!   Wasmtime queues it too. A thread suspended in a built-in of its
//!   own goes on once the frame that built-in waits in sees it ready.
//!   A thread that is not suspended fails with Wasmtime's message
//!   "cannot resume thread which is not suspended", and an index that
//!   names no thread fails as Wasmtime's handle table fails it.
//! - `thread.suspend` suspends the current thread until a resume
//!   names it.
//! - `thread.suspend-then-resume` and `thread.yield-then-resume`
//!   suspend the current thread, or make it ready, and switch to the
//!   suspended thread they name. A thread that is not suspended fails
//!   with the not-suspended message.
//! - `thread.suspend-then-promote` and `thread.yield-then-promote`
//!   switch to the thread they name when that thread is ready, and
//!   otherwise suspend or yield. A promote that names the current
//!   thread traps with the not-suspended message, which is the one
//!   Wasmtime raises there.
//!
//! Each built-in first traps with the cannot-leave cause when the
//! instance's may-leave flag is clear, which is the case while a
//! `realloc` or a `post-return` of that instance runs.
//!
//! A switch is a suspension that names the thread to run next. The
//! frame that resumed the current thread runs the named thread before
//! anything else, which is the reference's `Thread.resume` loop. Each
//! suspending built-in reaches the suspend seam through a host
//! trampoline, which cannot suspend a guest stack, so it blocks
//! through the seam's nested turn whatever provider the engine
//! selected, and a switch runs on the real stack:
//!
//! - A suspension waits until a nested turn runs the work that
//!   resumes the thread. A task that must not block runs the ready
//!   work of its own instance alone, and then fails with the
//!   cannot-block cause, as for every other block.
//! - A switch to a thread that has never run starts that thread from
//!   inside the built-in, on the real stack above it, as a nested
//!   start. The store's stack of current scopes carries a
//!   thread-switch mark for as long as the thread runs. Once the
//!   thread returns, a yielding built-in goes on, and a suspending one
//!   waits to be resumed.
//! - A switch to a thread that has run and is suspended, or waits, in
//!   a built-in of its own cannot run: that built-in's frame lies
//!   below the current one on the real stack. The switch fails with
//!   the stack-switch cause.
//!
//! An explicit thread runs in its task's scope, so a borrow it takes
//! counts against the task and `thread.index` answers the thread's
//! own index while it runs. It does not take the instance's exclusive
//! thread: only the implicit thread of a task lifted synchronously or
//! with a callback does. When its start function returns, the thread
//! leaves the instance's table and its task's list of threads. A
//! start function that traps ends the thread the same way. The trap
//! is the failure of the thread's task when a turn started the
//! thread, and the failure of the built-in when a switch did.
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
use crate::concurrency::{InstanceId, Item, ItemKind, SuspendSeam, TaskId, ThreadId, ThreadStart};
use crate::error::{Error, Result, SchedulerCause, TaskCause, ThreadCause};
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
/// suspended thread becomes ready. A thread that has never run starts
/// in a later turn, from the scheduler; a thread suspended in a
/// built-in goes on once the frame that built-in blocks in sees it
/// ready.
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
            let (thread, task, never_ran) = {
                let mut guard = lock_tables(&tables)?;
                make_ready(&mut guard, id, index).map_err(trap)?
            };
            if !never_ran {
                return Ok(());
            }
            let item = Item::new(
                ItemKind::ThreadStart,
                move |store: &mut StoreContext<'_, T>| start_ready_thread(store, thread),
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
/// answer the thread, its task, and whether it has never run. The
/// reference's `canon_thread_resume_later` traps on an index that
/// names no thread and on a thread that is not suspended.
fn make_ready(
    tables: &mut HandleTables,
    instance: InstanceId,
    index: u32,
) -> Result<(ThreadId, TaskId, bool)> {
    let thread = named_thread(tables, instance, index)?;
    let task = tables
        .tasks
        .thread(thread)
        .map(|record| record.task)
        .ok_or_else(|| Error::internal("a thread table names a thread the store does not hold"))?;
    let never_ran = tables.tasks.resume_later(thread)?;
    Ok((thread, task, never_ran))
}

/// The thread at `index` of `instance`'s thread table. An index that
/// names no thread fails as Wasmtime's handle table fails it.
fn named_thread(tables: &HandleTables, instance: InstanceId, index: u32) -> Result<ThreadId> {
    tables
        .tasks
        .thread_at(instance, index)
        .ok_or(Error::Thread(ThreadCause::UnknownThread { index }))
}

/// Start an explicit thread `thread.resume-later` made ready, which
/// is what the item that built-in queued does. A switch that named
/// the thread first has started it already, and the item then does
/// nothing.
///
/// A trap of the thread is the failure of the thread's task, which
/// reaches the call that started the task when that call is waiting
/// on it and ends the turn otherwise.
fn start_ready_thread<T: 'static>(store: &mut StoreContext<'_, T>, thread: ThreadId) -> Result<()> {
    let Some((task, start)) = store
        .internal()
        .lock_tables()?
        .tasks
        .take_thread_start(thread)
    else {
        return Ok(());
    };
    match run_thread(store, thread, start) {
        Ok(()) => Ok(()),
        Err(error) => store.internal().fail_export_task(Some(task), error),
    }
}

/// Run an explicit thread from its start to its end.
///
/// The thread runs in its task's scope. Its end is the same whether
/// its start function returned or trapped: the scope it pushed is
/// popped, with whatever a failed call left above it, and the thread
/// leaves its instance's table and its task. A trap comes back to
/// the caller, which says whose failure it is.
fn run_thread<T: 'static>(
    store: &mut StoreContext<'_, T>,
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
    outcome
}

/// Which thread a suspending built-in switches to.
#[derive(Clone, Copy)]
enum Switch {
    /// None: the built-in suspends or yields and names no thread.
    None,
    /// The suspended thread its argument names, which it resumes.
    Resume,
    /// The thread its argument names, when that thread is ready.
    Promote,
}

/// Build the `thread.suspend` built-in for `instance`: the current
/// thread suspends until a resume names it.
pub fn build_thread_suspend<T: 'static>(
    store: &mut StoreContext<'_, T>,
    instance: usize,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> RuntimeFunc {
    build_suspension(store, instance, signature, abi_state, false, Switch::None)
}

/// Build the `thread.suspend-then-resume` built-in for `instance`:
/// the current thread suspends and switches to the suspended thread
/// its argument names.
pub fn build_thread_suspend_then_resume<T: 'static>(
    store: &mut StoreContext<'_, T>,
    instance: usize,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> RuntimeFunc {
    build_suspension(store, instance, signature, abi_state, false, Switch::Resume)
}

/// Build the `thread.yield-then-resume` built-in for `instance`: the
/// current thread becomes ready and switches to the suspended thread
/// its argument names.
pub fn build_thread_yield_then_resume<T: 'static>(
    store: &mut StoreContext<'_, T>,
    instance: usize,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> RuntimeFunc {
    build_suspension(store, instance, signature, abi_state, true, Switch::Resume)
}

/// Build the `thread.suspend-then-promote` built-in for `instance`:
/// the current thread switches to the thread its argument names when
/// that thread is ready, and suspends otherwise.
pub fn build_thread_suspend_then_promote<T: 'static>(
    store: &mut StoreContext<'_, T>,
    instance: usize,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> RuntimeFunc {
    build_suspension(
        store,
        instance,
        signature,
        abi_state,
        false,
        Switch::Promote,
    )
}

/// Build the `thread.yield-then-promote` built-in for `instance`: the
/// current thread switches to the thread its argument names when that
/// thread is ready, and yields otherwise.
pub fn build_thread_yield_then_promote<T: 'static>(
    store: &mut StoreContext<'_, T>,
    instance: usize,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> RuntimeFunc {
    build_suspension(store, instance, signature, abi_state, true, Switch::Promote)
}

/// Build one of the five suspending built-ins. `yielding` says
/// whether the current thread stays ready rather than suspended, and
/// `switch` which thread it names. Each answers zero, as the
/// reference's built-ins do: nothing in this design delivers a
/// cancellation.
fn build_suspension<T: 'static>(
    store: &mut StoreContext<'_, T>,
    instance: usize,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
    yielding: bool,
    switch: Switch,
) -> RuntimeFunc {
    RuntimeFunc::new(
        store.internal().runtime_mut(),
        core_func_type(signature),
        move |mut store_ctx, args, results| {
            let id = calling_instance(&abi_state, instance)?;
            trap_if_cannot_leave(&abi_state, id, &mut store_ctx)?;
            let named = match switch {
                Switch::None => None,
                Switch::Resume | Switch::Promote => Some(arg_u32(args, 0)?),
            };
            let mut store = StoreContext::new(store_ctx.as_context_mut());
            suspension(&mut store, id, yielding, switch, named).map_err(suspension_trap)?;
            results[0] = RuntimeVal::I32(0);
            Ok(())
        },
    )
}

/// The body of a suspending built-in, once the may-leave check has
/// passed: find the thread to switch to, then suspend or yield
/// through the seam with that switch.
fn suspension<T: 'static>(
    store: &mut StoreContext<'_, T>,
    instance: InstanceId,
    yielding: bool,
    switch: Switch,
    named: Option<u32>,
) -> Result<()> {
    let current = store
        .internal()
        .lock_tables()?
        .tasks
        .current_thread()
        .ok_or_else(|| Error::internal("a thread built-in ran with no thread on the stack"))?;
    let target = match (switch, named) {
        (Switch::Resume, Some(index)) => Some(resume_target(store, instance, index, current)?),
        (Switch::Promote, Some(index)) => promote_target(store, instance, index, current)?,
        _ => None,
    };
    let run = move |store: &mut StoreContext<'_, T>| match target {
        Some(other) => start_switched(store, current, other),
        None => Ok(()),
    };
    match (yielding, target) {
        (true, Some(_)) => SuspendSeam::yield_to(store, run),
        (true, None) => SuspendSeam::give_way(store),
        (false, _) => SuspendSeam::suspend_current(store, run),
    }
}

/// The thread a resume names, which must be suspended, as the
/// reference's `canon_thread_suspend_then_resume` and
/// `canon_thread_yield_then_resume` state. The current thread is
/// running, so it is never suspended.
///
/// A suspended thread that has run is suspended in a built-in of its
/// own. Without a stack switch that built-in's frame lies below the
/// current one on the real stack, so the thread cannot run until the
/// current frame returns, and the switch fails with the stack-switch
/// cause.
fn resume_target<T: 'static>(
    store: &mut StoreContext<'_, T>,
    instance: InstanceId,
    index: u32,
    current: ThreadId,
) -> Result<ThreadId> {
    let guard = store.internal().lock_tables()?;
    let thread = named_thread(&guard, instance, index)?;
    let record = guard
        .tasks
        .thread(thread)
        .ok_or_else(|| Error::internal("a thread table names a thread the store does not hold"))?;
    if thread == current || !record.suspended {
        return Err(Error::Thread(ThreadCause::NotSuspended));
    }
    if record.start.is_none() {
        return Err(Error::Scheduler(SchedulerCause::StackSwitchNeeded));
    }
    Ok(thread)
}

/// The thread a promote switches to, or `None` when the thread it
/// names is not ready and the built-in suspends or yields instead.
///
/// A promote that names the current thread traps, as the reference
/// states. Its message is Wasmtime's, whose `resume_thread` raises
/// the not-suspended trap for the current thread whatever the
/// built-in. An index that names no thread fails as the handle table
/// fails it.
///
/// The thread is ready when it waits on a condition that holds, the
/// reference's `Thread.ready`:
///
/// - An explicit thread `thread.resume-later` made ready that has not
///   started yet is ready, and the switch starts it.
/// - The implicit thread of a callback task parked on a waitable set
///   is not on the stack. Its readiness also needs its instance's
///   exclusive thread free, which the recorded condition does not
///   say, so the promote suspends or yields instead, and the turns
///   of that wait run the callback when it is ready.
/// - Any other thread that waits is inside a built-in of its own,
///   below the current frame on the real stack. Without a stack
///   switch it cannot run until the current frame returns, and the
///   promote fails with the stack-switch cause.
fn promote_target<T: 'static>(
    store: &mut StoreContext<'_, T>,
    instance: InstanceId,
    index: u32,
    current: ThreadId,
) -> Result<Option<ThreadId>> {
    let (thread, startable, ready) = {
        let guard = store.internal().lock_tables()?;
        let thread = named_thread(&guard, instance, index)?;
        if thread == current {
            return Err(Error::Thread(ThreadCause::NotSuspended));
        }
        let record = guard.tasks.thread(thread).ok_or_else(|| {
            Error::internal("a thread table names a thread the store does not hold")
        })?;
        let startable = record.start.is_some() && record.readiness.is_some();
        (thread, startable, guard.tasks.thread_ready(thread))
    };
    if startable {
        return Ok(Some(thread));
    }
    if ready && !store.internal().scheduler().holds_callback_of(thread) {
        return Err(Error::Scheduler(SchedulerCause::StackSwitchNeeded));
    }
    Ok(None)
}

/// Start `other`, a thread that has never run, from inside the frame
/// of the built-in `switching` called, which is the switch of the
/// reference's `Thread.resume` loop without a stack switch: the
/// thread runs on the real stack above the built-in until it returns.
///
/// The stack carries a thread-switch mark for as long as the thread
/// runs, which the cause of a failed block above it reads. The mark
/// comes back off through an unwind too, as a start intrinsic's
/// nested-start mark does. A trap of the started thread is the
/// failure of the built-in that started it.
fn start_switched<T: 'static>(
    store: &mut StoreContext<'_, T>,
    switching: ThreadId,
    other: ThreadId,
) -> Result<()> {
    let tables = store.internal().tables_handle();
    let start = {
        let mut guard = tables
            .lock()
            .map_err(|_| Error::internal("resource handle tables lock poisoned"))?;
        let (_, start) = guard
            .tasks
            .take_thread_start(other)
            .ok_or_else(|| Error::internal("a switch named a thread with nothing to start"))?;
        guard.tasks.begin_thread_switch(switching);
        start
    };
    let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_thread(store, other, start)
    }));
    if let Ok(mut guard) = tables.lock() {
        guard.tasks.end_thread_switch();
    }
    ran.unwrap_or_else(|panic| std::panic::resume_unwind(panic))
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

/// The trap a suspending built-in's failure becomes on its way to the
/// guest. A cause of the built-in's own keeps its message, which the
/// conformance corpora match by substring, and a scheduler cause
/// takes the `wasm trap:` prefix `thread.yield` gives the seam's
/// causes. Any other failure is the failure of guest code the
/// built-in ran — the thread a switch started, or an item a nested
/// turn ran — and it keeps its whole chain, as the failure of a
/// callee a start intrinsic runs does.
fn suspension_trap(error: Error) -> anyhow::Error {
    match error {
        Error::Scheduler(cause) => anyhow!("wasm trap: {cause}"),
        Error::Thread(_) => trap(error),
        other => other.into(),
    }
}

fn arg_u32(args: &[RuntimeVal], index: usize) -> anyhow::Result<u32> {
    match args.get(index) {
        Some(RuntimeVal::I32(value)) => Ok(*value as u32),
        _ => Err(anyhow!("a thread built-in expected an i32 argument")),
    }
}
