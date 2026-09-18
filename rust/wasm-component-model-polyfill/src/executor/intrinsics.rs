//! Intrinsics that adapter modules import.
//!
//! When a component contains other components, the translator's
//! fused adapter compiler emits one core module per cross-component
//! call site. The adapter lifts arguments from the caller's memory
//! and lowers them into the callee's memory in core Wasm. It imports
//! a small set of host functions for the work it cannot do inline:
//!
//! - String transcoders, one per pair of string encodings. A
//!   transcode is a crossing between two guest memories, so it runs
//!   on one boundary context built for the copy, which is what reads
//!   and writes through each side's options.
//! - Resource transfer, which moves an `own<T>` or lends a
//!   `borrow<T>` from one component instance's handle table to
//!   another's. The polyfill keeps one handle table per component
//!   instance, shared by every resource type and every other handle
//!   kind the instance uses. An owned transfer removes the entry from
//!   the source table and inserts it into the destination table,
//!   which allocates its own index for it; a borrow transfer inserts
//!   a borrow entry into the destination table for the duration of
//!   the call.
//! - A trap intrinsic that raises a Wasmtime trap code.
//! - Enter and exit intrinsics around a synchronous call between two
//!   components. The enter intrinsic pushes the callee's task on the
//!   store's stack of current scopes and marks the callee instance
//!   as one that may not suspend; the exit intrinsic validates the
//!   task's borrows, restores the flag, and pops the task.
//! - The two context slots of the current thread, which an adapter
//!   saves and restores around the callee.

use std::sync::{Arc, Mutex};

use anyhow::anyhow;
use wasm_runtime_layer::{Func as RuntimeFunc, FuncType, Val as RuntimeVal, ValType as CoreType};
use wasmtime_environ::Trap;

use crate::abi::context::BoundaryContext;
use crate::abi::instance::BoundaryInstance;
use crate::abi::layout::FlatType;
use crate::abi::options::BoundaryOptions;
use crate::abi::runtime_state::AbiRuntimeState;
use crate::abi::transcode::transcode;
use crate::concurrency::{InstanceId, Scope, ThreadId};
use crate::error::{Error, Result};
use crate::executor::ir::{CoreSignature, TranscodeOp};
use crate::resource::{HandleKind, HandleTables, ResourceTableRuntime};
use crate::store::StoreContext;

/// Build a `context.get` intrinsic for `slot`. It reads the slot of
/// the current thread: the thread of the task on top of the store's
/// stack of current scopes.
pub fn build_context_get<T: 'static>(
    store: &mut StoreContext<'_, T>,
    slot: usize,
    signature: &CoreSignature,
) -> RuntimeFunc {
    let tables = store.tables_handle();
    RuntimeFunc::new(
        store.runtime_mut(),
        core_func_type(signature),
        move |_store_ctx, _args, results| {
            results[0] = RuntimeVal::I32(context_get(&tables, slot)?);
            Ok(())
        },
    )
}

/// Read slot `slot` of the current thread, which is what a
/// `context.get` intrinsic does.
fn context_get(tables: &Arc<Mutex<HandleTables>>, slot: usize) -> anyhow::Result<i32> {
    let guard = lock_tables(tables)?;
    let thread = current_thread(&guard)?;
    let value = *guard
        .tasks
        .thread(thread)
        .and_then(|record| record.context.get(slot))
        .ok_or_else(|| Error::internal("context slot index out of range"))?;
    Ok(value)
}

/// Build a `context.set` intrinsic for `slot`. See
/// [`build_context_get`]: it writes the current thread's slot.
pub fn build_context_set<T: 'static>(
    store: &mut StoreContext<'_, T>,
    slot: usize,
    signature: &CoreSignature,
) -> RuntimeFunc {
    let tables = store.tables_handle();
    RuntimeFunc::new(
        store.runtime_mut(),
        core_func_type(signature),
        move |_store_ctx, args, _results| {
            let value = arg_u32(args, 0)? as i32;
            context_set(&tables, slot, value)
        },
    )
}

/// Write `value` into slot `slot` of the current thread, which is
/// what a `context.set` intrinsic does.
fn context_set(tables: &Arc<Mutex<HandleTables>>, slot: usize, value: i32) -> anyhow::Result<()> {
    let mut guard = lock_tables(tables)?;
    let thread = current_thread(&guard)?;
    let target = guard
        .tasks
        .thread_mut(thread)
        .and_then(|record| record.context.get_mut(slot))
        .ok_or_else(|| Error::internal("context slot index out of range"))?;
    *target = value;
    Ok(())
}

