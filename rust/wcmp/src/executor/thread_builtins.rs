// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

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
//! anything else, which is the reference's `Thread.resume` loop.
//!
//! The five are blocking built-ins. Under a provider each reaches the
//! guest as the switch module's shim for it, and a thread that runs
//! on a stack of its own, of a task that may block, suspends in the
//! shim through the provider. So does such a thread of an instance
//! that must not suspend when a block of its own instance's call
//! started or last resumed it, since its suspension hands control
//! back to that block. A thread of an instance that must not suspend
//! whose frame below is no such block, the thread of a host call into
//! a sync-typed export, suspends there too when the call names a
//! thread to switch to: a resume, or a promote whose thread is ready.
//! A switch waits on nothing, so the reference and Wasmtime suspend
//! that thread as well, and the frame that started or resumed it
//! takes it back once the thread it switched to stops. The shim
//! serves each kind of call this way:
//!
//! - A suspending built-in suspends the thread until a resume names
//!   it. `thread.resume-later` makes it ready, and it resumes in a
//!   later turn. A yielding built-in makes it ready at once, and it
//!   resumes after every other ready item, as a yield does.
//! - A switch names the thread to run next, and the frame that
//!   started or resumed the switching thread runs it once the
//!   switching thread has left the real stack: a turn's item, or a
//!   trampoline that made a nested start. A thread that has never run
//!   starts, and a thread suspended in the provider resumes, each on
//!   a stack of its own, and it can switch again in turn. A switch
//!   can also name the thread whose block the switching thread hands
//!   control back to. That thread waits on the real stack in the
//!   frame that runs the named thread, and the frame lets it go on.
//!
//! Every other thread blocks through the seam's nested turn, and a
//! switch runs from inside the built-in: each thread of a store with
//! no provider, and under a provider a thread that runs on another
//! thread's stack, or a thread of a task that must not block that
//! neither of the two cases above covers.
//!
//! - A suspension waits until a nested turn runs the work that
//!   resumes the thread. A task that must not block runs the ready
//!   work of its own instance alone, its threads suspended in the
//!   provider included, and then fails with the cannot-block cause,
//!   as for every other block.
//! - A switch to a thread that has never run starts that thread from
//!   inside the built-in, above it, as a nested start: on the real
//!   stack with no provider, and on a stack of its own under one. A
//!   switch to a thread suspended in the provider resumes it there,
//!   from inside the built-in. The store's stack of current scopes
//!   carries a thread-switch mark for as long as the thread runs from
//!   the built-in. Once it returns or suspends, a yielding built-in
//!   goes on, and a suspending one waits to be resumed.
//! - A switch to a thread that has run and is suspended, or waits, in
//!   a built-in of its own on the real stack cannot run: that
//!   built-in's frame lies below the current one. The switch fails
//!   with the stack-switch cause.
//!
//! An explicit thread runs in its task's scope, so a borrow it takes
//! counts against the task and `thread.index` answers the thread's
//! own index while it runs. It does not take the instance's exclusive
//! thread: only the implicit thread of a task lifted synchronously or
//! with a callback does. When its start function returns, the thread
//! leaves the instance's table and its task's list of threads. A
//! start function that traps ends the thread the same way. The trap
//! poisons the store and ends the driver whose turn ran the thread,
//! whichever task the thread belongs to. A switch runs the thread it
//! names from inside the built-in, so a trap of that thread fails the
//! built-in first and travels out from there to the driver.
//!
//! A task lives until its last thread ends, whether that thread has
//! started or not. Its implicit thread can return first: the task
//! then goes on with its explicit threads, one of them may still call
//! `task.return`, and the end of the last of them ends the task, with
//! the no-result failure for a task that has not resolved. A task that
//! fails ends with all its threads, and a start no turn has run yet
//! goes with it, under the store's rule for the items of a task that
//! ends.
//!
//! The five that suspend or switch can carry the `cancellable`
//! immediate. The reference removed it, but Wasmtime 49 still reads
//! it and honors it, and the C generator of wit-bindgen still emits
//! `[cancellable][thread-suspend]` and the four cancellable switches,
//! so the polyfill honors it as Wasmtime does. Each answers zero
//! without it, and never takes a cancellation request. With it:
//!
//! - The built-in first takes a request pending for the current
//!   thread's task, and answers 1 at once, before it reads the thread
//!   it names, suspends, or yields.
//! - Otherwise it goes on as it would, and answers 1 when a request is
//!   pending once the thread goes on, which it then takes.
//! - A thread that yields — a built-in that yields and then switches,
//!   or a promote that yields because the thread it names is not
//!   ready — is run first by `subtask.cancel` of its task while it is
//!   suspended in the provider, as Wasmtime runs a thread in a
//!   cancellable yield.
//! - A thread that suspends is not woken by `subtask.cancel`. It
//!   answers 1 when another thread resumes it while a request is still
//!   pending.
//!
//! Taking a request moves the task to cancel-delivered.

