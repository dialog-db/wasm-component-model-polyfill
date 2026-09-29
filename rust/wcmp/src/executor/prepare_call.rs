//! The `prepare-call` intrinsic of a fused adapter.
//!
//! With the fused adapter compiler of Wasmtime 49, a call between
//! two components takes one of two paths. When the lower and the
//! lift are both synchronous, the adapter calls the enter and exit
//! intrinsics and nothing else. When either is asynchronous, the
//! adapter calls this intrinsic and then one of the two start
//! intrinsics.
//!
//! Prepare creates the records the call runs on and returns nothing.
//! It creates the callee's task, with the lift options the adapter
//! describes; it creates the subtask the caller owns, in its
//! starting state; and it records on the subtask the two functions
//! the adapter generated for the call, the caller's flat arguments,
//! the caller's thread, and how the caller takes its result. The
//! start intrinsic that follows picks the call up from the store and
//! runs it.
//!
//! The adapter passes the call as eight fixed arguments followed by
//! its own flat arguments:
//!
//! 1. the start function, as a `funcref`,
//! 2. the return function, as a `funcref`,
//! 3. the caller's component instance,
//! 4. the callee's component instance,
//! 5. the interned index of the result tuple the callee's lift
//!    declared, which the callee's `task.return` must name too,
//! 6. whether the callee's function type is `async`,
//! 7. the string encoding the callee's lift declared,
//! 8. how the caller takes its result: the count of its flat
//!    results, or one of the two sentinels of an asynchronous lower.
//!
//! The memory the callee's lift declared is not an argument: it is
//! fixed per adapter, so the translator records it on the spec.

use std::sync::{Arc, Mutex};

use wasm_runtime_layer::{Func as RuntimeFunc, Val as RuntimeVal};
use wasmtime_environ::component::{PREPARE_ASYNC_NO_RESULT, PREPARE_ASYNC_WITH_RESULT};

use crate::abi::runtime_state::AbiRuntimeState;
use crate::concurrency::{CallBridge, CallerKind, InstanceId, SubtaskId, ThreadId};
use crate::error::{Error, Result};
use crate::executor::intrinsics::core_func_type;
use crate::executor::ir::{CanonOptions, CoreSignature, DataModel, StringEncoding};
use crate::internal::ErrorInternal;
use crate::resource::{HandleTables, TableId};
use crate::store::StoreContext;
use crate::store::StoreContextInternalExt;

/// How many arguments of the prepare intrinsic describe the call.
/// Everything past them is the caller's own flat arguments.
const FIXED_ARGUMENTS: usize = 8;

/// Build the `prepare-call` intrinsic of one fused adapter.
/// `memory` is the runtime memory slot the callee's lift declared,
/// which the translator read off the trampoline.
pub fn build_prepare_call<T: 'static>(
    store: &mut StoreContext<'_, T>,
    memory: Option<usize>,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> RuntimeFunc {
    let tables = store.internal().tables_handle();
    RuntimeFunc::new(
        store.internal().runtime_mut(),
        core_func_type(signature),
        move |_store_ctx, args, _results| Ok(prepare_call(&tables, &abi_state, memory, args)?),
    )
}

/// The body of one call of the intrinsic.
fn prepare_call(
    tables: &Arc<Mutex<HandleTables>>,
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    memory: Option<usize>,
    args: &[RuntimeVal],
) -> Result<()> {
    let start = funcref_argument(args, 0)?;
    let return_ = funcref_argument(args, 1)?;
    let caller_index = u32_argument(args, 2)? as usize;
    let callee_index = u32_argument(args, 3)? as usize;
    let result_tuple = u32_argument(args, 4)? as usize;
    let callee_async_typed = u32_argument(args, 5)? != 0;
    let string_encoding = string_encoding(u32_argument(args, 6)?)?;
    let caller = caller_kind(u32_argument(args, 7)?);
    let arguments = args
        .get(FIXED_ARGUMENTS..)
        .ok_or_else(|| Error::internal("the prepare intrinsic was called with too few arguments"))?
        .to_vec();

    let callee = instance_at(abi_state, callee_index)?;
    // The caller's instance is named here and nowhere else, so its
    // handle table is resolved here: an asynchronous start puts the
    // subtask's entry in that table, and by then the adapter has
    // stopped saying whose call it is.
    let caller_table = handle_table_at(abi_state, caller_index)?;
    let options = CanonOptions {
        instance: callee_index,
        memory,
        // The adapter calls the callee's `cabi_realloc` itself, from
        // the start and return functions it generated, and a
        // synchronously lifted callee's post-return comes with the
        // asynchronous start intrinsic rather than with the
        // preparation. Neither slot is the polyfill's to fill here.
        realloc: None,
        post_return: None,
        // Whether the callee's lift is asynchronous is not known
        // yet: the adapter says so when it starts the call. Until
        // then the task carries the lift's other options alone.
        async_: false,
        callback: None,
        string_encoding,
        data_model: DataModel::LinearMemory,
    };

    let mut guard = tables
        .lock()
        .map_err(|_| Error::internal("resource handle tables lock poisoned"))?;
    let caller_thread: ThreadId = guard
        .tasks
        .current_thread()
        .ok_or_else(|| Error::internal("an adapter prepared a call with no task on the stack"))?;
    // The callee's task, its implicit thread, and the caller's
    // subtask are three records against the store's cap, and the cap
    // is asked for all three at once, so a call past it creates none.
    guard.tasks.admit_records(3)?;
    let task = guard
        .tasks
        .create_task(None, Some(Arc::new(options)), callee)?;
    if let Some(record) = guard.tasks.task_mut(task) {
        record.result_tuple = Some(result_tuple);
    }
    // The subtask is the caller's record of the call. It is not
    // pushed as a scope: a synchronous lower keeps its own task on
    // the stack while the callee runs, and an asynchronous lower
    // hands the record back to the caller to wait on.
    let subtask: SubtaskId = guard.tasks.insert_subtask()?;
    if let Some(record) = guard.tasks.subtask_mut(subtask) {
        record.callee = Some(task);
        record.bridge = Some(CallBridge {
            start,
            arguments,
            return_,
            caller,
            callee_async_typed,
            caller_thread,
            caller_table,
            caller_results: Vec::new(),
            flat_results: Vec::new(),
        });
    }
    if let Some(record) = guard.tasks.task_mut(task) {
        record.subtask = Some(subtask);
    }
    guard.tasks.prepare_call(subtask);
    Ok(())
}

