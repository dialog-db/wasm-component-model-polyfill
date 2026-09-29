//! The built-ins that copy values through a stream or a future,
//! `stream.read`, `stream.write`, `future.read`, and `future.write`,
//! and the four that cancel a copy, `stream.cancel-read`,
//! `stream.cancel-write`, `future.cancel-read`, and
//! `future.cancel-write`.
//!
//! The stream built-ins are the reference's `stream_copy` and the
//! future built-ins its `future_copy`, which is the same copy with a
//! count of one and no partial step. A stream built-in takes the index
//! of an end, a pointer into the memory its canon options name, and a
//! count; a future built-in takes the index and the pointer. Each
//! returns one word: the packed result of the copy, or the blocked
//! sentinel `0xffffffff` when the copy has not finished and the
//! built-in was declared `async`. A future's packed result always
//! counts zero values.
//!
//! The checks run in the reference's order, and each trap carries
//! Wasmtime's message:
//!
//! 1. The instance's may-leave flag must be set, or the built-in
//!    fails with the cannot-leave cause.
//! 2. The index must name an end of the built-in's kind.
//! 3. The end's stream or future must carry the payload the built-in
//!    declares.
//! 4. The end must be idle: a copy whose event has not been delivered
//!    fails the built-in with the concurrent-operation cause.
//! 5. An end that is done can copy no more. An end that reported the
//!    other end dropped fails with the message for its direction. A
//!    future end that is done because its one copy completed fails
//!    with the future's message for its direction, which for a write
//!    names both ways a future's writable end becomes done.
//! 6. A synchronous copy on an end in a waitable set fails with the
//!    waitable cause of a synchronous use of a waitable in a set.
//! 7. The count must be below 2^28.
//!
//! The reference checks the end's state, which is not idle when the
//! end is busy or done, before it checks the set, and Wasmtime asks
//! about the set only once a synchronous copy would block, which is
//! after it asks whether the end is done. So a done end in a set
//! fails as done here, as in both.
//!
//! The guest's buffer is then built eagerly, through a boundary
//! context over the memory with no borrow scope, because a payload
//! holds no borrow. When the payload is present and the count is
//! above zero, the pointer must be aligned for the payload and the
//! whole range must lie inside the memory.
//!
//! The store's records then pair the copy with the other end, as the
//! reference's shared stream and shared future do. When the other end
//! is pending with room left, values move at once, along the path the
//! translator selected from the payload type:
//!
//! - A payload of a number type, `s8` to `u64`, `f32`, or `f64`, moves
//!   as bytes: one runtime-layer read of the writer's memory and one
//!   write of the reader's, with no value built. Every bit pattern of
//!   those types is a valid value, so the bytes are the same as a
//!   value copy would give. `bool` and `char` are left out because not
//!   every bit pattern is valid for them, which is the set Wasmtime's
//!   compiler copies in one step. A copy that builds no value charges
//!   no copy budget, as Wasmtime's charges none.
//! - Any other payload moves one value at a time: the writer's context
//!   lifts them out of the writer's memory and the reader's context
//!   lowers them into the reader's, and an owned handle in a value
//!   moves from the writer's table to the reader's as a call moves
//!   one. An error context in a value is copied instead: the writer
//!   keeps its handle, and the reader gains one of its own over the
//!   same record. The two contexts are marked as the sides of a move
//!   between two guests, which is the one crossing an error context
//!   makes through a value. The lift charges the writer's context's
//!   copy budget, so a copy cannot make the host build values without
//!   bound.
//!
//! The same set gates a read and a write from one instance. The
//! reference traps, as a temporary rule, when a read or a write finds
//! the other end pending with a copy its own instance started and the
//! payload is not a number type. The store's records check it as soon
//! as they find the pending side, before any count is looked at, so a
//! copy that asks for nothing, or that finds the pending side full,
//! fails too. Wasmtime does the same: its copy compares the two
//! instances whatever the count, and the failure carries its message.
//! A payload of a number type, or none, copies within an instance.
//!
//! A read whose stream or future the host created meets no guest
//! writer. Its writable end is a producer the host serves, and the
//! read polls it once before the built-in returns, as a host task:
//! see `host_copy`. A poll that is ready completes the read here, and
//! a pending one leaves the read to a later turn. A write whose
//! readable end the host piped to a consumer meets no guest reader in
//! the same way, and polls the consumer: see `host_consume`.
//!
//! Such a read or write is never paired with a copy. The host's end
//! starts none and holds no buffer, so the guest's copy finds no
//! pending side and waits as one. Neither path above moves its
//! values, whatever the payload: the producer's delivery lowers them
//! into the reader's memory, and the consumer's source lifts them out
//! of the writer's, one value at a time, a number type included. And
//! the same-instance rule, which compares the instances of two
//! copies, never meets it: the host is no instance, even when the
//! guest that copies created the stream or future. Wasmtime does the
//! same: a guest's read against a host writer polls the producer and
//! lowers what it gives, a guest's write against a host reader hands
//! the consumer a source whose read lifts typed values, and neither
//! reaches the copy that compares instances and moves a number
//! payload's bytes.
//!
//! A copy that finished has an event on its end, and the built-in
//! takes it and returns its packed result. A copy that did not
//! finish returns the blocked sentinel when the built-in is `async`,
//! and leaves the event to the waitable set the end joins or to a
//! later cancel. Otherwise the thread blocks through the suspend seam
//! until the end holds an event, under the rules the seam states for
//! every blocking built-in, and then takes it.
//!
//! The cancels are the reference's `cancel_copy`. Each takes the index
//! of an end and returns the packed result of the copy it ends, or
//! the blocked sentinel. Its checks run in the reference's order: the
//! may-leave flag, the kind of the entry, and the payload, as a copy
//! checks them; then the end must be copying, with no thread waiting
//! on the copy synchronously, or the cancel fails with the
//! no-copy-pending cause; and a synchronous cancel on an end in a
//! waitable set fails with the waitable cause.
//!
//! The end then moves to `cancelling`. A copy that already completed
//! left its event on the end, and the cancel returns it with the
//! progress the copy made: as the cancelled result on a stream,
//! because the cancel ended the copy whatever it moved first, and as
//! the completed result on a future, whose one value moved. That is
//! what Wasmtime's cancel returns. A copy that found the other end
//! dropped returns the dropped result. A copy that is still the
//! pending side of a stream or future between guests stops being it
//! and takes the cancelled result, with the progress made so far.
//! Either way the cancel takes the event as a copy does, and the end
//! is idle afterwards unless the event reported the other end
//! dropped.
//!
//! A read that waits on a producer the host serves holds no event
//! yet, and the producer has to answer the cancel first, as in
//! Wasmtime's `cancel_read` against a host writer. The cancel wakes
//! the host task that polls the producer, whose next poll is asked to
//! finish, and the delivery that follows completes the read with the
//! progress made (see `host_copy`). A write that waits on a consumer
//! the host serves is cancelled the same way, as Wasmtime's
//! `cancel_write` against a host reader, and completes with what the
//! consumer took (see `host_consume`). Meanwhile an `async` cancel
//! returns the blocked sentinel, and a synchronous one blocks through
//! the suspend seam, as a copy does. A second cancel while the first
//! waits traps with the no-copy-pending cause, as the reference's
//! `cancel_copy` does; Wasmtime allows it.