use std::sync::{Arc, Mutex};

use anyhow::anyhow;

use crate::abi::layout::FlatType;
use crate::abi::runtime_state::AbiRuntimeState;
use crate::concurrency::{
    BlockStep, BlockingBuiltin, InstanceId, Item, ItemKind, Readiness, SuspendSeam, TaskId,
    ThreadId, ThreadStart,
};
use crate::error::{Error, Result, SchedulerCause, TaskCause, ThreadCause};
use crate::executor::intrinsics::core_func_type;
use crate::executor::ir::{CoreParameter, CoreSignature};
use crate::internal::ErrorInternal;
use crate::resource::HandleTables;
use crate::runtime_layer::host_func;
use crate::runtime_layer::{AsContextMut, Func as RuntimeFunc, Val as RuntimeVal, call_failure};
use crate::store::StoreContext;
use crate::store::StoreContextInternalExt;

/// Build the `thread.index` built-in for `instance`: it returns the
/// current thread's index in the instance's thread table.
pub fn build_thread_index<T: 'static>(
    store: &mut StoreContext<'_, T>,
    instance: usize,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> crate::error::Result<RuntimeFunc> {
    let tables = store.internal().tables_handle();
    host_func(
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
) -> crate::error::Result<RuntimeFunc> {
    let tables = store.internal().tables_handle();
    let context_type = match signature.params.get(1) {
        Some(CoreParameter::Value(FlatType::I64)) => FlatType::I64,
        _ => FlatType::I32,
    };
    host_func(
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
                .map_err(trap)?
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
) -> crate::error::Result<RuntimeFunc> {
    let tables = store.internal().tables_handle();
    host_func(
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
/// The start function is the thread's entry, so the scheduler starts
/// it through the store's provider when there is one, on a stack of
/// its own, and the thread ends when the entry finishes. A trap of
/// the thread poisons the store and ends the turn, which reaches the
/// driver that is polling.
fn start_ready_thread<T: 'static>(store: &mut StoreContext<'_, T>, thread: ThreadId) -> Result<()> {
    store.internal().start_ready_thread(thread)
}

/// Run an explicit thread from its start to its end, on the real
/// stack, which is what a switch does with no provider.
///
/// The thread runs in its task's scope. Its end is the same whether
/// its start function returned or trapped: the scope it pushed is
/// popped, with whatever a failed call left above it, and the thread
/// leaves its instance's table and its task. A trap poisons the store
/// and comes back to the caller, the built-in that switched, which
/// fails with it and carries it out to the driver whose turn is
/// running, exactly as the same trap does under a provider. That
/// holds whichever task the thread belongs to, the switching thread's
/// or another. A thread that returned and was the last of its task,
/// `task`, after the task's implicit thread exited, ends the task,
/// and a failure of that end comes back the same way.
fn run_thread<T: 'static>(
    store: &mut StoreContext<'_, T>,
    thread: ThreadId,
    task: TaskId,
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
        .map_err(call_failure);
    {
        let mut guard = store.internal().lock_tables()?;
        guard.leave_thread(thread);
        guard.tasks.end_thread(thread);
    }
    if let Err(error) = outcome {
        store.internal().poison();
        return Err(error);
    }
    store.internal().end_last_thread(task)
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
/// thread suspends until a resume names it. `cancellable` is the
/// built-in's `cancellable` immediate.
pub fn build_thread_suspend<T: 'static>(
    store: &mut StoreContext<'_, T>,
    instance: usize,
    cancellable: bool,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> BlockingBuiltin<T> {
    let form = Suspension {
        yielding: false,
        switch: Switch::None,
        cancellable,
    };
    build_suspension(store, instance, signature, abi_state, form)
}

/// Build the `thread.suspend-then-resume` built-in for `instance`:
/// the current thread suspends and switches to the suspended thread
/// its argument names.
pub fn build_thread_suspend_then_resume<T: 'static>(
    store: &mut StoreContext<'_, T>,
    instance: usize,
    cancellable: bool,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> BlockingBuiltin<T> {
    let form = Suspension {
        yielding: false,
        switch: Switch::Resume,
        cancellable,
    };
    build_suspension(store, instance, signature, abi_state, form)
}

/// Build the `thread.yield-then-resume` built-in for `instance`: the
/// current thread becomes ready and switches to the suspended thread
/// its argument names.
pub fn build_thread_yield_then_resume<T: 'static>(
    store: &mut StoreContext<'_, T>,
    instance: usize,
    cancellable: bool,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> BlockingBuiltin<T> {
    let form = Suspension {
        yielding: true,
        switch: Switch::Resume,
        cancellable,
    };
    build_suspension(store, instance, signature, abi_state, form)
}

/// Build the `thread.suspend-then-promote` built-in for `instance`:
/// the current thread switches to the thread its argument names when
/// that thread is ready, and suspends otherwise.
pub fn build_thread_suspend_then_promote<T: 'static>(
    store: &mut StoreContext<'_, T>,
    instance: usize,
    cancellable: bool,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> BlockingBuiltin<T> {
    let form = Suspension {
        yielding: false,
        switch: Switch::Promote,
        cancellable,
    };
    build_suspension(store, instance, signature, abi_state, form)
}

/// Build the `thread.yield-then-promote` built-in for `instance`: the
/// current thread switches to the thread its argument names when that
/// thread is ready, and yields otherwise.
pub fn build_thread_yield_then_promote<T: 'static>(
    store: &mut StoreContext<'_, T>,
    instance: usize,
    cancellable: bool,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> BlockingBuiltin<T> {
    let form = Suspension {
        yielding: true,
        switch: Switch::Promote,
        cancellable,
    };
    build_suspension(store, instance, signature, abi_state, form)
}

/// Which of the five suspending built-ins one is.
#[derive(Clone, Copy)]
struct Suspension {
    /// Whether the current thread stays ready rather than suspended.
    yielding: bool,
    /// Which thread the built-in names.
    switch: Switch,
    /// Whether the built-in carries the `cancellable` immediate.
    cancellable: bool,
}

/// What a suspending built-in answers when it took no cancellation
/// request, which is Wasmtime's `WaitResult::Completed`.
const COMPLETED: i32 = 0;

/// What a cancellable suspending built-in answers when it took a
/// cancellation request, which is Wasmtime's `WaitResult::Cancelled`.
const CANCELLED: i32 = 1;

/// Build one of the five suspending built-ins, of the `form` given.
/// Each answers zero, as the reference's built-ins do, unless it
/// carries the `cancellable` immediate and takes a cancellation
/// request, as the module documentation states.
///
/// The built-in has two bodies. A thread that runs on a stack of its
/// own suspends in the built-in's shim, through [`begin_suspension`],
/// when its task may block or when the call names a thread to switch
/// to. Any other thread, and every thread of a store with no provider,
/// runs [`suspension`] on the real stack.
fn build_suspension<T: 'static>(
    _store: &mut StoreContext<'_, T>,
    instance: usize,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
    form: Suspension,
) -> BlockingBuiltin<T> {
    let on_real_stack = abi_state.clone();
    let switching = abi_state.clone();
    let switch = form.switch;
    let builtin = BlockingBuiltin::with_fallback(
        core_func_type(signature),
        move |store: &mut StoreContext<'_, T>, args: &[RuntimeVal]| {
            let id = calling_instance(&abi_state, instance)?;
            trap_if_cannot_leave(&abi_state, id, store.internal().runtime_mut())?;
            let Some(cancel) = cancel_first(store, form)? else {
                return Ok(BlockStep::Ready(vec![RuntimeVal::I32(CANCELLED)]));
            };
            let named = named_argument(switch, args);
            let begun = named
                .and_then(|named| begin_suspension(store, id, form, cancel, named).map_err(trap));
            if begun.is_err() {
                // The thread never suspended, and is in no yield.
                answer(store, cancel, false)?;
            }
            begun
        },
        move |store: &mut StoreContext<'_, T>, args: &[RuntimeVal]| {
            let id = calling_instance(&on_real_stack, instance)?;
            trap_if_cannot_leave(&on_real_stack, id, store.internal().runtime_mut())?;
            let Some(cancel) = cancel_first(store, form)? else {
                return Ok(Some(vec![RuntimeVal::I32(CANCELLED)]));
            };
            let suspended = named_argument(switch, args)
                .and_then(|named| suspension(store, id, form, named).map_err(trap));
            if !matches!(suspended, Ok(None)) {
                let answer = answer(store, cancel, suspended.is_ok())?;
                suspended?;
                return Ok(Some(vec![RuntimeVal::I32(answer)]));
            }
            // The rest of the suspension went to the scheduler as a
            // plan, and the built-in answers what the plan's wait
            // ended with once the thread resumes.
            SuspendSeam::park(
                store,
                BlockStep::wait(
                    Readiness::Planned,
                    move |store: &mut StoreContext<'_, T>, waited| {
                        let answer = answer(store, cancel, waited.is_ok())?;
                        waited.map_err(trap)?;
                        Ok(vec![RuntimeVal::I32(answer)])
                    },
                ),
            )?;
            Ok(None)
        },
    );
    if matches!(switch, Switch::None) {
        return builtin;
    }
    builtin.switching(
        move |store: &mut StoreContext<'_, T>, args: &[RuntimeVal]| {
            names_switch(store, &switching, instance, switch, args)
        },
    )
}

