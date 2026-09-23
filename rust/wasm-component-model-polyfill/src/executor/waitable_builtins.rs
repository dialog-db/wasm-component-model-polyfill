//! The waitable built-ins.
//!
//! A waitable set is a group of waitables one thread waits on
//! together. A guest creates one, joins waitables to it, waits on it
//! or polls it, and drops it, through five canonical built-ins:
//! `waitable-set.new`, `waitable-set.wait`, `waitable-set.poll`,
//! `waitable-set.drop`, and `waitable.join`. A sixth, `subtask.drop`,
//! works on a single waitable rather than on a set: it takes the
//! entry of a resolved subtask away. Each one reaches the records of
//! the store through the handle table of the component instance that
//! called it, where a set or a subtask lives in the entry kind the
//! handle table reserves for it.
//!
//! Three rules are common to all of them. Each traps with the
//! cannot-leave cause when the instance's may-leave flag is clear,
//! which is the case while a `realloc` or a `post-return` of that
//! instance runs. Each but `waitable-set.new` traps when the index
//! it is given does not name what the built-in works on. And the two that
//! deliver an event write its two payloads as `u32` values at the
//! pointer and at the pointer plus four, through the memory the
//! built-in's own canon options name, and return the event's code.
//! A `u32` is stored at an aligned address, so a pointer that is not
//! a multiple of four traps and writes nothing. A poll of a set that
//! holds no event delivers the none event, whose payloads are zero,
//! so it writes and checks the pointer like any other delivery.
//!
//! `waitable-set.wait` is the one built-in here that can block. A
//! set that already holds an event delivers it and the thread does
//! not block; a set that holds none parks the thread on the set and
//! asks the suspend seam to suspend it until the set holds one. On a
//! target with no suspend provider the seam runs a nested turn, and
//! its failure is the cause the seam gives: cannot-block for a task
//! that must not block, and the deadlock or stack-switch cause for a
//! task that may. The wait on the set's record ends whichever way
//! the suspension went, so a set is never left naming a waiter that
//! is no longer there.

use std::sync::{Arc, Mutex};

use anyhow::anyhow;
use wasm_runtime_layer::{
    AsContextMut, Func as RuntimeFunc, StoreContextMut as RuntimeContextMut, Val as RuntimeVal,
};

use crate::abi::context::BoundaryContext;
use crate::abi::instance::BoundaryInstance;
use crate::abi::runtime_state::AbiRuntimeState;
use crate::backend::Backend;
use crate::concurrency::{Event, InstanceId, SuspendSeam, ThreadId, WaitableSetId};
use crate::error::{AbiCause, AbiError, AbiPosition, Error, TaskCause};
use crate::executor::intrinsics::core_func_type;
use crate::executor::ir::{CanonOptions, CoreSignature};
use crate::resource::{HandleTables, TableId};
use crate::store::StoreContextInternalExt;
use crate::store::{StoreContext, StoreData};
use crate::types::{PrimitiveType, ValueType};

/// Build the `waitable-set.new` built-in for `instance`: a waitable
/// set record enters the store and the built-in returns its index in
/// the instance's handle table.
pub fn build_waitable_set_new<T: 'static>(
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
            let (id, table) = calling_instance(&abi_state, instance)?;
            trap_if_cannot_leave(&abi_state, id, &mut store_ctx)?;
            let mut guard = lock_tables(&tables)?;
            let set = guard.tasks.insert_waitable_set();
            let index = guard.insert_waitable_set(table, set);
            results[0] = RuntimeVal::I32(index as i32);
            Ok(())
        },
    )
}

/// Build the `waitable-set.wait` built-in. The guest passes the set
/// index and a pointer, and receives the code of the event the set
/// delivered; the event's two payloads are written at the pointer.
pub fn build_waitable_set_wait<T: 'static>(
    store: &mut StoreContext<'_, T>,
    options: &CanonOptions,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> RuntimeFunc {
    let tables = store.internal().tables_handle();
    let options = Arc::new(options.clone());
    RuntimeFunc::new(
        store.internal().runtime_mut(),
        core_func_type(signature),
        move |store_ctx, args, results| {
            waitable_set_wait(store_ctx, &options, &abi_state, &tables, args, results)
        },
    )
}