use std::sync::{Arc, Mutex};

use anyhow::anyhow;

use crate::abi::context::BoundaryContext;
use crate::abi::instance::BoundaryInstance;
use crate::abi::layout::{alignment_of, size_of};
use crate::abi::runtime_state::AbiRuntimeState;
use crate::abi::{lift_list, lower};
use crate::concurrency::{
    BlockStep, BlockingBuiltin, CopyBuffer, CopyState, EndId, EndKind, InstanceId, Pairing,
    Readiness, WaitableId,
};
use crate::error::{AbiPosition, CopyCause, Error, TaskCause, WaitableCause};
use crate::executor::intrinsics::core_func_type;
use crate::executor::ir::{CanonOptions, CoreSignature};
use crate::internal::ErrorInternal;
use crate::resource::{HandleTables, TableId};
use crate::runtime_layer::{
    AsContextMut, Backend, StoreContextMut as RuntimeContextMut, Val as RuntimeVal,
};
use crate::store::StoreContextInternalExt;
use crate::store::{StoreContext, StoreData};
use crate::types::{ListType, ValueType};
use crate::value::Val;

use super::host_consume::serve_host_write;
use super::host_copy::serve_host_read;

/// The word a copy returns when it has not finished: the reference's
/// `BLOCKED`. No packed result equals it, because a count never
/// reaches 2^28.
const BLOCKED: u32 = 0xffff_ffff;