/// The cancellation part of a suspending built-in, before it reads
/// the thread it names: a cancellable built-in takes a request pending
/// for the current thread's task, and `None` then says it answers 1 at
/// once. Otherwise it answers the thread whose task a request is taken
/// for once the thread goes on — `None` inside for a built-in without
/// the immediate, which never takes one — and a cancellable yield
/// marks the thread, so that `subtask.cancel` runs it first.
fn cancel_first<T: 'static>(
    store: &mut StoreContext<'_, T>,
    form: Suspension,
) -> anyhow::Result<Option<Option<ThreadId>>> {
    if !form.cancellable {
        return Ok(Some(None));
    }
    let mut guard = store.internal().lock_tables()?;
    let Some(thread) = guard.tasks.current_thread() else {
        return Ok(Some(None));
    };
    if guard.tasks.take_pending_cancel_of(thread) {
        return Ok(None);
    }
    if form.yielding {
        guard.tasks.set_cancellable_yield(thread, true);
    }
    Ok(Some(Some(thread)))
}

/// What a suspending built-in answers once its thread goes on, as
/// [`cancel_first`] prepared it: `cancel` names the thread of a
/// cancellable built-in, which stops being marked as in a cancellable
/// yield, and takes a request still pending when `went_on` says the
/// wait did not fail.
fn answer<T: 'static>(
    store: &mut StoreContext<'_, T>,
    cancel: Option<ThreadId>,
    went_on: bool,
) -> anyhow::Result<i32> {
    let Some(thread) = cancel else {
        return Ok(COMPLETED);
    };
    let mut guard = store.internal().lock_tables()?;
    guard.tasks.set_cancellable_yield(thread, false);
    if went_on && guard.tasks.take_pending_cancel_of(thread) {
        return Ok(CANCELLED);
    }
    Ok(COMPLETED)
}

