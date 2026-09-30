//! The built-ins that create and drop the ends of a stream or a
//! future.
//!
//! A stream or future is one shared record in the store, and each of
//! its two ends is one end record, which a handle-table entry of the
//! end's kind names. `stream.new` and `future.new` create the three
//! records and put both ends in the calling instance's table, the
//! readable end first. They return the two indices packed into one
//! `i64`: the readable end's in the low half and the writable end's
//! in the high half, which is the reference's
//! `readable | (writable << 32)`.
//!
//! The four drop built-ins, `stream.drop-readable`,
//! `stream.drop-writable`, `future.drop-readable`, and
//! `future.drop-writable`, each take one index and drop the end it
//! names. Each traps when the index names no entry, or an entry that
//! is not an end of the built-in's kind, and when the end's stream or
//! future carries another payload type than the built-in was declared
//! with. The rules of the end itself come after: an end that is
//! copying or cancelling traps as busy, and a writable future end
//! that has not written its value traps, so that a reader always
//! gets one. A drop that traps leaves the entry where it was. A drop
//! that succeeds takes the entry away, frees its index to the
//! instance's free list, and takes the end out of the waitable set
//! it joined. The first of a pair to go marks the shared record
//! dropped and gives the other end the dropped result, when that end
//! is copying as the pending side, is idle, or is a stream end whose
//! completed copy has not been delivered, and the second takes the
//! shared record and both end records out of the store.
//!
//! A readable end whose stream or future the host created, with a
//! producer as its writable end, takes that end with it: the host's
//! end drops as the second of the pair, and the producer is dropped
//! unpolled, because nobody is left to read what it would produce. A
//! writable end whose readable end the host piped to a consumer does
//! the same with the consumer, which is how a consumer learns that
//! the stream ended, as in Wasmtime.
//!
//! Every one of the six traps with the cannot-leave cause when the
//! instance's may-leave flag is clear, which is the case while a
//! `realloc` or a `post-return` of that instance runs.

use std::sync::{Arc, Mutex};

use anyhow::anyhow;

use crate::abi::runtime_state::AbiRuntimeState;
use crate::concurrency::{EndKind, InstanceId, WaitableId};
use crate::error::{CopyCause, Error, TaskCause};
use crate::executor::intrinsics::core_func_type;
use crate::executor::ir::CoreSignature;
use crate::resource::{HandleTables, TableId};
use crate::runtime_layer::host_func;
use crate::runtime_layer::{AsContextMut, Func as RuntimeFunc, Val as RuntimeVal};
use crate::store::StoreContext;
use crate::store::StoreContextInternalExt;
use crate::types::ValueType;

/// Build the `stream.new` built-in for `instance`: a stream carrying
/// `payload` enters the store, and the built-in returns the indices
/// of its two ends in the instance's handle table.
pub fn build_stream_new<T: 'static>(
    store: &mut StoreContext<'_, T>,
    instance: usize,
    payload: Option<ValueType>,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> crate::error::Result<RuntimeFunc> {
    build_new(
        store,
        instance,
        payload,
        [EndKind::StreamReadable, EndKind::StreamWritable],
        signature,
        abi_state,
    )
}

/// Build the `future.new` built-in for `instance`, as
/// [`build_stream_new`] builds `stream.new`.
pub fn build_future_new<T: 'static>(
    store: &mut StoreContext<'_, T>,
    instance: usize,
    payload: Option<ValueType>,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> crate::error::Result<RuntimeFunc> {
    build_new(
        store,
        instance,
        payload,
        [EndKind::FutureReadable, EndKind::FutureWritable],
        signature,
        abi_state,
    )
}