/// The count a copy must stay below: 2^28, the reference's
/// `Buffer.MAX_LENGTH` plus one.
const COUNT_LIMIT: u32 = 1 << 28;

/// The argument slot of the buffer's pointer, which labels a failure
/// of the values that move through it.
const POINTER_ARGUMENT: AbiPosition = AbiPosition::Argument(1);

/// Build the copy built-in on an end of `kind`, declared with
/// `options` for a stream or future of `payload`: `stream.read` for
/// [`EndKind::StreamReadable`], `stream.write` for
/// [`EndKind::StreamWritable`], `future.read` for
/// [`EndKind::FutureReadable`], and `future.write` for
/// [`EndKind::FutureWritable`].
pub fn build_copy<T: 'static>(
    store: &mut StoreContext<'_, T>,
    kind: EndKind,
    options: &CanonOptions,
    payload: Option<ValueType>,
    copies_bytes: bool,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> BlockingBuiltin<T> {
    let tables = store.internal().tables_handle();
    let builtin = Builtin {
        kind,
        options: Arc::new(options.clone()),
        payload,
        copies_bytes,
        abi_state,
        tables,
    };
    BlockingBuiltin::new(core_func_type(signature), move |store, args| {
        builtin.copy(store, args)
    })
}

/// Build the cancel built-in on an end of `kind` for `instance`,
/// declared `async` when `async_` is set, for a stream or future of
/// `payload`: `stream.cancel-read` for [`EndKind::StreamReadable`],
/// `stream.cancel-write` for [`EndKind::StreamWritable`],
/// `future.cancel-read` for [`EndKind::FutureReadable`], and
/// `future.cancel-write` for [`EndKind::FutureWritable`].
pub fn build_cancel_copy<T: 'static>(
    store: &mut StoreContext<'_, T>,
    kind: EndKind,
    instance: usize,
    async_: bool,
    payload: Option<ValueType>,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> BlockingBuiltin<T> {
    let tables = store.internal().tables_handle();
    BlockingBuiltin::new(
        core_func_type(signature),
        move |store: &mut StoreContext<'_, T>, args: &[RuntimeVal]| {
            let index = arg_u32(args, 0)?;
            let (id, table) = calling_instance(&abi_state, instance)?;
            trap_if_cannot_leave(&abi_state, id, store.internal().runtime_mut())?;
            let (waitable, host_end) = {
                let mut guard = lock_tables(&tables)?;
                let end = copying_end(&guard, kind, &payload, async_, table, index)?;
                let host_end = guard.tasks.cancel_copy(kind, end).map_err(trap)?;
                (WaitableId::from_end(kind, end), host_end)
            };
            // A copy that waits on a host producer or consumer ends
            // when it answers a poll asked to finish: wake the host
            // task that polls it, as Wasmtime wakes the cancel waker.
            if let Some(host_end) = host_end {
                let waker = store
                    .internal()
                    .scheduler_mut()
                    .take_host_end_waker(host_end);
                if let Some(waker) = waker {
                    waker.wake();
                }
            }
            settle(&tables, waitable, async_)
        },
    )
}