/// Whether one call of a switching built-in names a thread to switch
/// to: a resume, and a promote whose thread is ready. A call that
/// fails to find its thread answers `false`, and the fallback then
/// fails it with the cause it finds.
fn names_switch<T: 'static>(
    store: &mut StoreContext<'_, T>,
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    instance: usize,
    switch: Switch,
    args: &[RuntimeVal],
) -> bool {
    let (Ok(id), Ok(named)) = (
        calling_instance(abi_state, instance),
        named_argument(switch, args),
    ) else {
        return false;
    };
    let Ok(current) = current_thread(store) else {
        return false;
    };
    let Ok(below) = store
        .internal()
        .lock_tables()
        .map(|guard| guard.tasks.returns_to(current))
    else {
        return false;
    };
    matches!(
        switch_target(store, id, current, switch, named, below),
        Ok(Some(_))
    )
}

/// The index of the thread a switching built-in names, which is its
/// one argument.
fn named_argument(switch: Switch, args: &[RuntimeVal]) -> anyhow::Result<Option<u32>> {
    match switch {
        Switch::None => Ok(None),
        Switch::Resume | Switch::Promote => Ok(Some(arg_u32(args, 0)?)),
    }
}

/// The thread a suspending built-in switches to, or `None` when it
/// names none or a promote finds the named thread not ready.
///
/// `below` is the thread whose blocked built-in the current thread
/// goes back to when it suspends through the provider, when that is a
/// block of its own instance's call. That thread waits on the real
/// stack, in the frame the switch returns to, so the switch can name
/// it: the frame runs it once the current thread has suspended. A
/// switch that runs from inside the built-in, where the current
/// thread cannot suspend, passes `None`.
fn switch_target<T: 'static>(
    store: &mut StoreContext<'_, T>,
    instance: InstanceId,
    current: ThreadId,
    switch: Switch,
    named: Option<u32>,
    below: Option<ThreadId>,
) -> Result<Option<ThreadId>> {
    match (switch, named) {
        (Switch::Resume, Some(index)) => {
            Ok(Some(resume_target(store, instance, index, current, below)?))
        }
        (Switch::Promote, Some(index)) => promote_target(store, instance, index, current, below),
        _ => Ok(None),
    }
}

