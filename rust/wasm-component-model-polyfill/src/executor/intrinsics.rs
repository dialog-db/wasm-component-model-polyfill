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
//!   another's. The polyfill keeps one handle table per resource
//!   type per store rather than one per component instance, so a
//!   transfer keeps the same index.
//! - A trap intrinsic that raises a Wasmtime trap code.
//! - Enter and exit intrinsics around a synchronous call that
//!   carries resources. They exist so borrow scopes can be validated
//!   at exit; without per-call borrow tracking they succeed.
//! - The two context slots of the current task, which an adapter
//!   saves and restores around the callee. The polyfill runs one
//!   task, so the slots are two integers per instantiation.

use std::sync::{Arc, Mutex};

use anyhow::anyhow;
use wasm_runtime_layer::{
    AsContextMut, Func as RuntimeFunc, FuncType, Memory, StoreContextMut, Val as RuntimeVal,
    ValType as CoreType,
};
use wasmtime_environ::Trap;

use crate::abi::layout::FlatType;
use crate::backend::Backend;
use crate::error::{Error, Result};
use crate::executor::ir::{CoreSignature, TranscodeOp};
use crate::executor::trampoline::AbiRuntimeState;
use crate::resource::{HandleKind, HandleTables, ResourceTableRuntime};
use crate::store::Store;

/// The tag a "compact UTF-16" length carries when the string was
/// left as UTF-16 rather than deflated to Latin-1.
const UTF16_TAG: u32 = 1 << 31;

/// The context slots of the polyfill's single task, shared by the
/// context intrinsics of one instantiation.
#[derive(Clone, Default)]
pub struct ContextSlots(Arc<Mutex<[i32; 2]>>);

/// Build a `context.get` intrinsic for `slot`.
pub fn build_context_get<T: 'static>(
    store: &mut Store<T>,
    slot: usize,
    signature: &CoreSignature,
    context: ContextSlots,
) -> RuntimeFunc {
    RuntimeFunc::new(
        store.inner_mut(),
        core_func_type(signature),
        move |_store_ctx, _args, results| {
            let slots = context
                .0
                .lock()
                .map_err(|_| Error::internal("context slots lock poisoned"))?;
            let value = *slots
                .get(slot)
                .ok_or_else(|| Error::internal("context slot index out of range"))?;
            results[0] = RuntimeVal::I32(value);
            Ok(())
        },
    )
}

/// Build a `context.set` intrinsic for `slot`.
pub fn build_context_set<T: 'static>(
    store: &mut Store<T>,
    slot: usize,
    signature: &CoreSignature,
    context: ContextSlots,
) -> RuntimeFunc {
    RuntimeFunc::new(
        store.inner_mut(),
        core_func_type(signature),
        move |_store_ctx, args, _results| {
            let value = arg_u32(args, 0)? as i32;
            let mut slots = context
                .0
                .lock()
                .map_err(|_| Error::internal("context slots lock poisoned"))?;
            let target = slots
                .get_mut(slot)
                .ok_or_else(|| Error::internal("context slot index out of range"))?;
            *target = value;
            Ok(())
        },
    )
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
/// callee instance. A call between two components opens a call
/// scope like a call across the host boundary does.
pub fn build_enter_sync_call<T: 'static>(
    store: &mut Store<T>,
    signature: &CoreSignature,
) -> RuntimeFunc {
    let tables = store.tables_handle();
    RuntimeFunc::new(
        store.inner_mut(),
        core_func_type(signature),
        move |_store_ctx, _args, _results| {
            tables
                .lock()
                .map_err(|_| anyhow!("resource handle tables lock poisoned"))?
                .enter_call();
            Ok(())
        },
    )
}

/// Build the `exit-sync-call` intrinsic. See
/// [`build_enter_sync_call`]: the scope is validated and closed, and
/// a borrow the callee did not drop traps with the message Wasmtime
/// uses.
pub fn build_exit_sync_call<T: 'static>(
    store: &mut Store<T>,
    signature: &CoreSignature,
) -> RuntimeFunc {
    let tables = store.tables_handle();
    RuntimeFunc::new(
        store.inner_mut(),
        core_func_type(signature),
        move |_store_ctx, _args, _results| {
            let outcome = tables
                .lock()
                .map_err(|_| anyhow!("resource handle tables lock poisoned"))?
                .exit_call();
            match outcome {
                Ok(()) => Ok(()),
                Err(_) => Err(anyhow!(
                    "wasm trap: borrow handles still remain at the end of the call"
                )),
            }
        },
    )
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
        if matches!(entry.kind, HandleKind::Own { .. }) {
            guard.lend(src.table, index);
        }
        entry.rep
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
            transcode(store_ctx, op, &from, &to, args, results)
                .map_err(|err| anyhow!("string transcoder failed: {err}"))
        },
    )
}

fn transcode<T: 'static>(
    mut ctx: StoreContextMut<'_, T, Backend>,
    op: TranscodeOp,
    from: &Memory,
    to: &Memory,
    args: &[RuntimeVal],
    results: &mut [RuntimeVal],
) -> Result<()> {
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

fn arg_usize(args: &[RuntimeVal], index: usize) -> Result<usize> {
    arg_u32(args, index).map(|v| v as usize)
}

fn set_results(results: &mut [RuntimeVal], values: &[usize]) -> Result<()> {
    if results.len() != values.len() {
        return Err(Error::internal("intrinsic result arity mismatch"));
    }
    for (slot, value) in results.iter_mut().zip(values) {
        *slot = RuntimeVal::I32(*value as u32 as i32);
    }
    Ok(())
}

fn invalid(message: &str) -> Error {
    Error::internal(message)
}
