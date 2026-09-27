//! Canonical-ABI lower: write a [`Val`] into guest memory.
//!
//! The entry point [`lower`] takes a [`BoundaryContext`], a memory
//! offset, and the value plus its declared type. It recurses through
//! compound shapes, calling `cabi_realloc` via the context for
//! heap-allocating value types (string and list).

use crate::abi::context::BoundaryContext;
use crate::abi::layout::{align_to, alignment_of, discriminant_size, size_of};
use crate::abi::lift::{ERROR_CONTEXT_AT_THE_HOST, declared_resource_index};
use crate::abi::strings;
use crate::concurrency::{EndId, EndKind, ErrorContextAny};
use crate::error::{AbiCause, AbiError, AbiPosition, CopyCause, Error, Result};
use crate::internal::{
    ErrorContextAnyInternal, ErrorInternal, FutureAnyInternal, StreamAnyInternal,
};
use crate::resource::{HandleKind, HandleLookupError, HandleTables, ResourceHandle, TableId};
use crate::types::{PrimitiveType, ValueType};
use crate::value::Val;

/// Lower `value` of declared type `ty` into the guest's linear
/// memory at `offset`. Heap-allocating types call into `cabi_realloc`
/// via [`BoundaryContext::allocate_aligned`].
pub fn lower<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    offset: usize,
    value: &Val,
    ty: &ValueType,
    position: AbiPosition,
) -> Result<()> {
    match (ty, value) {
        (ValueType::Primitive(prim), _) => lower_primitive(ctx, offset, value, *prim, position),
        (ValueType::List(list), Val::List(elements)) => {
            let ptr = lower_list(ctx, elements, list.element(), ty, position)?;
            write_pointer_pair(ctx, offset, ptr, elements.len(), position, ty)
        }
        (ValueType::FixedLengthList(fixed), Val::FixedLengthList(items)) => {
            if items.len() != fixed.length() as usize {
                return Err(host_value_mismatch(ty, position));
            }
            let element_ty = fixed.element();
            let element_size = size_of(element_ty);
            for (i, item) in items.iter().enumerate() {
                lower(ctx, offset + i * element_size, item, element_ty, position)?;
            }
            Ok(())
        }
        (ValueType::Map(map), Val::Map(entries)) => {
            // A map is laid out as the list of its entry tuples.
            let list = crate::abi::map_to_entries(entries);
            lower(
                ctx,
                offset,
                &list,
                &crate::abi::map_entries_type(map),
                position,
            )
        }
        (ValueType::Record(record), Val::Record(fields)) => {
            if fields.len() != record.fields().len() {
                return Err(host_value_mismatch(ty, position));
            }
            let mut field_offset = offset;
            for (record_field, val_field) in record.fields().iter().zip(fields.iter()) {
                if record_field.name() != val_field.name {
                    return Err(host_value_mismatch(ty, position));
                }
                field_offset = align_to(field_offset, alignment_of(record_field.ty()));
                lower(
                    ctx,
                    field_offset,
                    &val_field.value,
                    record_field.ty(),
                    position,
                )?;
                field_offset += size_of(record_field.ty());
            }
            Ok(())
        }
        (ValueType::Tuple(tuple), Val::Tuple(elements)) => {
            if elements.len() != tuple.elements().len() {
                return Err(host_value_mismatch(ty, position));
            }
            let mut elem_offset = offset;
            for (elem_ty, elem) in tuple.elements().iter().zip(elements.iter()) {
                elem_offset = align_to(elem_offset, alignment_of(elem_ty));
                lower(ctx, elem_offset, elem, elem_ty, position)?;
                elem_offset += size_of(elem_ty);
            }
            Ok(())
        }
        (
            ValueType::Variant(variant),
            Val::Variant {
                discriminant,
                payload,
            },
        ) => {
            let (tag, case) = variant
                .cases()
                .iter()
                .enumerate()
                .find(|(_, c)| c.name() == discriminant)
                .ok_or_else(|| host_value_mismatch(ty, position))?;
            let disc_size = discriminant_size(variant.cases().len());
            write_discriminant(ctx, offset, tag, disc_size, ty, position)?;
            let payload_align = variant
                .cases()
                .iter()
                .filter_map(|c| c.payload().map(alignment_of))
                .max()
                .unwrap_or(1);
            let payload_offset = align_to(offset + disc_size, payload_align);
            match (case.payload(), payload) {
                (Some(payload_ty), Some(payload_val)) => {
                    lower(ctx, payload_offset, payload_val, payload_ty, position)
                }
                (None, None) => Ok(()),
                _ => Err(host_value_mismatch(ty, position)),
            }
        }
        (ValueType::Option(option), Val::Option(payload)) => {
            let payload_align = alignment_of(option.payload());
            let payload_offset = align_to(offset + 1, payload_align);
            match payload {
                None => ctx.write_bytes(offset, &[0u8], position, ty),
                Some(inner) => {
                    ctx.write_bytes(offset, &[1u8], position, ty)?;
                    lower(ctx, payload_offset, inner, option.payload(), position)
                }
            }
        }
        (ValueType::Result(result_ty), Val::Result(result)) => {
            let payload_align = result_ty
                .ok()
                .map(alignment_of)
                .into_iter()
                .chain(result_ty.err().map(alignment_of))
                .max()
                .unwrap_or(1);
            let payload_offset = align_to(offset + 1, payload_align);
            match result {
                Ok(payload) => {
                    ctx.write_bytes(offset, &[0u8], position, ty)?;
                    match (result_ty.ok(), payload) {
                        (Some(ok_ty), Some(payload_val)) => {
                            lower(ctx, payload_offset, payload_val, ok_ty, position)
                        }
                        (None, None) => Ok(()),
                        _ => Err(host_value_mismatch(ty, position)),
                    }
                }
                Err(payload) => {
                    ctx.write_bytes(offset, &[1u8], position, ty)?;
                    match (result_ty.err(), payload) {
                        (Some(err_ty), Some(payload_val)) => {
                            lower(ctx, payload_offset, payload_val, err_ty, position)
                        }
                        (None, None) => Ok(()),
                        _ => Err(host_value_mismatch(ty, position)),
                    }
                }
            }
        }
        (ValueType::Enum(en), Val::Enum(name)) => {
            let tag = en
                .cases()
                .iter()
                .position(|c| c == name)
                .ok_or_else(|| host_value_mismatch(ty, position))?;
            let disc_size = discriminant_size(en.cases().len());
            write_discriminant(ctx, offset, tag, disc_size, ty, position)
        }
        (ValueType::Flags(flags_ty), Val::Flags(active)) => {
            let mut bytes = vec![0u8; size_of(ty)];
            for name in active.iter() {
                let idx = flags_ty
                    .names()
                    .iter()
                    .position(|n| n == name)
                    .ok_or_else(|| host_value_mismatch(ty, position))?;
                bytes[idx / 8] |= 1 << (idx % 8);
            }
            ctx.write_bytes(offset, &bytes, position, ty)
        }
        (ValueType::Own(_), Val::Own(handle))
        | (ValueType::Borrow(_), Val::Borrow(handle) | Val::Own(handle)) => {
            let index = lower_handle(ctx, handle, ty, position)?;
            ctx.write_bytes(offset, &index.to_le_bytes(), position, ty)
        }
        (ValueType::Stream(_), Val::Stream(stream)) => {
            let index = lower_host_end(ctx, stream.end(), ty, position)?;
            ctx.write_bytes(offset, &index.to_le_bytes(), position, ty)
        }
        (ValueType::Future(_), Val::Future(future)) => {
            let index = lower_host_end(ctx, future.end(), ty, position)?;
            ctx.write_bytes(offset, &index.to_le_bytes(), position, ty)
        }
        (ValueType::ErrorContext, Val::ErrorContext(context)) => {
            let index = lower_error_context(ctx, context, ty, position)?;
            ctx.write_bytes(offset, &index.to_le_bytes(), position, ty)
        }
        _ => Err(host_value_mismatch(ty, position)),
    }
}

