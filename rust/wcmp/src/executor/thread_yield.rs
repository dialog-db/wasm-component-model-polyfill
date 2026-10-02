// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The `thread.yield` built-in.
//!
//! `thread.yield` gives way and returns zero, unless it carries the
//! `cancellable` immediate and takes a cancellation request, as the
//! last paragraph states. The reference treats a yield as a point
//! where any other ready thread can run, and Wasmtime switches to a
//! ready thread of the instance when one exists. The built-in traps
//! with the cannot-leave cause when the instance's may-leave flag is
//! clear, which is the case while a `realloc` or a `post-return` of
//! that instance runs.
//!
//! Giving way is one call into the suspend seam. The built-in asks
//! it for one chance to be given back control, which is what a
//! yield waits for and no more. Under a provider a thread that runs
//! on a stack of its own suspends in the built-in's shim, and it
//! resumes as a resumption after a yield: after every other ready
//! item, and once a driver has given the host executor its turn.
//! Otherwise the seam runs one nested turn — the ready work of the
//! store, or of the calling task's own instance alone when that task
//! must not block — and the built-in returns. A task that must not
//! block with no ready work of its own instance therefore yields to
//! nothing, as Wasmtime runs it.
//!
//! The built-in has no rule of its own that fails, which is what
//! the reference states: `canon_thread_yield` has the may-leave
//! trap and otherwise always answers `[0]`, and Wasmtime has no
//! counterpart trap at all. A yield without the `cancellable`
//! immediate returns zero whenever it returns.
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
//! The `cancellable` immediate changes what the built-in answers. The
//! reference removed it, and `canon_thread_yield` there always answers
//! `[0]`, but Wasmtime 49 still reads it and honors it, and the C
//! generator of wit-bindgen still emits `[cancellable][thread-yield]`,
//! so the polyfill honors it as Wasmtime does. A cancellable yield
//! first takes a cancellation request pending for the calling
//! thread's task, and answers 1 at once without giving way. Otherwise
//! it gives way, and answers 1 when a request is pending once it goes
//! on, which it then takes; `subtask.cancel` of its task runs a thread
//! that gives way in a cancellable yield, suspended in the provider,
//! before anything else. Taking the request moves the task to
//! cancel-delivered. A yield without the immediate never takes a
//! request, and answers zero whenever it returns.

use std::sync::{Arc, Mutex};

use anyhow::anyhow;

use crate::abi::runtime_state::AbiRuntimeState;
use crate::concurrency::{BlockStep, BlockingBuiltin, InstanceId, Readiness};
use crate::error::{Error, TaskCause};
use crate::executor::intrinsics::core_func_type;
use crate::executor::ir::CoreSignature;
use crate::resource::HandleTables;
use crate::runtime_layer::{AsContextMut, Val as RuntimeVal};
use crate::store::StoreContext;
use crate::store::StoreContextInternalExt;

/// The value `thread.yield` returns when it takes no cancellation
/// request, which is Wasmtime's `WaitResult::Completed`.
/// `canon_thread_yield` answers `[0]` and the Explainer says the word
/// is always zero, so a yield without the `cancellable` immediate
/// answers it whenever it returns.
const COMPLETED: i32 = 0;

/// The value a cancellable yield returns when it took a cancellation
/// request, which is Wasmtime's `WaitResult::Cancelled`.
const CANCELLED: i32 = 1;

/// Build the `thread.yield` built-in for `instance`, the
/// translator's per-instantiation index of the component instance
/// that calls it. The guest imports the built-in directly, so the
/// instance comes from the trampoline rather than from an argument,
/// and the built-in takes nothing and returns one word. `cancellable`
/// is the built-in's `cancellable` immediate.
pub fn build_thread_yield<T: 'static>(
    _store: &mut StoreContext<'_, T>,
    instance: usize,
    cancellable: bool,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> BlockingBuiltin<T> {
    BlockingBuiltin::new(
        core_func_type(signature),
        move |store: &mut StoreContext<'_, T>, _args: &[RuntimeVal]| {
            begin_thread_yield(store, &abi_state, instance, cancellable)
        },
    )
}

/// The first part of the built-in: refuse the call the instance may
/// not be left for, take a pending cancellation request when the
/// yield is cancellable, and otherwise give way once.
///
/// One chance to be given back control is the whole of what a yield
/// waits for, so it waits on a condition that always holds. With no
/// provider the seam runs exactly one nested turn, and a provider
/// that switches stacks suspends the thread exactly once and resumes
/// it after every other ready item. What the wait can fail with is
/// the failure of an item the turn ran, or the seam's budget ending
/// the call this thread is inside; see the module documentation.
///
/// A cancellable yield marks its thread while it gives way, which is
/// what lets `subtask.cancel` run the thread first, and takes a
/// request pending once it goes on.
#[tracing::instrument(level = "trace", name = "thread.yield", skip_all)]
fn begin_thread_yield<T: 'static>(
    store: &mut StoreContext<'_, T>,
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    instance: usize,
    cancellable: bool,
) -> anyhow::Result<BlockStep<T>> {
    let id = calling_instance(abi_state, instance)?;
    trap_if_cannot_leave(abi_state, id, store.internal().runtime_mut())?;
    let tables = store.internal().tables_handle();
    let thread = {
        let mut guard = lock_tables(&tables)?;
        let thread = guard.tasks.current_thread().filter(|_| cancellable);
        if let Some(thread) = thread {
            if guard.tasks.take_pending_cancel_of(thread) {
                return Ok(BlockStep::Ready(vec![RuntimeVal::I32(CANCELLED)]));
            }
            guard.tasks.set_cancellable_yield(thread, true);
        }
        thread
    };
    Ok(BlockStep::wait(
        Readiness::Yielded,
        move |_store: &mut StoreContext<'_, T>, waited| {
            let cancelled = match thread {
                Some(thread) => {
                    let mut guard = lock_tables(&tables)?;
                    guard.tasks.set_cancellable_yield(thread, false);
                    waited.is_ok() && guard.tasks.take_pending_cancel_of(thread)
                }
                None => false,
            };
            waited.map_err(trap)?;
            Ok(vec![RuntimeVal::I32(if cancelled {
                CANCELLED
            } else {
                COMPLETED
            })])
        },
    ))
}

/// Lock the store's handle tables and record state.
fn lock_tables(
    tables: &Arc<Mutex<HandleTables>>,
) -> anyhow::Result<std::sync::MutexGuard<'_, HandleTables>> {
    tables
        .lock()
        .map_err(|_| anyhow!("resource handle tables lock poisoned"))
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