/// Build the `waitable-set.poll` built-in. It takes what
/// [`build_waitable_set_wait`] takes and never blocks: a set that
/// holds no event answers with the none code, whose two payloads are
/// zero and are written at the pointer like any other event's.
pub fn build_waitable_set_poll<T: 'static>(
    store: &mut StoreContext<'_, T>,
    options: &CanonOptions,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> RuntimeFunc {
    let tables = store.internal().tables_handle();
    let options = Arc::new(options.clone());
    RuntimeFunc::new(
        store.internal().runtime_mut(),
        core_func_type(signature),
        move |store_ctx, args, results| {
            waitable_set_poll(store_ctx, &options, &abi_state, &tables, args, results)
        },
    )
}

/// Build the `waitable-set.drop` built-in: the named set's entry
/// leaves the instance's handle table and its record leaves the
/// store.
pub fn build_waitable_set_drop<T: 'static>(
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
            let set_index = arg_u32(args, 0)?;
            let (id, table) = calling_instance(&abi_state, instance)?;
            trap_if_cannot_leave(&abi_state, id, &mut store_ctx)?;
            let mut guard = lock_tables(&tables)?;
            let set = set_at(&guard, table, set_index)?;
            // The record's own checks come first: a set that still
            // holds a waitable, or one a thread waits on, traps and
            // keeps its entry.
            guard.tasks.drop_waitable_set(set).map_err(trap)?;
            guard.remove(table, set_index);
            Ok(())
        },
    )
}

/// Build the `waitable.join` built-in: the named waitable joins the
/// named set, and a set index of zero takes it out of the set it is
/// in.
pub fn build_waitable_join<T: 'static>(
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
            let waitable_index = arg_u32(args, 0)?;
            let set_index = arg_u32(args, 1)?;
            let (id, table) = calling_instance(&abi_state, instance)?;
            trap_if_cannot_leave(&abi_state, id, &mut store_ctx)?;
            let mut guard = lock_tables(&tables)?;
            let waitable = guard
                .waitable_from_handle(table, waitable_index)
                .map_err(|err| anyhow!("{err}"))?;
            // Every lookup that can trap happens before the join, so
            // a join that fails leaves the waitable in the set it
            // already named.
            let set = match set_index {
                0 => None,
                index => Some(set_at(&guard, table, index)?),
            };
            guard.tasks.join_waitable_set(waitable, set).map_err(trap)
        },
    )
}

/// Build the `subtask.drop` built-in: the named subtask's entry
/// leaves the instance's handle table, and the records the entry
/// named leave the store with it.
///
/// A subtask is the record of a call the guest made and has taken
/// delivery of. The built-in refuses every other case: an index that
/// names no entry, an index that names an entry of another kind, and
/// a subtask whose resolution has not been delivered — a call still
/// running, and also one whose result is ready but whose event the
/// guest has not taken, because the handles the call borrowed are
/// still lent out and the one notice the guest gets is still
/// waiting.
///
/// What leaves the store with the entry is the subtask record and,
/// for a call into another component's export, the callee's task
/// record once its implicit thread has exited. A call into a host
/// function has no task of its own, so the subtask's record is all
/// of it.
pub fn build_subtask_drop<T: 'static>(
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
            let subtask_index = arg_u32(args, 0)?;
            let (id, table) = calling_instance(&abi_state, instance)?;
            trap_if_cannot_leave(&abi_state, id, &mut store_ctx)?;
            let mut guard = lock_tables(&tables)?;
            let subtask = guard
                .subtask_from_handle(table, subtask_index)
                .map_err(|err| anyhow!("{err}"))?;
            // The record's own check comes first: a subtask whose
            // resolution is still owed traps and keeps its entry.
            let waitable = guard.tasks.subtask_waitable(subtask);
            guard.tasks.drop_waitable(waitable).map_err(trap)?;
            guard.remove(table, subtask_index);
            Ok(())
        },
    )
}