/// Lower `context`, an error context another guest's crossing lifted,
/// into the guest's handle table, and return the index it takes
/// there. The entry is a new handle of the guest's own, so the
/// record's count of handles rises by one first; a count past
/// `u32::MAX` fails with Wasmtime's reference-count cause and enters
/// nothing.
///
/// The host holds no error context. A crossing between the host and a
/// guest fails with [`Error::Unsupported`], before it touches the
/// table.
pub fn lower_error_context<T: 'static>(
    ctx: &BoundaryContext<'_, T>,
    context: &ErrorContextAny,
    ty: &ValueType,
    position: AbiPosition,
) -> Result<u32> {
    if !ctx.crosses_between_guests() {
        return Err(Error::unsupported(ERROR_CONTEXT_AT_THE_HOST));
    }
    let (Some(tables), Some(table)) = (ctx.instance().tables(), ctx.instance().handle_table())
    else {
        return Err(Error::from(AbiError {
            position,
            valtype: Some(ty.clone()),
            cause: AbiCause::InvalidHandle {
                reason: "no handle table of the instance is available to the lower context"
                    .to_owned(),
            },
        }));
    };
    let mut guard = tables
        .lock()
        .map_err(|_| Error::internal("resource handle tables lock poisoned"))?;
    guard.tasks.retain_error_context(context.context())?;
    Ok(guard.insert_error_context(table, context.context()))
}

