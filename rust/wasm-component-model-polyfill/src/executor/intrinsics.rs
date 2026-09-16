//! Intrinsics that adapter modules import.
//!
//! When a component contains other components, the translator's
//! fused adapter compiler emits one core module per cross-component
//! call site. The adapter lifts arguments from the caller's memory
//! and lowers them into the callee's memory in core Wasm. It imports
//! a small set of host functions for the work it cannot do inline:
//!
//! - String transcoders, one per pair of string encodings, whose
//!   argument and result conventions follow the fused adapter
//!   compiler's `transcode` signatures and Wasmtime's libcalls of
//!   the same names. Pointers are byte offsets into the named
//!   memories; lengths count code units of the respective encoding.
//! - Resource transfer, which moves an `own<T>` or lends a
//!   `borrow<T>` from one component instance's handle table to
//!   another's. The polyfill keeps one handle table per component
//!   instance, shared by every resource type and every other handle
//!   kind the instance uses. An owned transfer removes the entry from
//!   the source table and inserts it into the destination table, so
//!   the index changes; a borrow transfer inserts a borrow entry into
//!   the destination table for the duration of the call.
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
use wasm_runtime_layer::{
    AsContextMut, Func as RuntimeFunc, FuncType, Memory, StoreContextMut, Val as RuntimeVal,
    ValType as CoreType,
};
use wasmtime_environ::Trap;

use crate::abi::layout::FlatType;
use crate::backend::Backend;
use crate::concurrency::{InstanceId, ThreadId};
use crate::error::{Error, Result};
use crate::executor::ir::{CoreSignature, TranscodeOp};
use crate::executor::trampoline::AbiRuntimeState;
use crate::resource::{HandleKind, HandleTables, ResourceTableRuntime};
use crate::store::Store;

/// The tag a "compact UTF-16" length carries when the string was
/// left as UTF-16 rather than deflated to Latin-1.
const UTF16_TAG: u32 = 1 << 31;