/// Build the drop built-in of `kind` for `instance`: the named end's
/// entry leaves the instance's handle table and the end is dropped.
pub fn build_drop_end<T: 'static>(
    store: &mut StoreContext<'_, T>,
    kind: EndKind,
    instance: usize,
    payload: Option<ValueType>,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> crate::error::Result<RuntimeFunc> {
    let tables = store.internal().tables_handle();
    host_func(
        store.internal().runtime_mut(),
        core_func_type(signature),
        move |mut store_ctx, args, _results| {
            let index = arg_u32(args, 0)?;
            let (id, table) = calling_instance(&abi_state, instance)?;
            trap_if_cannot_leave(&abi_state, id, &mut store_ctx)?;
            let mut guard = lock_tables(&tables)?;
            let end = guard
                .end_from_handle(table, index, kind)
                .map_err(|err| anyhow!("{err}"))?;
            let carried = guard
                .tasks
                .shared_record(end)
                .map(|record| &record.payload)
                .ok_or_else(|| anyhow!("an end's entry names an end with no shared record"))?;
            if *carried != payload {
                return Err(trap(Error::Copy(CopyCause::PayloadMismatch { kind })));
            }
            // An end whose other end the host serves takes the host's
            // end with it: nobody is left to read what a producer would
            // produce, or to write what a consumer would take.
            let host_end = guard.tasks.host_counterpart(end);
            // The record's own checks come first: a busy end, and a
            // writable future end that has not written, trap and
            // keep their entry.
            guard
                .tasks
                .drop_waitable(WaitableId::from_end(kind, end))
                .map_err(trap)?;
            guard.remove(table, index);
            let Some(host_end) = host_end else {
                return Ok(());
            };
            guard.tasks.release_host_end(host_end).map_err(trap)?;
            drop(guard);
            // The producer or consumer is dropped with no lock held,
            // because its own drop runs host code. Letting it go
            // forgets the waker kept for it too.
            let mut store = StoreContext::new(store_ctx.as_context_mut());
            let scheduler = store.internal().scheduler_mut();
            match kind {
                EndKind::StreamReadable | EndKind::FutureReadable => {
                    drop(scheduler.release_host_writer(host_end));
                }
                EndKind::StreamWritable | EndKind::FutureWritable => {
                    drop(scheduler.release_host_reader(host_end));
                }
            }
            Ok(())
        },
    )
}

/// Build `stream.new` or `future.new`, whose two ends are of the two
/// `kinds`, the readable kind first.
fn build_new<T: 'static>(
    store: &mut StoreContext<'_, T>,
    instance: usize,
    payload: Option<ValueType>,
    kinds: [EndKind; 2],
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> crate::error::Result<RuntimeFunc> {
    let tables = store.internal().tables_handle();
    host_func(
        store.internal().runtime_mut(),
        core_func_type(signature),
        move |mut store_ctx, _args, results| {
            let (id, table) = calling_instance(&abi_state, instance)?;
            trap_if_cannot_leave(&abi_state, id, &mut store_ctx)?;
            let mut guard = lock_tables(&tables)?;
            let (readable, writable) = guard.tasks.insert_ends(payload.clone()).map_err(trap)?;
            let readable = guard.insert_end(table, kinds[0], readable);
            let writable = guard.insert_end(table, kinds[1], writable);
            results[0] = RuntimeVal::I64(pack_indices(readable, writable));
            Ok(())
        },
    )
}

/// The word `stream.new` and `future.new` return: the readable end's
/// index in the low half and the writable end's in the high half.
fn pack_indices(readable: u32, writable: u32) -> i64 {
    (u64::from(readable) | (u64::from(writable) << 32)) as i64
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
/// error's message with its chain flattened into it, because a trap
/// crosses back into guest code as a string. The conformance corpora
/// match the message by substring.
fn trap(error: Error) -> anyhow::Error {
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
        _ => Err(anyhow!(
            "a stream or future built-in expected an i32 argument"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    fn it_packs_the_readable_index_low_and_the_writable_index_high() {
        let packed = pack_indices(1, 2) as u64;
        assert_eq!(packed & 0xffff_ffff, 1);
        assert_eq!(packed >> 32, 2);
        assert_eq!(
            pack_indices(u32::MAX, u32::MAX),
            -1,
            "both halves fill the whole word"
        );
    }
}