/// Lower the readable end `end` into `table` for a crossing of type
/// `ty`, a `stream<T>` or a `future<T>`: a readable entry of the kind
/// `ty` names enters the receiver's table, and its index is
/// returned. The reference's `lower_stream` and `lower_future` do the
/// same. The index is the receiver's own, and the end record the
/// entry names is the one the sender's entry named, so both ends of
/// the pair go on sharing their record.
pub fn lower_readable_end(
    tables: &mut HandleTables,
    table: TableId,
    end: EndId,
    ty: &ValueType,
) -> Result<u32> {
    let kind = match ty {
        ValueType::Stream(_) => EndKind::StreamReadable,
        ValueType::Future(_) => EndKind::FutureReadable,
        _ => {
            return Err(Error::internal(
                "a readable end was lowered as a type that is neither",
            ));
        }
    };
    Ok(tables.insert_end(table, kind, end))
}

/// Lower `end`, a readable end the host holds, into the guest's
/// handle table for a crossing of type `ty`, a `stream<T>` or a
/// `future<T>`, and return the index it takes there. The guest's end
/// shares its stream or future with the host's writable end, which
/// is how a producer the host created the stream with comes to serve
/// the guest's reads.
///
/// The checks:
///
/// 1. The end must be a readable end in the store, or the crossing
///    fails as an invalid handle with the message Wasmtime's lower
///    raises there, "resource not present": the value was closed, or
///    its end is gone.
/// 2. The host must hold the end, or the crossing fails as an invalid
///    handle. A value that names an end is a plain copy of its
///    identity, so a clone of a [`Val::Stream`] reaches here after
///    another copy lowered, piped, or closed the end, and this is the
///    check that stops it. Wasmtime does not make it: its lower moves
///    such an end into the guest all the same, and the guest's first
///    read of it then fails Wasmtime's own check that the reading
///    side is open. [`TaskTables::held_by_host`] states the rule.
/// 3. Its stream or future must carry the payload `ty` names, or the
///    crossing fails with the payload-mismatch cause.
///
/// [`TaskTables::held_by_host`]: crate::concurrency::TaskTables::held_by_host
pub fn lower_host_end<T: 'static>(
    ctx: &BoundaryContext<'_, T>,
    end: EndId,
    ty: &ValueType,
    position: AbiPosition,
) -> Result<u32> {
    let (kind, declared) = match ty {
        ValueType::Stream(stream) => (EndKind::StreamReadable, stream.payload()),
        ValueType::Future(future) => (EndKind::FutureReadable, future.payload()),
        _ => {
            return Err(Error::internal(
                "a readable end was lowered as a type that is neither",
            ));
        }
    };
    let invalid = |reason: &str| {
        Error::from(AbiError {
            position,
            valtype: Some(ty.clone()),
            cause: AbiCause::InvalidHandle {
                reason: reason.to_owned(),
            },
        })
    };
    let (Some(tables), Some(table)) = (ctx.instance().tables(), ctx.instance().handle_table())
    else {
        return Err(invalid(
            "no handle table of the instance is available to the lower context",
        ));
    };
    let mut guard = tables
        .lock()
        .map_err(|_| Error::internal("resource handle tables lock poisoned"))?;
    if guard.tasks.readable_end(end, kind).is_err() {
        return Err(invalid("resource not present"));
    }
    if !guard.tasks.held_by_host(end) {
        return Err(invalid("the readable end is not one the host holds"));
    }
    let carried = guard
        .tasks
        .shared_record(end)
        .map(|record| record.payload.as_ref())
        .ok_or_else(|| Error::internal("a readable end has no shared record"))?;
    if carried != declared {
        return Err(Error::Copy(CopyCause::PayloadMismatch { kind }));
    }
    lower_readable_end(&mut guard, table, end, ty)
}