/// The end the entry at `index` of `table` names, when a cancel of
/// kind `kind`, declared with `payload` and `async_`, may run on it.
/// The checks run in the reference's order: the entry must name an
/// end of the kind, the end must carry the payload, the end must be
/// copying with no thread waiting on it synchronously, and a
/// synchronous cancel must not name an end in a waitable set.
fn copying_end(
    tables: &HandleTables,
    kind: EndKind,
    payload: &Option<ValueType>,
    async_: bool,
    table: TableId,
    index: u32,
) -> anyhow::Result<EndId> {
    let end = tables
        .end_from_handle(table, index, kind)
        .map_err(|err| anyhow!("{err}"))?;
    let carried = tables
        .tasks
        .shared_record(end)
        .map(|record| &record.payload)
        .ok_or_else(|| anyhow!("an end's entry names an end with no shared record"))?;
    if carried != payload {
        return Err(trap(Error::Copy(CopyCause::PayloadMismatch { kind })));
    }
    let record = tables
        .tasks
        .end(end)
        .ok_or_else(|| anyhow!("an end's entry names an end that is not in the store"))?;
    if record.state != CopyState::Copying || record.waitable.synchronous_waiter {
        return Err(trap(Error::Copy(CopyCause::NoCopyPending { kind })));
    }
    if record.waitable.set.is_some() && !async_ {
        return Err(trap(Error::Waitable(WaitableCause::SyncAndAsync)));
    }
    Ok(end)
}

/// What a copy or a cancel on `waitable` comes to once the store's
/// records have done their part. An end that holds the event of its
/// copy gives it up, and the built-in is done with the packed result
/// it carries. Otherwise an `async` built-in is done with the blocked
/// sentinel and leaves the event to come to the waitable set the end
/// joins or to a later cancel, and any other waits until the end
/// holds an event, under the rules the suspend seam states for every
/// blocking built-in, and then takes it.
fn settle<T: 'static>(
    tables: &Arc<Mutex<HandleTables>>,
    waitable: WaitableId,
    async_: bool,
) -> anyhow::Result<BlockStep<T>> {
    {
        let mut guard = lock_tables(tables)?;
        if guard.tasks.has_pending_event(waitable).map_err(trap)? {
            return Ok(BlockStep::Ready(word(take_result(&mut guard, waitable)?)));
        }
        if async_ {
            return Ok(BlockStep::Ready(word(BLOCKED)));
        }
        guard.tasks.begin_synchronous_wait(waitable).map_err(trap)?;
    }
    let tables = tables.clone();
    Ok(BlockStep::wait(
        Readiness::Waitable { waitable },
        move |_store: &mut StoreContext<'_, T>, waited| {
            let mut guard = lock_tables(&tables)?;
            let ended = guard.tasks.end_synchronous_wait(waitable);
            waited.map_err(trap)?;
            ended.map_err(trap)?;
            Ok(word(take_result(&mut guard, waitable)?))
        },
    ))
}

/// The one result of a copy or a cancel: the word the guest receives.
fn word(value: u32) -> Vec<RuntimeVal> {
    vec![RuntimeVal::I32(value as i32)]
}

/// What one declaration of a copy built-in carries into every call
/// of it.
struct Builtin {
    /// The kind of end the built-in copies on.
    kind: EndKind,
    /// The canon options the built-in was declared with.
    options: Arc<CanonOptions>,
    /// The payload type the built-in was declared with.
    payload: Option<ValueType>,
    /// Whether the payload is a number type or absent, which the
    /// translator decided from it: a copy then moves bytes, and a
    /// read and a write from one instance may meet.
    copies_bytes: bool,
    /// The runtime state of the instantiation the options index.
    abi_state: Arc<Mutex<AbiRuntimeState>>,
    /// The store's handle tables and records.
    tables: Arc<Mutex<HandleTables>>,
}