/// The thread whose context slots the context intrinsics address.
fn current_thread(guard: &HandleTables) -> anyhow::Result<ThreadId> {
    guard
        .tasks
        .current_thread()
        .ok_or_else(|| anyhow!("an adapter read or wrote a context slot with no task on the stack"))
}

/// The runtime-layer function type for a [`CoreSignature`].
pub fn core_func_type(signature: &CoreSignature) -> FuncType {
    FuncType::new(
        signature.params.iter().copied().map(core_type_of_flat),
        signature.results.iter().copied().map(core_type_of_flat),
    )
}

fn core_type_of_flat(slot: FlatType) -> CoreType {
    match slot {
        FlatType::I32 => CoreType::I32,
        FlatType::I64 => CoreType::I64,
        FlatType::F32 => CoreType::F32,
        FlatType::F64 => CoreType::F64,
    }
}

/// Build the `trap` intrinsic: one `i32` Wasmtime trap code in,
/// a trap out. The message is the one Wasmtime prints for the code.
pub fn build_trap<T: 'static>(
    store: &mut StoreContext<'_, T>,
    signature: &CoreSignature,
) -> RuntimeFunc {
    RuntimeFunc::new(
        store.runtime_mut(),
        core_func_type(signature),
        move |_store_ctx, args, _results| {
            let code = arg_u32(args, 0)?;
            let trap = u8::try_from(code)
                .ok()
                .and_then(Trap::from_u8)
                .ok_or_else(|| anyhow!("adapter raised an unknown trap code {code}"))?;
            Err(anyhow!("{trap}"))
        },
    )
}

/// Build the `enter-sync-call` intrinsic. The adapter passes the
/// caller instance, whether the callee is asynchronous, and the
/// callee instance. A synchronous call between two components is a
/// task with one thread, so the intrinsic pushes the callee's task
/// on the store's stack of current scopes.
pub fn build_enter_sync_call<T: 'static>(
    store: &mut StoreContext<'_, T>,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> RuntimeFunc {
    let tables = store.tables_handle();
    RuntimeFunc::new(
        store.runtime_mut(),
        core_func_type(signature),
        move |_store_ctx, args, _results| {
            let (callee, callee_async) = enter_sync_call_arguments(&abi_state, args)?;
            enter_sync_call(&tables, callee, callee_async)
        },
    )
}

/// The callee of an `enter-sync-call` and whether it is
/// asynchronous. The adapter passes the caller instance, then
/// whether the callee is asynchronous, then the callee instance;
/// only the last two are read, because the task the intrinsic pushes
/// and the instance it flags are the callee's.
fn enter_sync_call_arguments(
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    args: &[RuntimeVal],
) -> anyhow::Result<(InstanceId, bool)> {
    let callee_async = arg_u32(args, 1)? != 0;
    let callee = instance_at(abi_state, arg_u32(args, 2)?)?;
    Ok((callee, callee_async))
}

/// Build the `exit-sync-call` intrinsic. See
/// [`build_enter_sync_call`]: the callee's task is validated and
/// popped, and a borrow the callee did not drop traps with the
/// message Wasmtime uses.
pub fn build_exit_sync_call<T: 'static>(
    store: &mut StoreContext<'_, T>,
    signature: &CoreSignature,
) -> RuntimeFunc {
    let tables = store.tables_handle();
    RuntimeFunc::new(
        store.runtime_mut(),
        core_func_type(signature),
        move |_store_ctx, _args, _results| exit_sync_call(&tables),
    )
}

/// Push the task of a synchronous call into `callee` and, unless the
/// callee is asynchronous, mark the instance as one that may not
/// suspend for the duration. The flag's old value is saved on the
/// task's thread, which is where Wasmtime saves it and where the
/// exit intrinsic reads it back.
fn enter_sync_call(
    tables: &Arc<Mutex<HandleTables>>,
    callee: InstanceId,
    callee_async: bool,
) -> anyhow::Result<()> {
    let mut guard = lock_tables(tables)?;
    let task = guard.tasks.push_task(None, None, callee);
    guard.tasks.start_task(task);
    if callee_async {
        return Ok(());
    }
    let old = guard
        .tasks
        .set_may_not_suspend(callee, true)
        .ok_or_else(|| anyhow!("the adapter named an instance the store does not hold"))?;
    let thread = guard
        .tasks
        .task(task)
        .map(|record| record.implicit_thread)
        .ok_or_else(|| anyhow!("a task pushed by the enter intrinsic has no record"))?;
    if let Some(record) = guard.tasks.thread_mut(thread) {
        record.old_may_not_suspend = Some(old);
    }
    Ok(())
}