/// How the caller takes its result, from the number the adapter
/// passes as the last fixed argument.
fn caller_kind(word: u32) -> CallerKind {
    match word {
        PREPARE_ASYNC_NO_RESULT => CallerKind::Async { has_result: false },
        PREPARE_ASYNC_WITH_RESULT => CallerKind::Async { has_result: true },
        flat_results => CallerKind::Sync { flat_results },
    }
}

/// The string encoding the callee's lift declared, from the number
/// the adapter passes. The numbers are the translator's own
/// discriminants.
fn string_encoding(word: u32) -> Result<StringEncoding> {
    Ok(match word {
        0 => StringEncoding::Utf8,
        1 => StringEncoding::Utf16,
        2 => StringEncoding::CompactUtf16,
        other => {
            return Err(Error::internal(format!(
                "an adapter named string encoding {other}"
            )));
        }
    })
}

/// One `funcref` argument, which an adapter never passes as null.
fn funcref_argument(args: &[RuntimeVal], index: usize) -> Result<RuntimeFunc> {
    match args.get(index) {
        Some(RuntimeVal::FuncRef(Some(func))) => Ok(func.clone()),
        Some(RuntimeVal::FuncRef(None)) => Err(Error::internal(
            "an adapter passed a null function reference to the prepare intrinsic",
        )),
        _ => Err(Error::internal(
            "the prepare intrinsic expected a `funcref` argument",
        )),
    }
}

/// One `i32` argument, read as the unsigned number the adapter
/// encoded.
pub fn u32_argument(args: &[RuntimeVal], index: usize) -> Result<u32> {
    match args.get(index) {
        Some(RuntimeVal::I32(value)) => Ok(*value as u32),
        _ => Err(Error::internal(
            "an adapter intrinsic expected an i32 argument",
        )),
    }
}

/// The handle table of the component instance the adapter names by
/// the translator's per-instantiation index.
fn handle_table_at(abi_state: &Arc<Mutex<AbiRuntimeState>>, index: usize) -> Result<TableId> {
    abi_state
        .lock()
        .map_err(|_| Error::internal("ABI runtime state lock poisoned"))?
        .handle_tables
        .get(index)
        .copied()
        .ok_or_else(|| {
            Error::internal(format!(
                "adapter named component instance {index}, which has no handle table"
            ))
        })
}

/// The store-wide identity of the component instance the adapter
/// names by the translator's per-instantiation index.
fn instance_at(abi_state: &Arc<Mutex<AbiRuntimeState>>, index: usize) -> Result<InstanceId> {
    let state = abi_state
        .lock()
        .map_err(|_| Error::internal("ABI runtime state lock poisoned"))?;
    state
        .component_instances
        .get(index)
        .copied()
        .ok_or_else(|| {
            Error::internal(format!(
                "adapter named component instance {index}, which this instantiation does not hold"
            ))
        })
}

/// The result of a prepared call, once the start intrinsic has taken
/// it out of the store: the subtask the caller owns.
pub fn take_prepared_call(tables: &Arc<Mutex<HandleTables>>) -> Result<SubtaskId> {
    tables
        .lock()
        .map_err(|_| Error::internal("resource handle tables lock poisoned"))?
        .tasks
        .take_prepared_call()
        .ok_or_else(|| Error::internal("an adapter started a call it had not prepared"))
}