/// Lower the elements of a list into memory the guest's
/// `cabi_realloc` hands out for them, and return the pointer it
/// handed out. The caller writes that pointer and the element count
/// wherever the list itself goes: a pair of fields, or a pair of
/// flat slots.
///
/// The list's bytes are assembled on the host and reach the guest in
/// one write, however many elements and fields it has. A numeric
/// element is encoded straight into them; any other element is
/// lowered by [`lower`], whose writes inside the list's bytes land
/// in the host copy. What an element points to — a string, a nested
/// list — is allocated and written on its own as the element is
/// lowered.
pub fn lower_list<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    elements: &[Val],
    element_ty: &ValueType,
    ty: &ValueType,
    position: AbiPosition,
) -> Result<usize> {
    let element_size = size_of(element_ty);
    let total_size = element_size.saturating_mul(elements.len());
    // `cabi_realloc` runs even for an empty list, as the canonical
    // ABI prescribes, so a guest allocator that misbehaves traps.
    let ptr = ctx.allocate_aligned(total_size, alignment_of(element_ty), ty, position)?;
    if let ValueType::Primitive(prim) = element_ty
        && *prim != PrimitiveType::String
    {
        let mut bytes = Vec::with_capacity(total_size);
        for element in elements {
            if !encode_numeric(*prim, element, &mut bytes) {
                return Err(host_value_mismatch(element_ty, position));
            }
        }
        if !bytes.is_empty() {
            ctx.write_bytes(ptr, &bytes, position, ty)?;
        }
        return Ok(ptr);
    }
    ctx.lowering_within(ptr, total_size, position, ty, |ctx| {
        for (i, element) in elements.iter().enumerate() {
            lower(ctx, ptr + i * element_size, element, element_ty, position)?;
        }
        Ok(())
    })?;
    Ok(ptr)
}

/// Allocate the list whose elements of `element_ty` are already
/// encoded as `bytes` and write them there in one access, answering
/// the list's pointer. This is a list of numbers lowered straight from
/// host memory: the one copy is the write into the guest.
pub fn lower_list_bytes<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    bytes: &[u8],
    element_ty: &ValueType,
    ty: &ValueType,
    position: AbiPosition,
) -> Result<usize> {
    // `cabi_realloc` runs even for an empty list, as the canonical
    // ABI prescribes, so a guest allocator that misbehaves traps.
    let ptr = ctx.allocate_aligned(bytes.len(), alignment_of(element_ty), ty, position)?;
    if !bytes.is_empty() {
        ctx.write_bytes(ptr, bytes, position, ty)?;
    }
    Ok(ptr)
}

/// Append the little-endian bytes of `value` as a `prim` to `out`,
/// for every primitive but `string`. Answers `false`, appending
/// nothing, when `value` is not a `prim`.
fn encode_numeric(prim: PrimitiveType, value: &Val, out: &mut Vec<u8>) -> bool {
    match (prim, value) {
        (PrimitiveType::Bool, Val::Bool(v)) => out.push(u8::from(*v)),
        (PrimitiveType::S8, Val::S8(v)) => out.push(*v as u8),
        (PrimitiveType::U8, Val::U8(v)) => out.push(*v),
        (PrimitiveType::S16, Val::S16(v)) => out.extend_from_slice(&v.to_le_bytes()),
        (PrimitiveType::U16, Val::U16(v)) => out.extend_from_slice(&v.to_le_bytes()),
        (PrimitiveType::S32, Val::S32(v)) => out.extend_from_slice(&v.to_le_bytes()),
        (PrimitiveType::U32, Val::U32(v)) => out.extend_from_slice(&v.to_le_bytes()),
        (PrimitiveType::S64, Val::S64(v)) => out.extend_from_slice(&v.to_le_bytes()),
        (PrimitiveType::U64, Val::U64(v)) => out.extend_from_slice(&v.to_le_bytes()),
        (PrimitiveType::F32, Val::F32(v)) => out.extend_from_slice(&v.to_le_bytes()),
        (PrimitiveType::F64, Val::F64(v)) => out.extend_from_slice(&v.to_le_bytes()),
        (PrimitiveType::Char, Val::Char(c)) => out.extend_from_slice(&(*c as u32).to_le_bytes()),
        _ => return false,
    }
    true
}