/// Validate and pop the task the enter intrinsic pushed. The two
/// intrinsics are separate adapter calls that pass no identity
/// between them, so the innermost task on the stack is the one to
/// pop, and anything a failed call left above it goes with it.
fn exit_sync_call(tables: &Arc<Mutex<HandleTables>>) -> anyhow::Result<()> {
    match lock_tables(tables)?.exit_current_task() {
        Ok(()) => Ok(()),
        Err(_) => Err(anyhow!(
            "wasm trap: borrow handles still remain at the end of the call"
        )),
    }
}

/// The store-wide identity of the component instance the adapter
/// names by `index`, the translator's per-instantiation index.
fn instance_at(abi_state: &Arc<Mutex<AbiRuntimeState>>, index: u32) -> anyhow::Result<InstanceId> {
    let state = abi_state
        .lock()
        .map_err(|_| anyhow!("ABI state poisoned"))?;
    state
        .component_instances
        .get(index as usize)
        .copied()
        .ok_or_else(|| {
            anyhow!(
                "adapter named component instance {index}, which this instantiation does not hold"
            )
        })
}

/// Lock the store's handle tables and record state.
fn lock_tables(
    tables: &Arc<Mutex<HandleTables>>,
) -> anyhow::Result<std::sync::MutexGuard<'_, HandleTables>> {
    tables
        .lock()
        .map_err(|_| anyhow!("resource handle tables lock poisoned"))
}

/// Build the `resource-transfer-own` or `resource-transfer-borrow`
/// intrinsic. The adapter passes the handle index in the caller's
/// table and the caller's and callee's table indices, and receives
/// the index in the callee's table. An owned handle moves between the
/// tables: it leaves the caller's table, which it cannot do while a
/// borrow of it is lent out, and enters the callee's. A borrowed
/// handle lends the caller's entry for the call and gives the callee
/// a borrow owed to the call's scope, or the rep itself when the
/// callee is the resource's defining instance.
pub fn build_resource_transfer<T: 'static>(
    store: &mut StoreContext<'_, T>,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
    own: bool,
) -> RuntimeFunc {
    let tables = store.tables_handle();
    RuntimeFunc::new(
        store.runtime_mut(),
        core_func_type(signature),
        move |_store_ctx, args, results| {
            let index = arg_u32(args, 0)?;
            let src = table_at(&abi_state, arg_u32(args, 1)?)?;
            let dst = table_at(&abi_state, arg_u32(args, 2)?)?;
            let out = if own {
                transfer_own(&tables, src, dst, index)?
            } else {
                transfer_borrow(&tables, src, dst, index)?
            };
            results[0] = RuntimeVal::I32(out as i32);
            Ok(())
        },
    )
}

fn table_at(
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    table_index: u32,
) -> anyhow::Result<ResourceTableRuntime> {
    let state = abi_state
        .lock()
        .map_err(|_| anyhow!("ABI state poisoned"))?;
    state
        .resource_tables
        .get(table_index as usize)
        .copied()
        .flatten()
        .ok_or_else(|| {
            anyhow!(
                "adapter named resource table {table_index}, which this instantiation does not hold"
            )
        })
}

fn transfer_own(
    tables: &Arc<Mutex<HandleTables>>,
    src: ResourceTableRuntime,
    dst: ResourceTableRuntime,
    index: u32,
) -> anyhow::Result<u32> {
    let mut guard = tables
        .lock()
        .map_err(|_| anyhow!("resource handle tables lock poisoned"))?;
    let rep = guard
        .remove_own(src.table, index, src.type_id, src.guest_defined)
        .map_err(|e| anyhow!("wasm trap: {e}"))?;
    Ok(guard.insert_own(dst.table, dst.type_id, dst.guest_defined, rep))
}