/// The current thread of `store`.
fn current_thread<T: 'static>(store: &mut StoreContext<'_, T>) -> Result<ThreadId> {
    store
        .internal()
        .lock_tables()?
        .tasks
        .current_thread()
        .ok_or_else(|| Error::internal("a thread built-in ran with no thread on the stack"))
}

/// The first part of a suspending built-in whose thread runs on a
/// stack of its own, once the may-leave check has passed: the
/// suspension the reference's `Thread.suspend`, `Thread.yield_`, and
/// `switch_to` make.
///
/// The thread suspends in the built-in's shim, through the provider,
/// and the part records what for:
///
/// - A suspending built-in suspends the thread: it waits on nothing
///   until a resume names it, and its shim asks
///   [`Readiness::Resumed`] each time it resumes.
///   `thread.resume-later` makes it ready, and it resumes in a later
///   turn, as a resumption after a yield. A switch that names it
///   resumes it at once.
/// - A yielding built-in makes the thread ready, as `thread.yield`
///   does, and it resumes in a later turn after every other ready
///   item.
/// - A switch names the thread to run next. The frame that started
///   or resumed the switching thread runs it before anything else,
///   once the switching thread has left the real stack: it starts a
///   thread that has never run, and resumes one suspended in the
///   provider. That is the reference's `Thread.resume` loop, and the
///   named thread runs on a stack of its own, so it can suspend and
///   switch in turn. A switch that names the thread whose block the
///   switching thread goes back to lets that thread go on instead:
///   it waits on the real stack, in the frame that runs the switch.
///
/// The finish part answers zero once the thread resumes, or 1 when
/// `cancel` names the thread of a cancellable built-in and a request
/// is pending then, and the trap of a wait that failed otherwise: the
/// deadlock cause when a driver found the store idle while the thread
/// was suspended.
fn begin_suspension<T: 'static>(
    store: &mut StoreContext<'_, T>,
    instance: InstanceId,
    form: Suspension,
    cancel: Option<ThreadId>,
    named: Option<u32>,
) -> Result<BlockStep<T>> {
    let current = current_thread(store)?;
    let below = store.internal().lock_tables()?.tasks.returns_to(current);
    let target = switch_target(store, instance, current, form.switch, named, below)?;
    let readiness = if form.yielding {
        Readiness::Yielded
    } else {
        store
            .internal()
            .lock_tables()?
            .tasks
            .suspend_thread(current)?;
        Readiness::Resumed { thread: current }
    };
    if let Some(target) = target {
        store.internal().scheduler_mut().name_next_thread(target);
    }
    Ok(BlockStep::wait(
        readiness,
        move |store: &mut StoreContext<'_, T>, waited| {
            let answer = answer(store, cancel, waited.is_ok())?;
            waited.map_err(trap)?;
            Ok(vec![RuntimeVal::I32(answer)])
        },
    ))
}

