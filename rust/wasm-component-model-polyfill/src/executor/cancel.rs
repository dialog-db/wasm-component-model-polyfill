//! The `task.cancel` and `subtask.cancel` built-ins.
//!
//! Cancellation is not built. The cancelled event is never delivered
//! and no subtask is ever cancelled, so a guest that calls either
//! built-in fails with [`Error::Unsupported`]. The translator still
//! accepts both, which is the one exception to its rule of refusing
//! what is not built. The binding layer of the Rust toolchain links
//! `task.cancel` in every `async` export and `subtask.cancel` in
//! every awaited import. Refusing the built-ins at translation would
//! refuse every such guest, where accepting them lets the guest
//! instantiate and run every path that does not cancel.
//!
//! The may-leave check comes first, as it does for every other
//! built-in: a call while the instance's may-leave flag is clear,
//! which is the case while a `realloc` or a `post-return` of that
//! instance runs, fails with the cannot-leave cause. Only a call the
//! instance may be left for reaches the unsupported failure.
//!
//! Both failures travel as the structured error itself rather than
//! as its message, so the call the guest is inside fails with a
//! substrate failure a host can read the cause back out of.

use std::sync::{Arc, Mutex};

use anyhow::anyhow;
use wasm_runtime_layer::{AsContextMut, Func as RuntimeFunc};

use crate::abi::runtime_state::AbiRuntimeState;
use crate::concurrency::InstanceId;
use crate::error::{Error, TaskCause};
use crate::executor::intrinsics::core_func_type;
use crate::executor::ir::CoreSignature;
use crate::internal::ErrorInternal;
use crate::store::StoreContext;
use crate::store::StoreContextInternalExt;

/// Build the `task.cancel` built-in for `instance`, the translator's
/// per-instantiation index of the component instance that calls it.
pub fn build_task_cancel<T: 'static>(
    store: &mut StoreContext<'_, T>,
    instance: usize,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> RuntimeFunc {
    build_refusal(
        store,
        "task cancellation (`task.cancel`)",
        instance,
        signature,
        abi_state,
    )
}

/// Build the `subtask.cancel` built-in for `instance`, the
/// translator's per-instantiation index of the component instance
/// that calls it. The subtask index the guest passes is never read.
pub fn build_subtask_cancel<T: 'static>(
    store: &mut StoreContext<'_, T>,
    instance: usize,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> RuntimeFunc {
    build_refusal(
        store,
        "subtask cancellation (`subtask.cancel`)",
        instance,
        signature,
        abi_state,
    )
}

/// A built-in that checks the may-leave flag of `instance` and then
/// fails with [`Error::Unsupported`] naming `feature`. It writes no
/// result, because it never returns.
fn build_refusal<T: 'static>(
    store: &mut StoreContext<'_, T>,
    feature: &'static str,
    instance: usize,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> RuntimeFunc {
    RuntimeFunc::new(
        store.internal().runtime_mut(),
        core_func_type(signature),
        move |mut store_ctx, _args, _results| {
            let id = calling_instance(&abi_state, instance)?;
            trap_if_cannot_leave(&abi_state, id, &mut store_ctx)?;
            Err(Error::unsupported(feature).into())
        },
    )
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