impl Builtin {
    /// One call of the built-in: the checks, the buffer, the pairing,
    /// and the word the guest receives.
    fn copy<T: 'static>(
        &self,
        store: &mut StoreContext<'_, T>,
        args: &[RuntimeVal],
    ) -> anyhow::Result<BlockStep<T>> {
        let index = arg_u32(args, 0)?;
        let pointer = arg_u32(args, 1)?;
        // A future carries one value, so its built-ins take no count.
        let count = if matches!(self.kind, EndKind::FutureReadable | EndKind::FutureWritable) {
            1
        } else {
            arg_u32(args, 2)?
        };
        let (id, table) = calling_instance(&self.abi_state, self.options.instance)?;
        trap_if_cannot_leave(&self.abi_state, id, store.internal().runtime_mut())?;
        let end = self.idle_end(table, index, count)?;
        let buffer = self.guest_buffer(store.internal().runtime_mut(), id, pointer, count)?;

        let pairing = lock_tables(&self.tables)?
            .tasks
            .start_copy(self.kind, end, buffer)
            .map_err(trap)?;
        if let Pairing::Move {
            writer,
            reader,
            count,
        } = pairing
        {
            move_values(
                store.internal().runtime_mut(),
                &self.tables,
                writer,
                reader,
                count,
                self.copies_bytes,
            )?;
            lock_tables(&self.tables)?
                .tasks
                .finish_move(self.kind, end, count)
                .map_err(trap)?;
        }

        let waitable = WaitableId::from_end(self.kind, end);
        let host_copy = {
            let guard = lock_tables(&self.tables)?;
            match guard.tasks.host_counterpart(end) {
                Some(host) if !guard.tasks.has_pending_event(waitable).map_err(trap)? => Some(host),
                _ => None,
            }
        };
        if let Some(host) = host_copy {
            match self.kind {
                EndKind::StreamReadable | EndKind::FutureReadable => serve_host_read(store, host),
                EndKind::StreamWritable | EndKind::FutureWritable => {
                    serve_host_write(store, host, true)
                }
            }
            .map_err(trap)?;
        }

        settle(&self.tables, waitable, self.options.async_)
    }

    /// The end the entry at `index` of `table` names, when a copy of
    /// `count` values may start on it: checks 2 to 7 of the module's
    /// list, in that order.
    fn idle_end(&self, table: TableId, index: u32, count: u32) -> anyhow::Result<EndId> {
        let kind = self.kind;
        let guard = lock_tables(&self.tables)?;
        let end = guard
            .end_from_handle(table, index, kind)
            .map_err(|err| anyhow!("{err}"))?;
        let carried = guard
            .tasks
            .shared_record(end)
            .map(|record| &record.payload)
            .ok_or_else(|| anyhow!("an end's entry names an end with no shared record"))?;
        if *carried != self.payload {
            return Err(trap(Error::Copy(CopyCause::PayloadMismatch { kind })));
        }
        let record = guard
            .tasks
            .end(end)
            .ok_or_else(|| anyhow!("an end's entry names an end that is not in the store"))?;
        if record.state.busy() {
            return Err(trap(Error::Copy(CopyCause::ConcurrentOperation)));
        }
        if record.state == CopyState::Done {
            // Wasmtime asks first whether the end was told the other
            // end dropped, and only then whether a future's one copy
            // is over, so a future end that learned of the drop gets
            // the stream's wording too.
            return Err(trap(Error::Copy(match kind {
                EndKind::StreamWritable => CopyCause::WriteAfterDropped,
                EndKind::StreamReadable => CopyCause::ReadAfterDropped,
                EndKind::FutureWritable if record.notified_dropped => CopyCause::WriteAfterDropped,
                EndKind::FutureReadable if record.notified_dropped => CopyCause::ReadAfterDropped,
                EndKind::FutureWritable => CopyCause::FutureWriteAfterDone,
                EndKind::FutureReadable => CopyCause::FutureReadAfterDone,
            })));
        }
        if record.waitable.set.is_some() && !self.options.async_ {
            return Err(trap(Error::Waitable(WaitableCause::SyncAndAsync)));
        }
        if count >= COUNT_LIMIT {
            return Err(trap(Error::Copy(CopyCause::CountTooLarge)));
        }
        Ok(end)
    }

    /// Build the guest's buffer of `count` values at `pointer`, the
    /// reference's `BufferGuestImpl`. When the payload is present and
    /// the count is above zero, the pointer must be aligned for the
    /// payload and the range must lie inside the memory, which a
    /// boundary context over that memory measures. The buffer records
    /// `caller`, the calling instance, for the store's records to
    /// compare with the other end's.
    fn guest_buffer<T: 'static>(
        &self,
        store_ctx: &mut RuntimeContextMut<'_, StoreData<T>, Backend>,
        caller: InstanceId,
        pointer: u32,
        count: u32,
    ) -> anyhow::Result<CopyBuffer> {
        if let Some(payload) = &self.payload
            && count > 0
        {
            let kind = self.kind;
            if !(pointer as usize).is_multiple_of(alignment_of(payload)) {
                return Err(trap(Error::Copy(CopyCause::BufferNotAligned { kind })));
            }
            let (options, instance) =
                BoundaryInstance::resolve(&self.options, &self.abi_state, &self.tables)
                    .map_err(trap)?;
            let mut ctx = BoundaryContext::new(store_ctx.as_context_mut(), options, instance, None);
            let end = u64::from(pointer) + u64::from(count) * size_of(payload) as u64;
            if ctx.memory_size().is_none_or(|size| end > size as u64) {
                return Err(trap(Error::Copy(CopyCause::BufferOutOfBounds { kind })));
            }
        }
        Ok(CopyBuffer {
            payload: self.payload.clone(),
            options: self.options.clone(),
            abi_state: self.abi_state.clone(),
            instance: caller,
            number_or_none: self.copies_bytes,
            pointer,
            length: count,
            progress: 0,
        })
    }
}