/// Write the pointer and the length of a string or a list side by
/// side at `offset`, in one write.
pub fn write_pointer_pair<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    offset: usize,
    ptr: usize,
    len: usize,
    position: AbiPosition,
    ty: &ValueType,
) -> Result<()> {
    let mut pair = [0u8; 8];
    pair[..4].copy_from_slice(&(ptr as u32).to_le_bytes());
    pair[4..].copy_from_slice(&(len as u32).to_le_bytes());
    ctx.write_bytes(offset, &pair, position, ty)
}

fn lower_primitive<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    offset: usize,
    value: &Val,
    prim: PrimitiveType,
    position: AbiPosition,
) -> Result<()> {
    let ty = ValueType::Primitive(prim);
    match (prim, value) {
        (PrimitiveType::Bool, Val::Bool(b)) => {
            ctx.write_bytes(offset, &[u8::from(*b)], position, &ty)
        }
        (PrimitiveType::S8, Val::S8(v)) => ctx.write_bytes(offset, &[*v as u8], position, &ty),
        (PrimitiveType::U8, Val::U8(v)) => ctx.write_bytes(offset, &[*v], position, &ty),
        (PrimitiveType::S16, Val::S16(v)) => {
            ctx.write_bytes(offset, &v.to_le_bytes(), position, &ty)
        }
        (PrimitiveType::U16, Val::U16(v)) => {
            ctx.write_bytes(offset, &v.to_le_bytes(), position, &ty)
        }
        (PrimitiveType::S32, Val::S32(v)) => {
            ctx.write_bytes(offset, &v.to_le_bytes(), position, &ty)
        }
        (PrimitiveType::U32, Val::U32(v)) => {
            ctx.write_bytes(offset, &v.to_le_bytes(), position, &ty)
        }
        (PrimitiveType::S64, Val::S64(v)) => {
            ctx.write_bytes(offset, &v.to_le_bytes(), position, &ty)
        }
        (PrimitiveType::U64, Val::U64(v)) => {
            ctx.write_bytes(offset, &v.to_le_bytes(), position, &ty)
        }
        (PrimitiveType::F32, Val::F32(v)) => {
            ctx.write_bytes(offset, &v.to_le_bytes(), position, &ty)
        }
        (PrimitiveType::F64, Val::F64(v)) => {
            ctx.write_bytes(offset, &v.to_le_bytes(), position, &ty)
        }
        (PrimitiveType::Char, Val::Char(c)) => {
            ctx.write_bytes(offset, &(*c as u32).to_le_bytes(), position, &ty)
        }
        (PrimitiveType::String, Val::String(s)) => lower_string(ctx, offset, s, position, &ty),
        _ => Err(host_value_mismatch(&ty, position)),
    }
}

fn lower_string<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    offset: usize,
    s: &str,
    position: AbiPosition,
    ty: &ValueType,
) -> Result<()> {
    let (ptr, units) = lower_str(ctx, s, position, ty)?;
    write_pointer_pair(ctx, offset, ptr, units as usize, position, ty)
}

