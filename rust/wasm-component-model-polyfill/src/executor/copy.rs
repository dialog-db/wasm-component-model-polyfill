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
//! is pending with room left, values move at once: the writer's
//! context lifts them out of the writer's memory and the reader's
//! context lowers them into the reader's, one value at a time, and an
//! owned handle in a value moves from the writer's table to the
//! reader's as a call moves one. The lift charges the writer's
//! context's copy budget, so a copy cannot make the host build values
//! without bound.
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
//! pending side of its stream or future stops being it and takes the
//! cancelled result, with the progress made so far. Either way the
//! cancel takes the event as a copy does, and the end is idle
//! afterwards unless the event reported the other end dropped. An end
//! that holds no event even then waits on a party that has to answer
//! the cancel first: an `async` cancel returns the blocked sentinel,
//! and a synchronous one blocks through the suspend seam, as a copy
//! does.

use std::sync::{Arc, Mutex};

use anyhow::anyhow;
use wasm_runtime_layer::{
    AsContextMut, Func as RuntimeFunc, StoreContextMut as RuntimeContextMut, Val as RuntimeVal,
};

use crate::abi::context::BoundaryContext;
use crate::abi::instance::BoundaryInstance;
use crate::abi::layout::{alignment_of, size_of};
use crate::abi::runtime_state::AbiRuntimeState;
use crate::abi::{lift_list, lower};
use crate::backend::Backend;
use crate::concurrency::{
    CopyBuffer, CopyState, EndId, EndKind, InstanceId, Pairing, SuspendSeam, WaitableId,
};
use crate::error::{AbiPosition, CopyCause, Error, TaskCause, WaitableCause};
use crate::executor::intrinsics::core_func_type;
use crate::executor::ir::{CanonOptions, CoreSignature};
use crate::internal::ErrorInternal;
use crate::resource::{HandleTables, TableId};
use crate::store::StoreContextInternalExt;
use crate::store::{StoreContext, StoreData};
use crate::types::{ListType, ValueType};
use crate::value::Val;

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
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> RuntimeFunc {
    let tables = store.internal().tables_handle();
    let builtin = Builtin {
        kind,
        options: Arc::new(options.clone()),
        payload,
        abi_state,
        tables,
    };
    RuntimeFunc::new(
        store.internal().runtime_mut(),
        core_func_type(signature),
        move |store_ctx, args, results| {
            let word = builtin.copy(store_ctx, args)?;
            results[0] = RuntimeVal::I32(word as i32);
            Ok(())
        },
    )
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
) -> RuntimeFunc {
    let tables = store.internal().tables_handle();
    RuntimeFunc::new(
        store.internal().runtime_mut(),
        core_func_type(signature),
        move |mut store_ctx, args, results| {
            let index = arg_u32(args, 0)?;
            let (id, table) = calling_instance(&abi_state, instance)?;
            trap_if_cannot_leave(&abi_state, id, &mut store_ctx)?;
            let waitable = {
                let mut guard = lock_tables(&tables)?;
                let end = copying_end(&guard, kind, &payload, async_, table, index)?;
                guard.tasks.cancel_copy(kind, end).map_err(trap)?;
                WaitableId::from_end(kind, end)
            };
            let word = finish(&mut store_ctx, &tables, waitable, async_)?;
            results[0] = RuntimeVal::I32(word as i32);
            Ok(())
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

/// The word a copy or a cancel on `waitable` returns once the store's
/// records have done their part. An end that holds the event of its
/// copy gives it up, and the word is the packed result it carries.
/// Otherwise an `async` built-in returns the blocked sentinel and
/// leaves the event to come to the waitable set the end joins or to
/// a later cancel, and any other blocks the thread through the
/// suspend seam until the end holds an event, under the rules the
/// seam states for every blocking built-in, and then takes it.
fn finish<T: 'static>(
    store_ctx: &mut RuntimeContextMut<'_, StoreData<T>, Backend>,
    tables: &Arc<Mutex<HandleTables>>,
    waitable: WaitableId,
    async_: bool,
) -> anyhow::Result<u32> {
    {
        let mut guard = lock_tables(tables)?;
        if guard.tasks.has_pending_event(waitable).map_err(trap)? {
            return take_result(&mut guard, waitable);
        }
        if async_ {
            return Ok(BLOCKED);
        }
        guard.tasks.begin_synchronous_wait(waitable).map_err(trap)?;
    }
    let suspended = {
        let mut store = StoreContext::new(store_ctx.as_context_mut());
        SuspendSeam::suspend(&mut store, |store| {
            store
                .internal()
                .lock_tables()
                .ok()
                .and_then(|guard| guard.tasks.has_pending_event(waitable).ok())
                .unwrap_or(false)
        })
    };
    let mut guard = lock_tables(tables)?;
    let ended = guard.tasks.end_synchronous_wait(waitable);
    suspended.map_err(trap)?;
    ended.map_err(trap)?;
    take_result(&mut guard, waitable)
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
        mut store_ctx: RuntimeContextMut<'_, StoreData<T>, Backend>,
        args: &[RuntimeVal],
    ) -> anyhow::Result<u32> {
        let index = arg_u32(args, 0)?;
        let pointer = arg_u32(args, 1)?;
        // A future carries one value, so its built-ins take no count.
        let count = if matches!(self.kind, EndKind::FutureReadable | EndKind::FutureWritable) {
            1
        } else {
            arg_u32(args, 2)?
        };
        let (id, table) = calling_instance(&self.abi_state, self.options.instance)?;
        trap_if_cannot_leave(&self.abi_state, id, &mut store_ctx)?;
        let end = self.idle_end(table, index, count)?;
        let buffer = self.guest_buffer(&mut store_ctx, pointer, count)?;

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
            move_values(&mut store_ctx, &self.tables, writer, reader, count)?;
            lock_tables(&self.tables)?
                .tasks
                .finish_move(self.kind, end, count)
                .map_err(trap)?;
        }

        let waitable = WaitableId::from_end(self.kind, end);
        finish(&mut store_ctx, &self.tables, waitable, self.options.async_)
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
    /// boundary context over that memory measures.
    fn guest_buffer<T: 'static>(
        &self,
        store_ctx: &mut RuntimeContextMut<'_, StoreData<T>, Backend>,
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
    /// scope.
    fn context<'a, T: 'static>(
        &self,
        store_ctx: RuntimeContextMut<'a, StoreData<T>, Backend>,
        tables: &Arc<Mutex<HandleTables>>,
    ) -> anyhow::Result<BoundaryContext<'a, StoreData<T>>> {
        let (options, instance) =
            BoundaryInstance::resolve(&self.options, &self.abi_state, tables).map_err(trap)?;
        Ok(BoundaryContext::new(store_ctx, options, instance, None))
    }
}

/// Move `count` values out of `writer`'s buffer and into `reader`'s,
/// each from where its copy has got to. The writer's context lifts
/// them, charging its copy budget as a list of the same values
/// would, and the reader's context lowers them one at a time. A
/// stream or future that carries no values moves nothing but the
/// count.
fn move_values<T: 'static>(
    store_ctx: &mut RuntimeContextMut<'_, StoreData<T>, Backend>,
    tables: &Arc<Mutex<HandleTables>>,
    writer: EndId,
    reader: EndId,
    count: u32,
) -> anyhow::Result<()> {
    let (source, destination) = {
        let guard = lock_tables(tables)?;
        (MoveSide::of(&guard, writer)?, MoveSide::of(&guard, reader)?)
    };
    let (Some(source_ty), Some(destination_ty)) = (&source.payload, &destination.payload) else {
        return Ok(());
    };
    let count = count as usize;
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