/// Build a `context.get` intrinsic for `slot`. It reads the slot of
/// the current thread: the thread of the task on top of the store's
/// stack of current scopes.
pub fn build_context_get<T: 'static>(
    store: &mut Store<T>,
    slot: usize,
    signature: &CoreSignature,
) -> RuntimeFunc {
    let tables = store.tables_handle();
    RuntimeFunc::new(
        store.inner_mut(),
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
    store: &mut Store<T>,
    slot: usize,
    signature: &CoreSignature,
) -> RuntimeFunc {
    let tables = store.tables_handle();
    RuntimeFunc::new(
        store.inner_mut(),
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
pub fn build_trap<T: 'static>(store: &mut Store<T>, signature: &CoreSignature) -> RuntimeFunc {
    RuntimeFunc::new(
        store.inner_mut(),
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
    store: &mut Store<T>,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> RuntimeFunc {
    let tables = store.tables_handle();
    RuntimeFunc::new(
        store.inner_mut(),
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
    store: &mut Store<T>,
    signature: &CoreSignature,
) -> RuntimeFunc {
    let tables = store.tables_handle();
    RuntimeFunc::new(
        store.inner_mut(),
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
    store: &mut Store<T>,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
    own: bool,
) -> RuntimeFunc {
    let tables = store.tables_handle();
    RuntimeFunc::new(
        store.inner_mut(),
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
            guard.lend(src.table, index);
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

/// Build a string transcoder. The source and destination memories
/// are read out of `abi_state` at call time, because the executor's
/// `ExtractMemory` directives fill the slots between trampoline
/// construction and the adapter's first call.
pub fn build_transcoder<T: 'static>(
    store: &mut Store<T>,
    op: TranscodeOp,
    from_memory: usize,
    to_memory: usize,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> RuntimeFunc {
    let result_widths: Vec<FlatType> = signature.results.clone();
    RuntimeFunc::new(
        store.inner_mut(),
        core_func_type(signature),
        move |store_ctx, args, results| {
            let (from, to) = {
                let state = abi_state
                    .lock()
                    .map_err(|_| Error::internal("ABI runtime state lock poisoned"))?;
                let from = state
                    .memories
                    .get(from_memory)
                    .and_then(|m| m.clone())
                    .ok_or_else(|| Error::internal("transcoder source memory is not extracted"))?;
                let to = state
                    .memories
                    .get(to_memory)
                    .and_then(|m| m.clone())
                    .ok_or_else(|| {
                        Error::internal("transcoder destination memory is not extracted")
                    })?;
                (from, to)
            };
            transcode(store_ctx, op, &from, &to, args, results, &result_widths)
                .map_err(|err| anyhow!("string transcoder failed: {err}"))
        },
    )
}

/// Run one transcoder. Pointers and lengths arrive at the width of
/// the memory they address, `i32` for a 32-bit memory and `i64` for
/// a 64-bit one, and the results are written back at the widths
/// `result_widths` names.
fn transcode<T: 'static>(
    mut ctx: StoreContextMut<'_, T, Backend>,
    op: TranscodeOp,
    from: &Memory,
    to: &Memory,
    args: &[RuntimeVal],
    results: &mut [RuntimeVal],
    result_widths: &[FlatType],
) -> Result<()> {
    let set_results =
        |results: &mut [RuntimeVal], values: &[usize]| set_results(results, result_widths, values);
    match op {
        TranscodeOp::CopyUtf8 => {
            let (src, len, dst) = three(args)?;
            let bytes = read(&mut ctx, from, src, len)?;
            core::str::from_utf8(&bytes).map_err(|_| invalid("invalid utf8 encoding"))?;
            write(&mut ctx, to, dst, &bytes)
        }
        TranscodeOp::CopyUtf16 => {
            let (src, len, dst) = three(args)?;
            let units = read_utf16(&mut ctx, from, src, len)?;
            decode_utf16(&units)?;
            write_utf16(&mut ctx, to, dst, &units)
        }
        TranscodeOp::CopyLatin1 => {
            let (src, len, dst) = three(args)?;
            let bytes = read(&mut ctx, from, src, len)?;
            write(&mut ctx, to, dst, &bytes)
        }
        TranscodeOp::Latin1ToUtf16 => {
            let (src, len, dst) = three(args)?;
            let bytes = read(&mut ctx, from, src, len)?;
            let units: Vec<u16> = bytes.iter().map(|b| u16::from(*b)).collect();
            write_utf16(&mut ctx, to, dst, &units)
        }
        TranscodeOp::Utf8ToUtf16 => {
            let (src, len, dst) = three(args)?;
            let bytes = read(&mut ctx, from, src, len)?;
            let text =
                core::str::from_utf8(&bytes).map_err(|_| invalid("invalid utf8 encoding"))?;
            let units: Vec<u16> = text.encode_utf16().collect();
            write_utf16(&mut ctx, to, dst, &units)?;
            set_results(results, &[units.len()])
        }
        TranscodeOp::Utf16ToUtf8 => {
            let (src, src_len, dst, dst_len, first_pass) = five(args)?;
            let units = read_utf16(&mut ctx, from, src, src_len)?;
            let mut out: Vec<u8> = Vec::with_capacity(dst_len);
            let mut src_read = 0usize;
            let mut consumed = 0usize;
            for ch in char::decode_utf16(units.iter().copied()) {
                let ch = ch.map_err(|_| invalid("invalid utf16 encoding"))?;
                consumed += ch.len_utf16();
                if first_pass != 0 && u32::from(ch) >= 0x80 {
                    break;
                }
                let remaining = dst_len - out.len();
                if remaining < 4 && remaining < ch.len_utf8() {
                    break;
                }
                let mut buf = [0u8; 4];
                out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                src_read = consumed;
            }
            write(&mut ctx, to, dst, &out)?;
            set_results(results, &[src_read, out.len()])
        }
        TranscodeOp::Latin1ToUtf8 => {
            let (src, src_len, dst, dst_len, first_pass) = five(args)?;
            let bytes = read(&mut ctx, from, src, src_len)?;
            let stop = if first_pass != 0 {
                bytes.iter().position(|b| *b >= 0x80).unwrap_or(bytes.len())
            } else {
                bytes.len()
            };
            let mut out: Vec<u8> = Vec::with_capacity(dst_len);
            let mut read_count = 0usize;
            for b in &bytes[..stop] {
                let ch = char::from(*b);
                if out.len() + ch.len_utf8() > dst_len {
                    break;
                }
                let mut buf = [0u8; 4];
                out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                read_count += 1;
            }
            write(&mut ctx, to, dst, &out)?;
            set_results(results, &[read_count, out.len()])
        }
        TranscodeOp::Utf16ToCompactProbablyUtf16 => {
            let (src, len, dst) = three(args)?;
            let units = read_utf16(&mut ctx, from, src, len)?;
            decode_utf16(&units)?;
            if units.iter().all(|u| *u <= 0xFF) {
                let bytes: Vec<u8> = units.iter().map(|u| *u as u8).collect();
                write(&mut ctx, to, dst, &bytes)?;
                set_results(results, &[len])
            } else {
                write_utf16(&mut ctx, to, dst, &units)?;
                set_results(results, &[len | UTF16_TAG as usize])
            }
        }
        TranscodeOp::Utf8ToLatin1 => {
            let (src, len, dst) = three(args)?;
            let bytes = read(&mut ctx, from, src, len)?;
            let text =
                core::str::from_utf8(&bytes).map_err(|_| invalid("invalid utf8 encoding"))?;
            let mut out: Vec<u8> = Vec::with_capacity(len);
            let mut read_count = 0usize;
            for ch in text.chars() {
                match u8::try_from(u32::from(ch)) {
                    Ok(b) => out.push(b),
                    Err(_) => break,
                }
                read_count += ch.len_utf8();
            }
            write(&mut ctx, to, dst, &out)?;
            set_results(results, &[read_count, out.len()])
        }
        TranscodeOp::Utf16ToLatin1 => {
            let (src, len, dst) = three(args)?;
            let units = read_utf16(&mut ctx, from, src, len)?;
            let mut out: Vec<u8> = Vec::with_capacity(len);
            for u in &units {
                match u8::try_from(*u) {
                    Ok(b) => out.push(b),
                    Err(_) => break,
                }
            }
            write(&mut ctx, to, dst, &out)?;
            set_results(results, &[out.len(), out.len()])
        }
        TranscodeOp::Utf8ToCompactUtf16 => {
            let (src, src_len, dst, _dst_len, latin1_so_far) = five(args)?;
            inflate_latin1(&mut ctx, to, dst, latin1_so_far)?;
            let bytes = read(&mut ctx, from, src, src_len)?;
            let text =
                core::str::from_utf8(&bytes).map_err(|_| invalid("invalid utf8 encoding"))?;
            let units: Vec<u16> = text.encode_utf16().collect();
            write_utf16(&mut ctx, to, dst + latin1_so_far * 2, &units)?;
            set_results(results, &[units.len() + latin1_so_far])
        }
        TranscodeOp::Utf16ToCompactUtf16 => {
            let (src, src_len, dst, _dst_len, latin1_so_far) = five(args)?;
            inflate_latin1(&mut ctx, to, dst, latin1_so_far)?;
            let units = read_utf16(&mut ctx, from, src, src_len)?;
            decode_utf16(&units)?;
            write_utf16(&mut ctx, to, dst + latin1_so_far * 2, &units)?;
            set_results(results, &[src_len + latin1_so_far])
        }
    }
}

/// Inflate the first `count` Latin-1 bytes at `dst` into UTF-16
/// code units in place, from the end so nothing is overwritten
/// before it is read.
fn inflate_latin1<T: 'static>(
    ctx: &mut StoreContextMut<'_, T, Backend>,
    memory: &Memory,
    dst: usize,
    count: usize,
) -> Result<()> {
    if count == 0 {
        return Ok(());
    }
    let bytes = read(ctx, memory, dst, count)?;
    let units: Vec<u16> = bytes.iter().map(|b| u16::from(*b)).collect();
    write_utf16(ctx, memory, dst, &units)
}

fn read<T: 'static>(
    ctx: &mut StoreContextMut<'_, T, Backend>,
    memory: &Memory,
    offset: usize,
    length: usize,
) -> Result<Vec<u8>> {
    let mut buffer = vec![0u8; length];
    memory
        .read(ctx.as_context_mut(), offset, &mut buffer)
        .map_err(|_| invalid("out-of-bounds string read in adapter"))?;
    Ok(buffer)
}

fn write<T: 'static>(
    ctx: &mut StoreContextMut<'_, T, Backend>,
    memory: &Memory,
    offset: usize,
    bytes: &[u8],
) -> Result<()> {
    memory
        .write(ctx.as_context_mut(), offset, bytes)
        .map_err(|_| invalid("out-of-bounds string write in adapter"))
}

fn read_utf16<T: 'static>(
    ctx: &mut StoreContextMut<'_, T, Backend>,
    memory: &Memory,
    offset: usize,
    units: usize,
) -> Result<Vec<u16>> {
    let bytes = read(ctx, memory, offset, units * 2)?;
    Ok(bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect())
}

fn write_utf16<T: 'static>(
    ctx: &mut StoreContextMut<'_, T, Backend>,
    memory: &Memory,
    offset: usize,
    units: &[u16],
) -> Result<()> {
    let mut bytes = Vec::with_capacity(units.len() * 2);
    for u in units {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    write(ctx, memory, offset, &bytes)
}

fn decode_utf16(units: &[u16]) -> Result<()> {
    for ch in char::decode_utf16(units.iter().copied()) {
        ch.map_err(|_| invalid("invalid utf16 encoding"))?;
    }
    Ok(())
}

fn three(args: &[RuntimeVal]) -> Result<(usize, usize, usize)> {
    Ok((
        arg_usize(args, 0)?,
        arg_usize(args, 1)?,
        arg_usize(args, 2)?,
    ))
}

fn five(args: &[RuntimeVal]) -> Result<(usize, usize, usize, usize, usize)> {
    Ok((
        arg_usize(args, 0)?,
        arg_usize(args, 1)?,
        arg_usize(args, 2)?,
        arg_usize(args, 3)?,
        arg_usize(args, 4)?,
    ))
}

fn arg_u32(args: &[RuntimeVal], index: usize) -> Result<u32> {
    match args.get(index) {
        Some(RuntimeVal::I32(v)) => Ok(*v as u32),
        _ => Err(Error::internal("intrinsic expected an i32 argument")),
    }
}

/// A pointer or length argument at the width of the memory it
/// addresses. A 64-bit offset that the host cannot address (a 32-bit
/// host with a memory past 4 GiB) is reported rather than truncated.
fn arg_usize(args: &[RuntimeVal], index: usize) -> Result<usize> {
    match args.get(index) {
        Some(RuntimeVal::I32(v)) => Ok(*v as u32 as usize),
        Some(RuntimeVal::I64(v)) => usize::try_from(*v as u64).map_err(|_| {
            Error::internal("the host cannot address a 64-bit memory offset of this size")
        }),
        _ => Err(Error::internal("intrinsic expected an integer argument")),
    }
}

/// Write the transcoder's results at the widths the adapter's core
/// signature declares.
fn set_results(results: &mut [RuntimeVal], widths: &[FlatType], values: &[usize]) -> Result<()> {
    if results.len() != values.len() || widths.len() != values.len() {
        return Err(Error::internal("intrinsic result arity mismatch"));
    }
    for ((slot, width), value) in results.iter_mut().zip(widths).zip(values) {
        *slot = match width {
            FlatType::I64 => RuntimeVal::I64(*value as u64 as i64),
            _ => RuntimeVal::I32(*value as u32 as i32),
        };
    }
    Ok(())
}

fn invalid(message: &str) -> Error {
    Error::internal(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::concurrency::Scope;
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

    #[test]
    fn it_moves_an_owned_handle_from_the_source_table_to_the_destination_table() {
        let tables = Arc::new(Mutex::new(HandleTables::new()));
        let type_id = ResourceTypeId::fresh();
        let src_table = TableId::fresh();
        let dst_table = TableId::fresh();
        let src = runtime(src_table, type_id);
        let dst = runtime(dst_table, type_id);
        let index = tables
            .lock()
            .unwrap()
            .insert_own(src_table, type_id, true, 42);

        let new_index = transfer_own(&tables, src, dst, index).unwrap();

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
    }

    #[test]
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
        assert!(
            guard.lookup(src_table, index, type_id, true).is_ok(),
            "the source entry stays lent, not removed"
        );
    }

    #[test]
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

    #[test]
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

    #[test]
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

    #[test]
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

    #[test]
    fn it_takes_the_callee_of_an_enter_from_the_third_adapter_argument() {
        let mut tables = HandleTables::new();
        let caller = tables.tasks.insert_instance();
        let callee = tables.tasks.insert_instance();
        let abi_state = Arc::new(Mutex::new(AbiRuntimeState {
            memories: Vec::new(),
            reallocs: Vec::new(),
            post_returns: Vec::new(),
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