/// Allocate `s` in the guest under the crossing's encoding and write
/// it there, answering its pointer and the length word that goes with
/// it. A UTF-8 string is written straight from its own bytes, so it
/// reaches the guest in the one copy the write makes.
pub fn lower_str<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    s: &str,
    position: AbiPosition,
    ty: &ValueType,
) -> Result<(usize, u32)> {
    let encoding = ctx.string_encoding();
    let encoded;
    let (bytes, units) = match strings::utf8_view(encoding, s) {
        Some(view) => view,
        None => {
            encoded = strings::encode(encoding, s);
            (encoded.0.as_slice(), encoded.1)
        }
    };
    // `cabi_realloc` runs even for an empty string, as the canonical
    // ABI prescribes, so a guest allocator that misbehaves traps.
    let ptr = ctx.allocate_aligned(bytes.len(), strings::alignment(encoding), ty, position)?;
    ctx.write_bytes(ptr, bytes, position, ty)?;
    Ok((ptr, units))
}

fn write_discriminant<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    offset: usize,
    tag: usize,
    width: usize,
    ty: &ValueType,
    position: AbiPosition,
) -> Result<()> {
    match width {
        1 => ctx.write_bytes(offset, &[tag as u8], position, ty),
        2 => ctx.write_bytes(offset, &(tag as u16).to_le_bytes(), position, ty),
        4 => ctx.write_bytes(offset, &(tag as u32).to_le_bytes(), position, ty),
        _ => Err(Error::from(AbiError {
            position,
            valtype: Some(ty.clone()),
            cause: AbiCause::InvalidEncoding {
                message: format!("unexpected discriminant width: {width}"),
            },
        })),
    }
}