/// The body of the `waitable-set.wait` built-in.
fn waitable_set_wait<T: 'static>(
    mut store_ctx: RuntimeContextMut<'_, StoreData<T>, Backend>,
    options: &Arc<CanonOptions>,
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    tables: &Arc<Mutex<HandleTables>>,
    args: &[RuntimeVal],
    results: &mut [RuntimeVal],
) -> anyhow::Result<()> {
    let set_index = arg_u32(args, 0)?;
    let pointer = arg_u32(args, 1)?;
    let (id, table) = calling_instance(abi_state, options.instance)?;
    trap_if_cannot_leave(abi_state, id, &mut store_ctx)?;

    let (set, thread, delivered) = {
        let mut guard = lock_tables(tables)?;
        let set = set_at(&guard, table, set_index)?;
        let thread = current_thread(&guard)?;
        // A set that already holds an event delivers it here and the
        // thread does not block; otherwise the thread is parked on
        // the set and the suspension below is what gives way.
        let delivered = guard.wait_on_waitable_set(set, thread).map_err(trap)?;
        (set, thread, delivered)
    };

    let event = match delivered {
        Some(event) => event,
        None => block_until_ready(&mut store_ctx, tables, set, thread)?,
    };

    let (code, payloads) = (event.code().value(), event.payloads());
    write_payloads(
        &mut store_ctx,
        options,
        abi_state,
        tables,
        pointer,
        payloads,
    )?;
    results[0] = RuntimeVal::I32(code as i32);
    Ok(())
}

/// The body of the `waitable-set.poll` built-in.
fn waitable_set_poll<T: 'static>(
    mut store_ctx: RuntimeContextMut<'_, StoreData<T>, Backend>,
    options: &Arc<CanonOptions>,
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    tables: &Arc<Mutex<HandleTables>>,
    args: &[RuntimeVal],
    results: &mut [RuntimeVal],
) -> anyhow::Result<()> {
    let set_index = arg_u32(args, 0)?;
    let pointer = arg_u32(args, 1)?;
    let (id, table) = calling_instance(abi_state, options.instance)?;
    trap_if_cannot_leave(abi_state, id, &mut store_ctx)?;

    let delivered = {
        let mut guard = lock_tables(tables)?;
        let set = set_at(&guard, table, set_index)?;
        if guard.tasks.set_has_pending_event(set).map_err(trap)? {
            Some(guard.poll_waitable_set(set).map_err(trap)?)
        } else {
            None
        }
    };

    // A poll of a set that holds no event answers with the none
    // event, whose two payloads are zero. That event is written at
    // the pointer the way a delivered one is: the reference stores
    // both words on every path, so the none path checks the pointer
    // the delivering path checks, and a pointer that is misaligned
    // or leaves the memory traps whether or not the set held an
    // event. A guest that reads the pair after the none code reads
    // two zero words rather than what it last left there.
    let event = delivered.unwrap_or_else(Event::none);
    let (code, payloads) = (event.code().value(), event.payloads());
    write_payloads(
        &mut store_ctx,
        options,
        abi_state,
        tables,
        pointer,
        payloads,
    )?;
    results[0] = RuntimeVal::I32(code as i32);
    Ok(())
}

/// Suspend the current thread until `set` holds an event, and take
/// the event it holds when it does.
///
/// The wait the caller began ends whichever way the suspension went.
/// A set left naming a waiter that is no longer there would trap
/// every later drop of it, and the thread would keep a readiness
/// condition it is no longer parked on.
fn block_until_ready<T: 'static>(
    store_ctx: &mut RuntimeContextMut<'_, StoreData<T>, Backend>,
    tables: &Arc<Mutex<HandleTables>>,
    set: WaitableSetId,
    thread: ThreadId,
) -> anyhow::Result<Event> {
    let suspended = {
        let mut store = StoreContext::new(store_ctx.as_context_mut());
        SuspendSeam::suspend(&mut store, |store| {
            store
                .internal()
                .lock_tables()
                .ok()
                .and_then(|guard| guard.tasks.set_has_pending_event(set).ok())
                .unwrap_or(false)
        })
    };
    let ended = lock_tables(tables)?.finish_wait_on_waitable_set(set, thread);
    suspended.map_err(trap)?;
    ended.map_err(trap)
}

/// The alignment the pointer of a delivered event must have: the
/// pair written through it is two `u32` values, so four bytes.
const EVENT_PAYLOAD_ALIGNMENT: usize = 4;

