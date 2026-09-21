//! The `thread.yield` built-in.
//!
//! `thread.yield` gives way and returns zero. The reference treats a
//! yield as a point where any other ready thread can run, and
//! Wasmtime switches to a ready thread of the instance when one
//! exists. The built-in traps with the cannot-leave cause when the
//! instance's may-leave flag is clear, which is the case while a
//! `realloc` or a `post-return` of that instance runs.
//!
//! Giving way is one call into the suspend seam. The built-in asks
//! it for one chance to be given back control, which is what a
//! yield waits for and no more. On a target with no provider the
//! seam runs one nested turn — the ready work of the store, or of
//! the calling task's own instance alone when that task must not
//! block — and the built-in returns. A task that must not block
//! with no ready work of its own instance therefore yields to
//! nothing, as Wasmtime runs it.
//!
//! The built-in has no rule of its own that fails, which is what
//! the reference states: `canon_thread_yield` has the may-leave
//! trap and otherwise always answers `[0]`, and Wasmtime has no
//! counterpart trap at all. The built-in returns zero whenever it
//! returns.
//!
//! Two things can stop it returning, and neither is the yield's own
//! rule. An error the nested turn raised while it ran an item is
//! the failure of that work: it would have failed the turn that ran
//! it, and the built-in hands it on rather than losing it. And the
//! suspend seam keeps one budget over the nested turns the store
//! does not serve, which the seam's own documentation states. A
//! thread that gives way over and over against a store that holds
//! nothing is spin-waiting for a guest frame on the real stack that
//! only a stack switch could resume, so past the budget the seam
//! gives up and the call the thread is inside fails with the
//! stack-switch cause. One corpus directive depends on it, and
//! without it that directive runs for ever rather than failing.
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
use crate::concurrency::{InstanceId, SuspendSeam};
use crate::error::{Error, TaskCause};
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

    // One chance to be given back control, which is the whole of
    // what a yield waits for. The seam runs exactly one nested turn
    // on a target with no provider, and a provider that switches
    // stacks suspends the thread exactly once. What comes back is
    // the failure of an item the turn ran, or the seam's budget
    // ending the call this thread is inside; see the module
    // documentation.
    let mut store = StoreContext::new(store_ctx.as_context_mut());
    SuspendSeam::give_way(&mut store).map_err(trap)
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
/// the seam's budget raises is the polyfill's own and has no
/// Wasmtime trap code behind it; it takes the prefix because it
/// reaches the guest as a trap all the same.
fn trap(error: Error) -> anyhow::Error {
    match error {
        Error::Scheduler(cause) => anyhow!("wasm trap: {cause}"),
        other => anyhow!("{other}"),
    }
}
