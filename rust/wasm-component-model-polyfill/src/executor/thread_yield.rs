//! The `thread.yield` built-in.
//!
//! `thread.yield` gives way and returns zero. The reference treats a
//! yield as a point where any other ready thread can run, and
//! Wasmtime switches to a ready thread of the instance when one
//! exists. The built-in traps with the cannot-leave cause when the
//! instance's may-leave flag is clear, which is the case while a
//! `realloc` or a `post-return` of that instance runs.
//!
//! Giving way is one call into the suspend seam: the built-in asks
//! it to suspend the current thread until a condition that holds as
//! soon as the thread has been given back control, which is what a
//! yield waits for and no more. On a target with no provider the
//! seam runs one nested turn under that condition — the ready work
//! of the store, or of the calling task's own instance alone when
//! that task must not block — and the built-in returns. A task that
//! must not block with no ready work of its own instance therefore
//! yields to nothing, as Wasmtime runs it.
//!
//! The seam's causes do not reach the built-in. The condition holds
//! by the time the fallback's loop ends, however the loop ended, so
//! the check that stands between the loop and the cannot-block,
//! deadlock and stack-switch causes always succeeds. The built-in
//! swallows them all the same, because any one of them would mean
//! the same thing here: that the turn did not meet a condition a
//! yield never asked it to meet.
//!
//! An error the nested turn raised while it ran an item is a
//! different thing: it is the failure of that work, it would have
//! failed the turn that ran it, and the built-in hands it on rather
//! than losing it.
//!
//! ## One shape does fail, and it is the polyfill's own rule
//!
//! The reference's `canon_thread_yield` has one trap, may-leave, and
//! otherwise always answers `[0]`; Wasmtime has no counterpart trap
//! at all. This built-in departs from that in one case, and the
//! departure is the polyfill's own — no design document states it,
//! and whether it becomes a stated rule is the design's to decide.
//!
//! The case is a thread that gives way more times over than the
//! budget the scheduler names `SPIN_BUDGET`, with all of:
//!
//! - nothing of the store's having run between any two of them, so
//!   every one of those yields gave way to nothing;
//! - the store holding nothing at all at each of them — no ready
//!   item, no resumption, no task at a gate, no callback held for an
//!   event, and no host future that can still resolve — so nothing
//!   of the store's could have run either;
//! - a guest frame of another task below this one on the stack.
//!
//! Such a thread is spin-waiting for that frame. The store has
//! nothing to give it and cannot come by anything on its own, and
//! the one thread that could release it is the caller whose frame
//! the polyfill cannot leave without a stack switch. The yield past
//! the budget therefore fails with the stack-switch cause rather
//! than returning zero for ever. One corpus directive depends on it,
//! and without it that directive runs for ever rather than failing.
//!
//! Each half of the rule is there to keep a thread that is being
//! served out of it. A yield with a store item run between it and
//! the one before gave way to something, so it starts the count
//! over, and so does a yield taken while the store still holds a
//! host future that wants another poll. What is left is the guest's
//! own progress between two yields, which nothing here can see: a
//! loop that gives way past the budget and then returns is cut short
//! by this rule, and nothing short of running the guest to its end
//! could tell it from one that never returns. That is what makes the
//! bound a budget rather than a proof, and why the number is drawn
//! generously.
//!
//! An item the nested turn runs that calls the built-in again opens
//! a nested turn one frame further down, because nested turns nest.
//! Such a yield gives way to whatever the level above it has not
//! reached, which is often nothing, and returns zero either way.
//!
//! From inside a guest frame with no stack switch the polyfill
//! cannot hand control to the host executor. A yield here therefore
//! runs a resumption the low-priority queue holds where a driver's
//! turn would first give the executor its turn: that is the nested
//! turn's rule, and it is what keeps a task blocked on such a
//! resumption from waiting for ever. A callback task that wants the
//! executor to run returns the yield status word instead.
//!
//! The `cancellable` immediate the translator drops has no effect
//! here, and it is not the reference's. `canon thread.yield` takes
//! no immediate there and `canon_thread_yield` answers `[0]`
//! whatever the caller is; the Explainer says of the returned `i32`
//! that it is always zero and may be removed in a later ABI
//! revision. The immediate is a field of the trampoline IR of the
//! Wasmtime release this crate reads components with, where it
//! marks a caller that may be told a cancellation is pending, and
//! the release after it has dropped the field. Nothing in this
//! design makes a cancellation pending either way.

use std::sync::{Arc, Mutex};

use anyhow::anyhow;
use wasm_runtime_layer::{
    AsContextMut, Func as RuntimeFunc, StoreContextMut as RuntimeContextMut, Val as RuntimeVal,
};

use crate::abi::runtime_state::AbiRuntimeState;
use crate::backend::Backend;
use crate::concurrency::{InstanceId, Scope, SuspendSeam};
use crate::error::{Error, SchedulerCause, TaskCause};
use crate::executor::intrinsics::core_func_type;
use crate::executor::ir::CoreSignature;
use crate::resource::HandleTables;
use crate::store::{StoreContext, StoreData};