fn transfer_borrow(
    tables: &Arc<Mutex<HandleTables>>,
    src: ResourceTableRuntime,
    dst: ResourceTableRuntime,
    index: u32,
) -> anyhow::Result<u32> {
    let mut guard = tables
        .lock()
        .map_err(|_| anyhow!("resource handle tables lock poisoned"))?;
    // Lift the borrow out of the caller: the defining instance holds
    // reps directly; anyone else holds a table entry, and an owning
    // entry is lent for the rest of the call.
    let rep = if src.defining {
        index
    } else {
        let entry = guard
            .lookup(src.table, index, src.type_id, src.guest_defined)
            .map_err(|e| anyhow!("wasm trap: {e}"))?;
        if matches!(entry, HandleKind::Own { .. }) {
            guard
                .lend(src.table, index)
                .map_err(|e| anyhow!("wasm trap: {e}"))?;
        }
        entry
            .rep()
            .expect("lookup only ever returns a resource entry")
    };
    // Lower it into the callee: the defining instance receives the
    // rep; anyone else receives a borrow entry owed to the call.
    if dst.defining {
        Ok(rep)
    } else {
        guard
            .insert_borrow(dst.table, dst.type_id, dst.guest_defined, rep)
            .ok_or_else(|| anyhow!("wasm trap: a borrow can only be transferred during a call"))
    }
}

/// Build a string transcoder. The transcode is a crossing between
/// two guest memories, so it runs on one boundary context built for
/// the copy: the options of each side name the memory slot, and the
/// context is what reads and writes through them.
///
/// The slots are resolved at call time, because the executor's
/// `ExtractMemory` directives fill them between trampoline
/// construction and the adapter's first call. The scope the crossing
/// counts against is the one on top of the store's stack, which is
/// the callee's task: the fused adapter runs its enter intrinsic
/// before it translates any argument.
pub fn build_transcoder<T: 'static>(
    store: &mut StoreContext<'_, T>,
    op: TranscodeOp,
    from_memory: usize,
    to_memory: usize,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> RuntimeFunc {
    let result_widths: Vec<FlatType> = signature.results.clone();
    let tables = store.tables_handle();
    RuntimeFunc::new(
        store.runtime_mut(),
        core_func_type(signature),
        move |store_ctx, args, results| {
            let source = BoundaryOptions::for_memory(from_memory, &abi_state)?;
            let destination = BoundaryOptions::for_memory(to_memory, &abi_state)?;
            let (instance, scope) = copy_scope(&tables)?;
            let mut ctx = BoundaryContext::for_copy(
                store_ctx,
                destination,
                source,
                BoundaryInstance::without_tables(instance),
                scope,
            );
            transcode(&mut ctx, op, args, results, &result_widths)
                .map_err(|err| anyhow!("string transcoder failed: {err}"))
        },
    )
}

/// The scope an adapter's copy between two guest memories counts
/// against, and the component instance of that scope's task. Both
/// are absent only when nothing is on the stack, which no adapter
/// call reaches.
fn copy_scope(
    tables: &Arc<Mutex<HandleTables>>,
) -> anyhow::Result<(Option<InstanceId>, Option<Scope>)> {
    let guard = lock_tables(tables)?;
    let scope = guard.tasks.current_scope();
    let instance = guard
        .tasks
        .current_task()
        .and_then(|task| guard.tasks.task(task))
        .map(|record| record.instance);
    Ok((instance, scope))
}