/// The body of a suspending built-in whose thread cannot suspend its
/// stack, once the may-leave check has passed: find the thread to
/// switch to, then suspend or yield through the seam's nested turn
/// with that switch. It answers `None` when the seam left the rest to
/// the scheduler as a plan.
fn suspension<T: 'static>(
    store: &mut StoreContext<'_, T>,
    instance: InstanceId,
    form: Suspension,
    named: Option<u32>,
) -> Result<Option<()>> {
    let current = current_thread(store)?;
    let target = switch_target(store, instance, current, form.switch, named, None)?;
    let run = move |store: &mut StoreContext<'_, T>| match target {
        Some(other) => start_switched(store, current, other),
        None => Ok(()),
    };
    match (form.yielding, target) {
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
/// own. One suspended in the provider waits on a stack of its own,
/// and the switch resumes it there. One suspended on the real stack
/// waits in a frame below the current one. When that frame is
/// `below`, the one the current thread goes back to as it suspends
/// through the provider, the frame runs the thread once the switch
/// has suspended the current one. Any other such thread cannot run
/// until the current frame returns, and the switch fails with the
/// stack-switch cause.
fn resume_target<T: 'static>(
    store: &mut StoreContext<'_, T>,
    instance: InstanceId,
    index: u32,
    current: ThreadId,
    below: Option<ThreadId>,
) -> Result<ThreadId> {
    let (thread, never_ran) = {
        let guard = store.internal().lock_tables()?;
        let thread = named_thread(&guard, instance, index)?;
        let record = guard.tasks.thread(thread).ok_or_else(|| {
            Error::internal("a thread table names a thread the store does not hold")
        })?;
        if thread == current || !record.suspended {
            return Err(Error::Thread(ThreadCause::NotSuspended));
        }
        (thread, record.start.is_some())
    };
    if !never_ran && !store.internal().scheduler().is_parked(thread) && below != Some(thread) {
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
/// - A thread suspended in the provider whose condition holds is
///   ready, and the switch resumes it on its own stack.
/// - The implicit thread of a callback task parked on a waitable set
///   is not on the stack. Its readiness also needs its instance's
///   exclusive thread free, which the recorded condition does not
///   say, so the promote suspends or yields instead, and the turns
///   of that wait run the callback when it is ready.
/// - Any other thread that waits is inside a built-in of its own on
///   the real stack, below the current frame. When it waits in
///   `below`, the frame the current thread goes back to as it
///   suspends through the provider, that frame runs it once the
///   switch has suspended the current thread. Any other such thread
///   cannot run until the current frame returns, and the promote
///   fails with the stack-switch cause.
fn promote_target<T: 'static>(
    store: &mut StoreContext<'_, T>,
    instance: InstanceId,
    index: u32,
    current: ThreadId,
    below: Option<ThreadId>,
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
    if startable
        || (ready && (store.internal().scheduler().is_parked(thread) || below == Some(thread)))
    {
        return Ok(Some(thread));
    }
    if ready && !store.internal().scheduler().holds_callback_of(thread) {
        return Err(Error::Scheduler(SchedulerCause::StackSwitchNeeded));
    }
    Ok(None)
}

/// Run `other` from inside the frame of the built-in `switching`
/// called, which is the switch of the reference's `Thread.resume`
/// loop made where the switching thread cannot suspend its stack.
///
/// With no provider, `other` has never run, and it runs on the real
/// stack above the built-in until it returns. Under a provider it
/// runs on a stack of its own, through the provider: a thread that
/// has never run starts, and one suspended in the provider resumes.
/// Either returns here once it suspends or finishes, after running
/// whatever it switched to in turn.
///
/// The stack carries a thread-switch mark for as long as the thread
/// runs from here, which the cause of a failed block above it reads.
/// The mark comes back off through an unwind too, as a start
/// intrinsic's nested-start mark does. A trap of the started thread
/// poisons the store and is the failure of the built-in that started
/// it, whichever task the thread belongs to, and it travels out from
/// there to the driver whose turn is running.
fn start_switched<T: 'static>(
    store: &mut StoreContext<'_, T>,
    switching: ThreadId,
    other: ThreadId,
) -> Result<()> {
    let provider = store.internal().provider().is_some();
    let tables = store.internal().tables_handle();
    let start = {
        let mut guard = tables
            .lock()
            .map_err(|_| Error::internal("resource handle tables lock poisoned"))?;
        let start = if provider {
            None
        } else {
            let started = guard
                .tasks
                .take_thread_start(other)
                .ok_or_else(|| Error::internal("a switch named a thread with nothing to start"))?;
            Some(started)
        };
        guard.tasks.begin_thread_switch(switching);
        start
    };
    let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match start {
        Some((task, start)) => run_thread(store, other, task, start),
        None => store.internal().run_switched_thread(other),
    }));
    // A thread this runs that has to resume a thread it cannot resume
    // from here leaves that to the store, and the switch goes on once
    // the store has done it. The mark comes off then.
    if matches!(ran, Ok(Ok(()))) && store.internal().defers_work() {
        store
            .internal()
            .scheduler_mut()
            .deferred_mut()
            .ends_thread_switch = true;
        return Ok(());
    }
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

/// The trap a structured error becomes on its way to the guest: the
/// error itself, as the trap's error. The runtime layer hands it back
/// unchanged, so the call into the guest gets the error back as it was
/// raised (see `call_failure`), and its message, which the conformance
/// corpora match by substring, is the error's own.
fn trap(error: Error) -> anyhow::Error {
    anyhow::Error::from(error)
}

fn arg_u32(args: &[RuntimeVal], index: usize) -> anyhow::Result<u32> {
    match args.get(index) {
        Some(RuntimeVal::I32(value)) => Ok(*value as u32),
        _ => Err(anyhow!("a thread built-in expected an i32 argument")),
    }
}