/// The value `thread.yield` returns. `canon_thread_yield` answers
/// `[0]` and the Explainer says the word is always zero, so the
/// built-in answers zero.
const ALWAYS_ZERO: i32 = 0;

/// Build the `thread.yield` built-in for `instance`, the
/// translator's per-instantiation index of the component instance
/// that calls it. The guest imports the built-in directly, so the
/// instance comes from the trampoline rather than from an argument,
/// and the built-in takes nothing and returns one word.
pub fn build_thread_yield<T: 'static>(
    store: &mut StoreContext<'_, T>,
    instance: usize,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> RuntimeFunc {
    let tables = store.tables_handle();
    RuntimeFunc::new(
        store.runtime_mut(),
        core_func_type(signature),
        move |store_ctx, _args, results| {
            thread_yield(store_ctx, &abi_state, &tables, instance)?;
            results[0] = RuntimeVal::I32(ALWAYS_ZERO);
            Ok(())
        },
    )
}

/// The body of the built-in: refuse the call the instance may not be
/// left for, then give way once.
fn thread_yield<T: 'static>(
    mut store_ctx: RuntimeContextMut<'_, StoreData<T>, Backend>,
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    tables: &Arc<Mutex<HandleTables>>,
    instance: usize,
) -> anyhow::Result<()> {
    let id = calling_instance(abi_state, instance)?;
    trap_if_cannot_leave(tables, id)?;

    // The condition is false the first time the seam asks and true
    // afterwards: the thread waits for one chance to be given back
    // control and nothing else. The seam therefore runs exactly one
    // nested turn on a target with no provider, and a provider that
    // switches stacks suspends the thread exactly once.
    let mut given_back = false;
    let mut store = StoreContext::new(store_ctx.as_context_mut());
    match SuspendSeam::suspend(&mut store, |_| std::mem::replace(&mut given_back, true)) {
        Ok(()) => {}
        // A cause the seam raised says the turn did not meet a
        // condition a yield never asked it to meet, so it is not a
        // failure of the yield; see the module documentation.
        Err(Error::Scheduler(_)) => {}
        Err(error) => return Err(trap(error)),
    }
    if spinning_for_its_caller(&mut store, tables)? {
        return Err(trap(Error::Scheduler(SchedulerCause::StackSwitchNeeded)));
    }
    Ok(())
}

/// Whether this yield is one past the budget in a run of yields the
/// same thread took against a store that held nothing, made from a
/// task with a guest frame of another task below it on the stack.
///
/// Both halves are needed, and the scheduler keeps the first of
/// them: [`Scheduler::note_yield`] counts the run and says when it
/// has gone past the budget, which means every one of those yields
/// gave way to nothing and nothing of the store's could have run
/// between them. The frame below says who is left to release the
/// thread: a task the polyfill reached from another task's frame can
/// only go on once that frame does, and leaving it needs a stack
/// switch. A task the host called has no such frame, and a guest
/// loop of its own is the host's to bound.
///
/// [`Scheduler::note_yield`]: crate::concurrency::Scheduler::note_yield
fn spinning_for_its_caller<T: 'static>(
    store: &mut StoreContext<'_, T>,
    tables: &Arc<Mutex<HandleTables>>,
) -> anyhow::Result<bool> {
    let (thread, caller_below) = {
        let guard = tables
            .lock()
            .map_err(|_| anyhow!("resource handle tables lock poisoned"))?;
        let current = guard.tasks.current_task();
        let caller_below = guard.tasks.scopes().iter().any(|scope| match scope {
            Scope::Task(task) => Some(*task) != current,
            Scope::Subtask(_) => false,
        });
        (guard.tasks.current_thread(), caller_below)
    };
    let Some(thread) = thread else {
        return Ok(false);
    };
    let past_budget = store.scheduler_mut().note_yield(thread);
    Ok(past_budget && caller_below)
}

/// The store-wide identity of the component instance the translator
/// named for the built-in.
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
/// runs.
fn trap_if_cannot_leave(
    tables: &Arc<Mutex<HandleTables>>,
    instance: InstanceId,
) -> anyhow::Result<()> {
    let guard = tables
        .lock()
        .map_err(|_| anyhow!("resource handle tables lock poisoned"))?;
    let record = guard
        .tasks
        .instance(instance)
        .ok_or_else(|| anyhow!("a built-in named an instance the store does not hold"))?;
    if record.may_leave {
        return Ok(());
    }
    Err(trap(Error::Task(TaskCause::CannotLeave)))
}

/// The trap a structured error becomes on its way to the guest. The
/// message is the error's own, which the conformance corpora match
/// by substring, with the `wasm trap:` prefix a trap reaching guest
/// code renders with for a scheduler cause. The stack-switch cause
/// this built-in raises is the polyfill's own and has no Wasmtime
/// trap code behind it; it takes the prefix because it reaches the
/// guest as a trap all the same.
fn trap(error: Error) -> anyhow::Error {
    match error {
        Error::Scheduler(cause) => anyhow!("wasm trap: {cause}"),
        other => anyhow!("{other}"),
    }
}