/// Lower a handle into the guest and return the index to write.
///
/// The declared parameter names a resource table by its index in the
/// component; a handle whose identity is not the resource type that
/// table holds (one minted by another instance of the same component,
/// say) is rejected with the unregistered-resource-type cause. For an
/// `own<T>` parameter the handle must name a live owning entry in the
/// host's table, which moves into the instance's table: this is the
/// canonical ABI's transfer of ownership into the guest. For a
/// `borrow<T>` parameter the handle must name a live owning entry in
/// the host's table too: the entry is lent to the crossing's scope
/// for the length of the call, and the guest receives a borrow entry
/// owed to the current task, or the rep itself when the instance
/// defines the resource.
///
/// Both handle types therefore refuse a handle the host has released
/// and one the host never minted, with the invalid-handle cause. A
/// handle is a copyable record of an index and a rep, so a stale
/// copy of one outlives the entry it names; the lookup is what keeps
/// such a copy from putting a freed or arbitrary rep in front of the
/// guest. A `borrow<T>` is checked further: the entry the index
/// names must hold the rep the handle records. That is what refuses
/// a handle whose index belongs to a guest's table rather than the
/// host's — which is what a borrow the host received out of a guest
/// carries — instead of silently lending whichever host entry sits
/// at the same index. The lend is what keeps the entry from going
/// away under a borrow the guest still holds: while it stands, taking
/// the entry back out — for a `Store::resource_drop` from a host
/// function the guest called, or for a second lowering as an
/// `own<T>` — fails.
pub fn lower_handle<T: 'static>(
    ctx: &BoundaryContext<'_, T>,
    handle: &ResourceHandle,
    ty: &ValueType,
    position: AbiPosition,
) -> Result<u32> {
    let tables = ctx.instance().tables().ok_or_else(|| {
        Error::from(AbiError {
            position,
            valtype: Some(ty.clone()),
            cause: AbiCause::InvalidHandle {
                reason: "no handle-tables ledger available to the lower context".to_owned(),
            },
        })
    })?;
    let table = declared_resource_index(ty)
        .and_then(|i| ctx.instance().resource_tables().get(i).copied().flatten())
        .ok_or_else(|| {
            Error::from(AbiError {
                position,
                valtype: Some(ty.clone()),
                cause: AbiCause::InvalidHandle {
                    reason: "the handle's type names no resource table of the instance".to_owned(),
                },
            })
        })?;
    if table.type_id != handle.type_id() {
        return Err(Error::from(AbiError {
            position,
            valtype: Some(ty.clone()),
            cause: AbiCause::UnregisteredResourceType,
        }));
    }
    let mut guard = tables.lock().map_err(|_| Error::Internal {
        message: "resource handle tables lock poisoned".to_owned(),
    })?;
    if matches!(ty, ValueType::Borrow(_)) {
        // The handle names an entry in the host's table for the
        // resource type, and the rep the guest borrows is the one
        // that entry holds. Taking `handle.rep` at its word instead
        // would lower a borrow of a resource the host has released,
        // or of a rep no handle of this store ever named: the handle
        // is a plain copyable record and says nothing about whether
        // the entry behind it is still live.
        let host_table = guard.host_table(handle.type_id());
        let entry = guard
            .lookup(
                host_table,
                handle.index(),
                table.type_id,
                table.guest_defined,
            )
            .map_err(|e| invalid_host_handle(e, ty, position))?;
        let rep = match entry {
            HandleKind::Own { rep, .. } => rep,
            // A borrow entry is not the host's to lend on: it is
            // already owed to a call of its own.
            _ => {
                return Err(invalid_host_handle(
                    HandleLookupError::NotOwned {
                        index: handle.index(),
                    },
                    ty,
                    position,
                ));
            }
        };
        // The entry must be the one the handle records, and an index
        // on its own does not say that: the host's table and every
        // guest's table allocate from a free list that starts low, so
        // one index names a live entry in each of them. A `borrow<T>`
        // the host received out of a guest carries that guest's table
        // index, and lowering it back would otherwise reach whatever
        // host entry of the same type happens to sit at the same
        // index — a different resource, lent and handed to the guest
        // with nothing to say it went wrong. A host entry's rep never
        // changes after it is minted, so the rep the handle carries
        // and the rep the entry holds agree for every handle the host
        // actually holds, and disagree exactly when the index came
        // from somewhere else.
        if rep != handle.rep() {
            return Err(invalid_host_handle(
                HandleLookupError::Unknown {
                    index: handle.index(),
                },
                ty,
                position,
            ));
        }
        // The entry is lent to the crossing's scope for the length
        // of the call, so nothing can take it back out — a
        // re-entrant `Store::resource_drop`, or a second lowering as
        // an `own<T>` — while the guest still holds the borrow. The
        // scope's end gives the lend back, exactly as it does for a
        // borrow a guest lifted out of one of its own owning
        // entries.
        guard
            .lend_to(ctx.scope(), host_table, handle.index())
            .map_err(|e| invalid_host_handle(e, ty, position))?;
        // The defining instance receives its own resource's rep; any
        // other instance receives a borrow entry owed to the current
        // task, which the guest must drop before that task returns.
        if table.defining {
            return Ok(rep);
        }
        return guard
            .insert_borrow_for(
                ctx.scope(),
                table.table,
                table.type_id,
                table.guest_defined,
                rep,
            )
            .ok_or_else(|| {
                Error::from(AbiError {
                    position,
                    valtype: Some(ty.clone()),
                    cause: AbiCause::InvalidHandle {
                        reason: "a borrow can only be lowered during a call".to_owned(),
                    },
                })
            });
    }
    // Ownership moves from the host's table into the instance's: the
    // handle must name a live owning entry the host holds.
    let host_table = guard.host_table(handle.type_id());
    let rep = guard
        .remove_own(
            host_table,
            handle.index(),
            handle.type_id(),
            table.guest_defined,
        )
        .map_err(|e| invalid_host_handle(e, ty, position))?;
    Ok(guard.insert_own(table.table, table.type_id, table.guest_defined, rep))
}

/// The invalid-handle failure a lookup in the host's table raises,
/// under the declared type `ty` at `position`.
///
/// An index that names nothing is reported against the host's table
/// by name: that is the table a handle the host holds addresses, and
/// a bare "unknown handle index" would read as a guest's mistake.
/// Every other reason says enough on its own.
fn invalid_host_handle(error: HandleLookupError, ty: &ValueType, position: AbiPosition) -> Error {
    Error::from(AbiError {
        position,
        valtype: Some(ty.clone()),
        cause: AbiCause::InvalidHandle {
            reason: match error {
                HandleLookupError::Unknown { index } => {
                    format!("handle index {index} is not live in the host's resource table")
                }
                other => other.to_string(),
            },
        },
    })
}

fn host_value_mismatch(ty: &ValueType, position: AbiPosition) -> Error {
    Error::from(AbiError {
        position,
        valtype: Some(ty.clone()),
        cause: AbiCause::HostValueMismatch,
    })
}