fn arg_u32(args: &[RuntimeVal], index: usize) -> Result<u32> {
    match args.get(index) {
        Some(RuntimeVal::I32(v)) => Ok(*v as u32),
        _ => Err(Error::internal("intrinsic expected an i32 argument")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::{ResourceTypeId, TableId};

    fn runtime(table: TableId, type_id: ResourceTypeId) -> ResourceTableRuntime {
        ResourceTableRuntime {
            table,
            type_id,
            resource_index: 0,
            defining: false,
            guest_defined: true,
        }
    }

    #[wcmp_macros::test]
    fn it_moves_an_owned_handle_from_the_source_table_to_the_destination_table() {
        let tables = Arc::new(Mutex::new(HandleTables::new()));
        let type_id = ResourceTypeId::fresh();
        let src_table = TableId::fresh();
        let dst_table = TableId::fresh();
        let src = runtime(src_table, type_id);
        let dst = runtime(dst_table, type_id);
        let (index, seeded) = {
            let mut guard = tables.lock().unwrap();
            // Seed the destination first, so the index it allocates
            // for the transferred entry cannot be the source's index.
            let seeded = guard.insert_own(dst_table, type_id, true, 99);
            let index = guard.insert_own(src_table, type_id, true, 42);
            (index, seeded)
        };
        assert_eq!(index, seeded, "each fresh table starts at the same index");

        let new_index = transfer_own(&tables, src, dst, index).unwrap();

        assert_ne!(
            new_index, index,
            "the destination allocates its own index for the entry"
        );
        let guard = tables.lock().unwrap();
        assert!(
            guard.lookup(src_table, index, type_id, true).is_err(),
            "the entry leaves the source table"
        );
        assert_eq!(
            guard
                .lookup(dst_table, new_index, type_id, true)
                .unwrap()
                .rep(),
            Some(42),
            "the entry takes an index in the destination table"
        );
        assert_eq!(
            guard
                .lookup(dst_table, seeded, type_id, true)
                .unwrap()
                .rep(),
            Some(99),
            "the destination's own entry keeps its index"
        );
    }

    #[wcmp_macros::test]
    fn it_inserts_a_borrow_entry_in_the_destination_table_for_the_call() {
        let tables = Arc::new(Mutex::new(HandleTables::new()));
        let type_id = ResourceTypeId::fresh();
        let src_table = TableId::fresh();
        let dst_table = TableId::fresh();
        let src = runtime(src_table, type_id);
        let dst = runtime(dst_table, type_id);
        let (index, task) = {
            let mut guard = tables.lock().unwrap();
            let index = guard.insert_own(src_table, type_id, true, 7);
            let instance = guard.tasks.insert_instance();
            let task = guard.tasks.push_task(None, None, instance);
            (index, task)
        };

        let borrow_index = transfer_borrow(&tables, src, dst, index).unwrap();

        let guard = tables.lock().unwrap();
        assert_eq!(
            guard
                .lookup(dst_table, borrow_index, type_id, true)
                .unwrap(),
            HandleKind::Borrow {
                type_id,
                guest_defined: true,
                rep: 7,
                task,
            }
        );
        assert_eq!(
            guard.lookup(src_table, index, type_id, true).unwrap(),
            HandleKind::Own {
                type_id,
                guest_defined: true,
                rep: 7,
                lend_count: 1,
            },
            "the source entry stays, counting the one lend"
        );
    }

    #[wcmp_macros::test]
    fn it_pushes_the_callees_task_and_marks_the_instance_may_not_suspend() {
        let tables = Arc::new(Mutex::new(HandleTables::new()));
        let callee = tables.lock().unwrap().tasks.insert_instance();

        enter_sync_call(&tables, callee, false).expect("the enter intrinsic runs");

        {
            let guard = tables.lock().unwrap();
            let Some(Scope::Task(task)) = guard.tasks.current_scope() else {
                panic!("the enter intrinsic pushes the callee's task");
            };
            assert_eq!(
                guard.tasks.task(task).map(|record| record.instance),
                Some(callee),
                "the task names the callee instance"
            );
            assert!(
                guard
                    .tasks
                    .instance(callee)
                    .is_some_and(|record| record.may_not_suspend),
                "the callee instance may not suspend for the call"
            );
        }

        exit_sync_call(&tables).expect("the exit intrinsic runs");

        let guard = tables.lock().unwrap();
        assert_eq!(
            guard.tasks.current_scope(),
            None,
            "the exit intrinsic pops the task"
        );
        assert!(
            guard
                .tasks
                .instance(callee)
                .is_some_and(|record| !record.may_not_suspend),
            "the exit intrinsic restores the flag"
        );
        assert_eq!(guard.tasks.task_count(), 0, "the task record is gone");
        assert_eq!(guard.tasks.thread_count(), 0, "its thread record too");
    }

    #[wcmp_macros::test]
    fn it_leaves_the_flag_set_for_a_nested_synchronous_call() {
        let tables = Arc::new(Mutex::new(HandleTables::new()));
        let callee = tables.lock().unwrap().tasks.insert_instance();

        enter_sync_call(&tables, callee, false).expect("the outer call");
        enter_sync_call(&tables, callee, false).expect("a call back into the same instance");
        exit_sync_call(&tables).expect("the inner call returns");

        assert!(
            tables
                .lock()
                .unwrap()
                .tasks
                .instance(callee)
                .is_some_and(|record| record.may_not_suspend),
            "the inner call restores the value the outer call set"
        );

        exit_sync_call(&tables).expect("the outer call returns");
        assert!(
            tables
                .lock()
                .unwrap()
                .tasks
                .instance(callee)
                .is_some_and(|record| !record.may_not_suspend),
            "the outer call restores the value the instance started with"
        );
    }

    #[wcmp_macros::test]
    fn it_reads_and_writes_the_context_slots_of_the_current_thread() {
        // Every read and write here goes through the bodies the
        // `context.get` and `context.set` intrinsics run.
        let tables = Arc::new(Mutex::new(HandleTables::new()));
        let instance = tables.lock().unwrap().tasks.insert_instance();
        assert!(
            context_get(&tables, 0).is_err(),
            "there is no thread to address with no task on the stack"
        );

        let caller = tables.lock().unwrap().tasks.push_task(None, None, instance);
        context_set(&tables, 0, 7).expect("the caller writes its first slot");
        context_set(&tables, 1, 8).expect("the caller writes its second slot");
        assert!(
            context_set(&tables, 2, 9).is_err(),
            "a thread has two context slots and no more"
        );

        let callee = tables.lock().unwrap().tasks.push_task(None, None, instance);
        assert_eq!(
            (
                context_get(&tables, 0).unwrap(),
                context_get(&tables, 1).unwrap()
            ),
            (0, 0),
            "the callee's task brings its own thread, with empty slots"
        );
        context_set(&tables, 0, 9).expect("the callee writes its own first slot");

        assert_eq!(tables.lock().unwrap().exit_task(callee), Ok(()));
        assert_eq!(
            (
                context_get(&tables, 0).unwrap(),
                context_get(&tables, 1).unwrap()
            ),
            (7, 8),
            "the caller's slots are as it left them"
        );
        assert_eq!(
            tables.lock().unwrap().tasks.current_task(),
            Some(caller),
            "the caller's task is current again"
        );
    }

    #[wcmp_macros::test]
    fn it_pops_the_callees_task_with_the_export_task_when_the_call_is_abandoned() {
        // A guest that traps inside a composed call never reaches the
        // exit intrinsic, so the callee's task is still on the stack
        // when the export's task is abandoned. Abandoning the export's
        // task ends the callee's with it and restores the flag the
        // enter intrinsic set.
        let tables = Arc::new(Mutex::new(HandleTables::new()));
        let (caller, callee) = {
            let mut guard = tables.lock().unwrap();
            let instance = guard.tasks.insert_instance();
            let caller = guard.tasks.push_task(None, None, instance);
            (caller, instance)
        };

        enter_sync_call(&tables, callee, false).expect("the enter intrinsic runs");
        assert!(
            tables
                .lock()
                .unwrap()
                .tasks
                .instance(callee)
                .is_some_and(|record| record.may_not_suspend),
            "the callee instance may not suspend for the call"
        );

        // The callee traps: the exit intrinsic never runs, and the
        // failure reaches the export's task instead.
        tables.lock().unwrap().abandon_task(caller);

        let guard = tables.lock().unwrap();
        assert_eq!(
            guard.tasks.current_scope(),
            None,
            "both tasks leave the stack"
        );
        assert!(
            guard
                .tasks
                .instance(callee)
                .is_some_and(|record| !record.may_not_suspend),
            "the flag the enter intrinsic set is restored"
        );
        assert_eq!(guard.tasks.task_count(), 0, "no task record is left");
        assert_eq!(guard.tasks.thread_count(), 0, "nor any thread record");
    }

    #[wcmp_macros::test]
    fn it_takes_the_callee_of_an_enter_from_the_third_adapter_argument() {
        let mut tables = HandleTables::new();
        let caller = tables.tasks.insert_instance();
        let callee = tables.tasks.insert_instance();
        let abi_state = Arc::new(Mutex::new(AbiRuntimeState {
            memories: Vec::new(),
            reallocs: Vec::new(),
            post_returns: Vec::new(),
            callbacks: Vec::new(),
            resource_tables: Vec::new(),
            component_instances: vec![caller, callee],
        }));

        // The adapter passes the caller instance, whether the callee
        // is asynchronous, and the callee instance, in that order.
        let (named, callee_async) = enter_sync_call_arguments(
            &abi_state,
            &[RuntimeVal::I32(0), RuntimeVal::I32(0), RuntimeVal::I32(1)],
        )
        .expect("the arguments decode");
        assert_eq!(named, callee, "the third argument names the callee");
        assert_ne!(named, caller, "not the first, which names the caller");
        assert!(!callee_async, "the second argument is clear");

        let (named, callee_async) = enter_sync_call_arguments(
            &abi_state,
            &[RuntimeVal::I32(1), RuntimeVal::I32(1), RuntimeVal::I32(0)],
        )
        .expect("the arguments decode");
        assert_eq!(
            named, caller,
            "the two instances swap when the third argument does"
        );
        assert!(
            callee_async,
            "the second argument says the callee is asynchronous"
        );
    }
}