/// Where one side of a move reads or writes: the side's canon options
/// and runtime state, the payload type its built-in declared, and the
/// address of the first value the move touches.
struct MoveSide {
    /// The canon options of the built-in that started the side's
    /// copy.
    options: Arc<CanonOptions>,
    /// The runtime state of the instantiation the options index.
    abi_state: Arc<Mutex<AbiRuntimeState>>,
    /// The payload type the side's built-in declared.
    payload: Option<ValueType>,
    /// The address of the first value the move touches.
    offset: usize,
}

impl MoveSide {
    /// The side of `end`'s copy, from where the copy has got to.
    fn of(tables: &HandleTables, end: EndId) -> anyhow::Result<Self> {
        let buffer = tables
            .tasks
            .end(end)
            .and_then(|record| record.buffer.as_ref())
            .ok_or_else(|| anyhow!("a moving copy's end holds no buffer"))?;
        let size = buffer.payload.as_ref().map(size_of).unwrap_or(0);
        Ok(Self {
            options: buffer.options.clone(),
            abi_state: buffer.abi_state.clone(),
            payload: buffer.payload.clone(),
            offset: buffer.pointer as usize + buffer.progress as usize * size,
        })
    }

    /// A boundary context over this side's memory, with no borrow
    /// scope, marked as one side of a move between two guests.
    fn context<'a, T: 'static>(
        &self,
        store_ctx: RuntimeContextMut<'a, StoreData<T>, Backend>,
        tables: &Arc<Mutex<HandleTables>>,
    ) -> anyhow::Result<BoundaryContext<'a, StoreData<T>>> {
        let (options, instance) =
            BoundaryInstance::resolve(&self.options, &self.abi_state, tables).map_err(trap)?;
        Ok(BoundaryContext::new(store_ctx, options, instance, None).between_guests())
    }
}

/// Move `count` values out of `writer`'s buffer and into `reader`'s,
/// each from where its copy has got to. A stream or future that
/// carries no values moves nothing but the count.
///
/// A read and a write from one instance never reach here with a
/// payload that is not a number type: the store's records refuse the
/// copy when they find the pending side, whatever the counts.
///
/// A payload of a number type moves as bytes, see [`move_bytes`].
/// Any other moves as values: the writer's context lifts them,
/// charging its copy budget as a list of the same values would, and
/// the reader's context lowers them one at a time, which moves an
/// owned handle from the writer's table to the reader's as a call
/// moves one.
fn move_values<T: 'static>(
    store_ctx: &mut RuntimeContextMut<'_, StoreData<T>, Backend>,
    tables: &Arc<Mutex<HandleTables>>,
    writer: EndId,
    reader: EndId,
    count: u32,
    copies_bytes: bool,
) -> anyhow::Result<()> {
    let (source, destination) = {
        let guard = lock_tables(tables)?;
        (MoveSide::of(&guard, writer)?, MoveSide::of(&guard, reader)?)
    };
    let (Some(source_ty), Some(destination_ty)) = (&source.payload, &destination.payload) else {
        return Ok(());
    };
    let count = count as usize;
    if copies_bytes {
        return move_bytes(
            store_ctx,
            tables,
            &source,
            &destination,
            count * size_of(source_ty),
        );
    }
    let values = {
        let mut ctx = source.context(store_ctx.as_context_mut(), tables)?;
        let list = ValueType::List(ListType::new(source_ty.clone()));
        match lift_list(
            &mut ctx,
            source.offset,
            count,
            source_ty,
            &list,
            POINTER_ARGUMENT,
        )
        .map_err(trap)?
        {
            Val::List(values) => values,
            _ => return Err(trap(Error::internal("a lifted list is not a list"))),
        }
    };
    let mut ctx = destination.context(store_ctx.as_context_mut(), tables)?;
    let size = size_of(destination_ty);
    ctx.lowering_within(
        destination.offset,
        count * size,
        POINTER_ARGUMENT,
        destination_ty,
        |ctx| {
            for (i, value) in values.iter().enumerate() {
                lower(
                    ctx,
                    destination.offset + i * size,
                    value,
                    destination_ty,
                    POINTER_ARGUMENT,
                )?;
            }
            Ok(())
        },
    )
    .map_err(trap)
}