/// Write the two payloads of a delivered event at `pointer` and at
/// `pointer` plus four, through the memory the built-in's own canon
/// options name.
///
/// The pointer's alignment is checked first: the reference stores
/// each payload as a `u32`, and a store of a `u32` traps on a
/// pointer that is not a multiple of four. The pair is then one
/// write, as Wasmtime checks the pair for bounds before it writes
/// either word: a pointer that leaves the memory halfway through the
/// pair writes nothing.
fn write_payloads<T: 'static>(
    store_ctx: &mut RuntimeContextMut<'_, StoreData<T>, Backend>,
    options: &Arc<CanonOptions>,
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    tables: &Arc<Mutex<HandleTables>>,
    pointer: u32,
    payloads: [u32; 2],
) -> anyhow::Result<()> {
    if !(pointer as usize).is_multiple_of(EVENT_PAYLOAD_ALIGNMENT) {
        return Err(trap(misaligned_event_pointer()));
    }
    let (boundary_options, instance) =
        BoundaryInstance::resolve(options, abi_state, tables).map_err(trap)?;
    let scope = lock_tables(tables)?.tasks.current_scope();
    let mut bytes = [0u8; 8];
    bytes[..4].copy_from_slice(&payloads[0].to_le_bytes());
    bytes[4..].copy_from_slice(&payloads[1].to_le_bytes());
    let mut ctx = BoundaryContext::new(
        store_ctx.as_context_mut(),
        boundary_options,
        instance,
        scope,
    );
    ctx.write_own_bytes(pointer as usize, &bytes).map_err(trap)
}

/// The store-wide identity of the component instance the translator
/// named for a built-in, with the handle table that instance keeps.
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

/// The waitable set the entry at `index` names, or the trap for an
/// index that names something else.
fn set_at(tables: &HandleTables, table: TableId, index: u32) -> anyhow::Result<WaitableSetId> {
    tables
        .waitable_set_from_handle(table, index)
        .map_err(|err| anyhow!("{err}"))
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

/// The thread a wait parks: the thread of the task on top of the
/// store's stack of current scopes.
fn current_thread(tables: &HandleTables) -> anyhow::Result<ThreadId> {
    tables
        .tasks
        .current_thread()
        .ok_or_else(|| anyhow!("a waitable set built-in ran with no task on the stack"))
}

/// Lock the store's handle tables and record state.
fn lock_tables(
    tables: &Arc<Mutex<HandleTables>>,
) -> anyhow::Result<std::sync::MutexGuard<'_, HandleTables>> {
    tables
        .lock()
        .map_err(|_| anyhow!("resource handle tables lock poisoned"))
}

/// The failure of a wait or a poll whose event pointer is not
/// aligned. The pointer is the built-in's second argument, and each
/// of the two words stored through it is a `u32`.
fn misaligned_event_pointer() -> Error {
    Error::from(AbiError {
        position: AbiPosition::Argument(1),
        valtype: Some(ValueType::Primitive(PrimitiveType::U32)),
        cause: AbiCause::InvalidEncoding {
            message: format!("event pointer not aligned to {EVENT_PAYLOAD_ALIGNMENT}"),
        },
    })
}

/// The trap a structured error becomes on its way to the guest. The
/// message is the error's own, which the conformance corpora match
/// by substring.
///
/// Two things happen on the way. A scheduler cause takes the `wasm
/// trap:` prefix a trap reaching guest code renders with, and drops
/// the wrapper the [`Error::Scheduler`] variant would otherwise put
/// in front of it, so that a corpus file can match the whole prefix
/// and message. Two of the five causes are Wasmtime trap codes and
/// carry its text exactly: the deadlock cause is `AsyncDeadlock` and
/// the cannot-block cause is `CannotBlockSyncTask`, both in
/// `wasmtime-environ`'s `src/trap_encoding.rs`. The other three —
/// the stack-switch, recursive-driver and store-not-in-poll causes —
/// are the polyfill's own, with no trap code of Wasmtime's behind
/// them; they take the same prefix because they reach the guest as
/// traps all the same. And the error's chain is flattened into the
/// message, because a trap crosses back into guest code as a string:
/// an error that carries the trap of the work a nested turn ran
/// would otherwise reach the host as the wrapper alone, with the
/// guest's own trap lost under it.
fn trap(error: Error) -> anyhow::Error {
    if let Error::Scheduler(cause) = &error {
        return anyhow!("wasm trap: {cause}");
    }
    let mut message = error.to_string();
    let mut link = std::error::Error::source(&error);
    while let Some(source) = link {
        message.push_str(": ");
        message.push_str(&source.to_string());
        link = source.source();
    }
    anyhow!("{message}")
}

fn arg_u32(args: &[RuntimeVal], index: usize) -> anyhow::Result<u32> {
    match args.get(index) {
        Some(RuntimeVal::I32(value)) => Ok(*value as u32),
        _ => Err(anyhow!("a waitable set built-in expected an i32 argument")),
    }
}