/// Move the `length` bytes at `source`'s offset to `destination`'s,
/// which is how values of a number type move: every bit pattern of
/// those types is a valid value, so the bytes are the values, and no
/// value is built. One context over the two memories reads the whole
/// range out of the writer's memory in one runtime-layer read and
/// writes it into the reader's in one write. The copy builds no host
/// value, so it charges no copy budget, as Wasmtime's copy of a flat
/// payload charges none. Both ranges were checked to lie inside their
/// memories when their copies started, and a memory never shrinks.
/// When the two sides share a memory the ranges can overlap, and the
/// whole range is read before any of it is written, so the reader
/// sees the bytes the writer offered.
///
/// The read lands in a transient host buffer the size of the copy,
/// which the eager strategy allocates for every load, and the write
/// takes it from there. The runtime layer offers no view of a guest
/// memory, only reads into and writes out of host bytes, so the bytes
/// cannot move from memory to memory in place, as Wasmtime moves
/// them. The buffer is bounded: its length is the count, below 2^28,
/// times the size of a number type, and both ranges it spans were
/// checked against their memories when their copies started.
fn move_bytes<T: 'static>(
    store_ctx: &mut RuntimeContextMut<'_, StoreData<T>, Backend>,
    tables: &Arc<Mutex<HandleTables>>,
    source: &MoveSide,
    destination: &MoveSide,
    length: usize,
) -> anyhow::Result<()> {
    let (source_options, _) =
        BoundaryInstance::resolve(&source.options, &source.abi_state, tables).map_err(trap)?;
    let (destination_options, instance) =
        BoundaryInstance::resolve(&destination.options, &destination.abi_state, tables)
            .map_err(trap)?;
    let mut ctx = BoundaryContext::for_copy(
        store_ctx.as_context_mut(),
        destination_options,
        source_options,
        instance,
        None,
    );
    let bytes = ctx.read_source_bytes(source.offset, length).map_err(trap)?;
    ctx.write_own_bytes(destination.offset, &bytes)
        .map_err(trap)
}

/// Take the event of the finished copy on `waitable` and answer the
/// packed result it carries.
fn take_result(tables: &mut HandleTables, waitable: WaitableId) -> anyhow::Result<u32> {
    let event = tables
        .take_event(waitable)
        .map_err(trap)?
        .ok_or_else(|| anyhow!("a finished copy left no event on its end"))?;
    Ok(event.payloads()[1])
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

/// The trap a structured error becomes on its way to the guest. The
/// message is the error's own, which the conformance corpora match
/// by substring.
///
/// A scheduler cause, which a blocked synchronous copy fails with,
/// takes the `wasm trap:` prefix a trap reaching guest code renders
/// with, as the other blocking built-ins give it. Every other error
/// has its chain flattened into the message, because a trap crosses
/// back into guest code as a string: an error that carries the trap
/// of the work a nested turn ran would otherwise reach the host as
/// the wrapper alone.
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
        _ => Err(anyhow!("a copy built-in expected an i32 argument")),
    }
}
