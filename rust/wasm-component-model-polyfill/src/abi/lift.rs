//! Canonical-ABI lift: read a [`Val`] out of guest memory.
//!
//! The entry point [`lift`] takes a [`BoundaryContext`], a memory
//! offset, and the destination [`ValueType`]. It recurses through
//! compound shapes, dispatching to per-variant primitives at the
//! leaves.

use crate::abi::context::BoundaryContext;
use crate::abi::layout::{align_to, alignment_of, discriminant_size, size_of};
use crate::abi::strings;
use crate::concurrency::{CopyState, EndId, EndKind};
use crate::error::{AbiCause, AbiError, AbiPosition, CopyCause, Error, Result};
use crate::internal::ErrorInternal;
use crate::resource::{HandleKind, HandleLookupError, HandleTables, ResourceHandleParts, TableId};
use crate::types::{MapType, PrimitiveType, ValueType};
use crate::value::{Val, ValField};

/// Lift the value of type `ty` out of the guest's linear memory at
/// `offset`. The lift is recursive; compound types read field- or
/// element-wise after computing per-element offsets via
/// [`crate::abi::layout`].
pub fn lift<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    offset: usize,
    ty: &ValueType,
    position: AbiPosition,
) -> Result<Val> {
    match ty {
        ValueType::Primitive(prim) => lift_primitive(ctx, offset, *prim, position),
        ValueType::List(list) => {
            let (ptr, len) = read_pointer_pair(ctx, offset, position, ty)?;
            lift_list(ctx, ptr, len, list.element(), ty, position)
        }
        ValueType::FixedLengthList(fixed) => {
            // Elements sit inline, one element size apart.
            let element_ty = fixed.element();
            let element_size = size_of(element_ty);
            ctx.charge_copy_budget(fixed.length() as usize, LIST_ELEMENT_COST, position, ty)?;
            let mut out = Vec::with_capacity(fixed.length() as usize);
            for i in 0..fixed.length() as usize {
                out.push(lift(ctx, offset + i * element_size, element_ty, position)?);
            }
            Ok(Val::FixedLengthList(out.into_boxed_slice()))
        }
        ValueType::Map(map) => {
            // A map is laid out as the list of its entry tuples.
            let entries_ty = crate::abi::map_entries_type(map);
            let (ptr, len) = read_pointer_pair(ctx, offset, position, &entries_ty)?;
            lift_map(ctx, ptr, len, map, ty, position)
        }
        ValueType::Record(record) => {
            let mut fields: Vec<ValField> = Vec::with_capacity(record.fields().len());
            let mut field_offset = offset;
            for f in record.fields() {
                field_offset = align_to(field_offset, alignment_of(f.ty()));
                let value = lift(ctx, field_offset, f.ty(), position)?;
                field_offset += size_of(f.ty());
                fields.push(ValField {
                    name: f.name().to_owned(),
                    value,
                });
            }
            Ok(Val::Record(fields.into_boxed_slice()))
        }
        ValueType::Tuple(tuple) => {
            let mut elements: Vec<Val> = Vec::with_capacity(tuple.elements().len());
            let mut elem_offset = offset;
            for elem_ty in tuple.elements() {
                elem_offset = align_to(elem_offset, alignment_of(elem_ty));
                elements.push(lift(ctx, elem_offset, elem_ty, position)?);
                elem_offset += size_of(elem_ty);
            }
            Ok(Val::Tuple(elements.into_boxed_slice()))
        }
        ValueType::Variant(variant) => {
            let case_count = variant.cases().len();
            let disc_size = discriminant_size(case_count);
            let discriminant = read_discriminant(ctx, offset, disc_size, position, ty)?;
            let case = variant.cases().get(discriminant).ok_or_else(|| {
                invalid_encoding(
                    ty,
                    position,
                    &format!("discriminant {discriminant} out of range [0..{case_count})"),
                )
            })?;
            let payload_align = variant
                .cases()
                .iter()
                .filter_map(|c| c.payload().map(alignment_of))
                .max()
                .unwrap_or(1);
            let payload_offset = align_to(offset + disc_size, payload_align);
            let payload = if let Some(payload_ty) = case.payload() {
                Some(Box::new(lift(ctx, payload_offset, payload_ty, position)?))
            } else {
                None
            };
            Ok(Val::Variant {
                discriminant: case.name().to_owned(),
                payload,
            })
        }
        ValueType::Option(option) => {
            let [discriminant] = ctx.read_array(offset, position, ty)?;
            let discriminant = discriminant as usize;
            let payload_align = alignment_of(option.payload());
            let payload_offset = align_to(offset + 1, payload_align);
            match discriminant {
                0 => Ok(Val::Option(None)),
                1 => {
                    let inner = lift(ctx, payload_offset, option.payload(), position)?;
                    Ok(Val::Option(Some(Box::new(inner))))
                }
                _ => Err(invalid_encoding(
                    ty,
                    position,
                    "option discriminant must be 0 or 1",
                )),
            }
        }
        ValueType::Result(result) => {
            let [discriminant] = ctx.read_array(offset, position, ty)?;
            let discriminant = discriminant as usize;
            let payload_align = result
                .ok()
                .map(alignment_of)
                .into_iter()
                .chain(result.err().map(alignment_of))
                .max()
                .unwrap_or(1);
            let payload_offset = align_to(offset + 1, payload_align);
            match discriminant {
                0 => {
                    let payload = if let Some(ok_ty) = result.ok() {
                        Some(Box::new(lift(ctx, payload_offset, ok_ty, position)?))
                    } else {
                        None
                    };
                    Ok(Val::Result(Ok(payload)))
                }
                1 => {
                    let payload = if let Some(err_ty) = result.err() {
                        Some(Box::new(lift(ctx, payload_offset, err_ty, position)?))
                    } else {
                        None
                    };
                    Ok(Val::Result(Err(payload)))
                }
                _ => Err(invalid_encoding(
                    ty,
                    position,
                    "result discriminant must be 0 or 1",
                )),
            }
        }
        ValueType::Enum(en) => {
            let disc_size = discriminant_size(en.cases().len());
            let discriminant = read_discriminant(ctx, offset, disc_size, position, ty)?;
            let case = en
                .cases()
                .get(discriminant)
                .ok_or_else(|| invalid_encoding(ty, position, "enum discriminant out of range"))?;
            Ok(Val::Enum(case.clone()))
        }
        ValueType::Flags(flags) => {
            let n = flags.names().len();
            let bytes = ctx.read_bytes(offset, size_of(ty), position, ty)?;
            let mut active: Vec<String> = Vec::new();
            for (i, name) in flags.names().iter().enumerate() {
                if i >= n {
                    break;
                }
                let byte = bytes.get(i / 8).copied().unwrap_or(0);
                if (byte >> (i % 8)) & 1 == 1 {
                    active.push(name.clone());
                }
            }
            Ok(Val::Flags(active.into_boxed_slice()))
        }
        ValueType::Own(_) | ValueType::Borrow(_) => {
            let index = u32::from_le_bytes(ctx.read_array(offset, position, ty)?);
            lift_handle(ctx, index, ty, position, matches!(ty, ValueType::Own(_)))
        }
        ValueType::Stream(_) | ValueType::Future(_) => {
            let index = u32::from_le_bytes(ctx.read_array(offset, position, ty)?);
            lift_end_for_host(ctx, index, ty, position)
        }
    }
}

/// The refusal of a crossing that carries a `stream<T>` or a
/// `future<T>` out of a guest to the host. Such a value names a
/// readable end, and the host has no value to lift one into yet, so
/// the type translates and the crossing fails at the call. The other
/// way, a readable end the host holds lowers into a guest. Between
/// two components the end crosses through the transfer intrinsics
/// of the adapter instead, which never reach this.
pub fn end_transfer_unsupported(ty: &ValueType) -> Error {
    Error::unsupported(match ty {
        ValueType::Future(_) => "the transfer of a `future<T>` readable end to or from the host",
        _ => "the transfer of a `stream<T>` readable end to or from the host",
    })
}

/// Lift the readable end at `index` of the guest's handle table
/// toward the host. The lift makes every check a crossing makes, so
/// the guest learns of an end that cannot cross before it learns
/// that the host cannot take one: an end in a waitable set traps as
/// it would on its way into another component. An end that passes
/// stays where it was, and the crossing fails with
/// [`end_transfer_unsupported`].
pub fn lift_end_for_host<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    index: u32,
    ty: &ValueType,
    position: AbiPosition,
) -> Result<Val> {
    let invalid = |reason: String| {
        Error::from(AbiError {
            position,
            valtype: Some(ty.clone()),
            cause: AbiCause::InvalidHandle { reason },
        })
    };
    let (Some(tables), Some(table)) = (ctx.instance().tables(), ctx.instance().handle_table())
    else {
        return Err(invalid(
            "no handle table of the instance is available to the lift context".to_owned(),
        ));
    };
    let guard = tables
        .lock()
        .map_err(|_| Error::internal("resource handle tables lock poisoned"))?;
    readable_end_at(&guard, table, index, ty, |err| invalid(err.to_string()))?;
    Err(end_transfer_unsupported(ty))
}

/// Lift the readable end at `index` of `table` for a crossing of
/// type `ty`, a `stream<T>` or a `future<T>`: the entry leaves the
/// sender's table and the end it named is returned, to be lowered
/// into the receiver's table. The reference's `lift_async_value`
/// makes the same checks, in the order [`readable_end_at`] states.
/// Only the readable end ever crosses: the writable end stays in the
/// instance that created the pair.
///
/// A lift that fails leaves the entry where it was. That departs from
/// the reference and Wasmtime, which both take the entry out of the
/// table first and check after, so a failed lift there has already
/// removed it. Every such failure is a trap, so the difference shows
/// only after the trap, to a later call into the same instance, which
/// finds the entry still there. The polyfill checks before it removes
/// because its lift of a readable end toward the host makes the same
/// checks and must keep the entry when that crossing is refused.
///
/// An end is an index, as a resource handle is, so its lift charges
/// the crossing's copy budget nothing, as Wasmtime charges its
/// hostcall fuel nothing for one.
///
/// A lookup that fails reaches the caller through `invalid`, which
/// says how the failure reads at that caller. Every other failure is
/// an [`Error`].
pub fn lift_readable_end<E: From<Error>>(
    tables: &mut HandleTables,
    table: TableId,
    index: u32,
    ty: &ValueType,
    invalid: impl FnOnce(HandleLookupError) -> E,
) -> core::result::Result<EndId, E> {
    let end = readable_end_at(tables, table, index, ty, invalid)?;
    tables.remove(table, index);
    Ok(end)
}

/// The readable end at `index` of `table`, when it may cross as a
/// value of type `ty`. The checks run in the order of the
/// reference's `lift_async_value` and of Wasmtime's removal of a
/// readable end from its handle table, and each trap carries
/// Wasmtime's message:
///
/// 1. The entry must be a readable end of the kind `ty` names.
/// 2. The end's stream or future must carry the payload `ty` names.
/// 3. No copy may be in progress on the end, and the end must not be
///    done. The two cannot hold at once, so their order is moot.
/// 4. The end must not be in a waitable set.
///
/// An end that is copying and in a set therefore fails as busy, which
/// is what a guest that started an asynchronous read and then joined
/// the end to a set meets.
pub fn readable_end_at<E: From<Error>>(
    tables: &HandleTables,
    table: TableId,
    index: u32,
    ty: &ValueType,
    invalid: impl FnOnce(HandleLookupError) -> E,
) -> core::result::Result<EndId, E> {
    let (kind, payload) = match ty {
        ValueType::Stream(stream) => (EndKind::StreamReadable, stream.payload()),
        ValueType::Future(future) => (EndKind::FutureReadable, future.payload()),
        _ => {
            return Err(Error::internal("a readable end crossed as a type that is neither").into());
        }
    };
    let end = tables
        .end_from_handle(table, index, kind)
        .map_err(invalid)?;
    let record = tables
        .tasks
        .end(end)
        .ok_or_else(|| Error::internal("an end's entry names an end that is not in the store"))?;
    let shared = tables
        .tasks
        .shared_record(end)
        .ok_or_else(|| Error::internal("an end's entry names an end with no shared record"))?;
    if shared.payload.as_ref() != payload {
        return Err(Error::Copy(CopyCause::PayloadMismatch { kind }).into());
    }
    if record.state.busy() {
        return Err(Error::Copy(CopyCause::LiftDuringCopy { kind }).into());
    }
    if record.state == CopyState::Done {
        return Err(Error::Copy(CopyCause::LiftAfterDone { kind }).into());
    }
    if record.waitable.set.is_some() {
        return Err(Error::Copy(CopyCause::LiftInWaitableSet { kind }).into());
    }
    Ok(end)
}

/// Lift the `len` elements of a list that starts at `ptr`, after
/// gating the range the guest presented. The gate is the same
/// wherever the pointer and the length come from — a pair of fields
/// in memory or a pair of flat slots — and all of it runs before a
/// single element is read and before a single byte of capacity is
/// reserved. A guest that presents a length of `0xFFFF_FFFF` would
/// otherwise have the host reserve the whole of it, which asks the
/// allocator for about 160 GiB natively and overflows the capacity
/// computation where a pointer is 32 bits wide.
///
/// The gate first measures the byte range against the memory. The
/// traps on the range are ordered as the canonical ABI's
/// `load_list_from_range` orders them: the byte length, then the
/// alignment of the pointer, then the bounds. The byte range bounds
/// the element count only while an element spans a byte. An element
/// of zero size — a fixed-length list of length zero — spans none,
/// so its byte range is empty at every length, and the count is also
/// measured against the memory itself: a list holds no more elements
/// than the memory the length was measured against holds bytes.
/// Wasmtime raises that shape as the exhaustion of its per-call
/// fuel; the bounds wording here is borrowed from its trap for a
/// range the memory does not hold.
///
/// A range the memory holds can still cost the host far more than
/// the guest: the host holds one [`Val`] per element, several times
/// the size of a `u8`, so a `list<u8>` as long as a large memory is
/// wide asks for many times that memory. Once the range has passed
/// the gate, the list therefore charges the crossing's copy budget
/// [`LIST_ELEMENT_COST`] bytes per element, before anything is read
/// or reserved, as Wasmtime charges its hostcall fuel after its
/// bounds check.
///
/// Once the gate has passed, the whole byte range is read out of the
/// guest in one access, and the elements are decoded from that copy.
/// A numeric element is copied straight out of it; any other element
/// is lifted by [`lift`], whose reads inside the range the copy
/// serves.
pub fn lift_list<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    ptr: usize,
    len: usize,
    element_ty: &ValueType,
    ty: &ValueType,
    position: AbiPosition,
) -> Result<Val> {
    lift_elements(ctx, ptr, len, element_ty, ty, position, LIST_ELEMENT_COST)
}

/// Lift the `len` entries of the map `ty` that start at `ptr`. The
/// entries are laid out and gated as the list of their key-value
/// tuples, and every failure on that list names it, but the copy
/// budget is charged [`MAP_ENTRY_COST`] bytes per entry, for the key
/// and the value the host holds for it.
pub fn lift_map<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    ptr: usize,
    len: usize,
    map: &MapType,
    ty: &ValueType,
    position: AbiPosition,
) -> Result<Val> {
    let entries = lift_elements(
        ctx,
        ptr,
        len,
        &map.entry(),
        &crate::abi::map_entries_type(map),
        position,
        MAP_ENTRY_COST,
    )?;
    crate::abi::entries_to_map(entries, ty, position)
}

/// What one element of a list or of a fixed-length list charges the
/// crossing's copy budget, in bytes: the size of a [`Val`] on a 64-bit
/// host, as Wasmtime charges the size of its own `Val` per element.
/// It is fixed rather than the size of a `Val` on the target, which
/// is smaller in a browser, so a list is refused at the same length
/// on every target.
pub const LIST_ELEMENT_COST: usize = 32;

/// What one entry of a map charges the crossing's copy budget, in
/// bytes: the size of a key and a value, a pair of [`Val`]s, on a
/// 64-bit host, fixed for the same reason as [`LIST_ELEMENT_COST`].
pub const MAP_ENTRY_COST: usize = 64;

/// Lift the `len` elements of a list that starts at `ptr`, as
/// [`lift_list`] states, charging the copy budget `cost` bytes per
/// element.
fn lift_elements<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    ptr: usize,
    len: usize,
    element_ty: &ValueType,
    ty: &ValueType,
    position: AbiPosition,
    cost: usize,
) -> Result<Val> {
    let element_size = size_of(element_ty);
    let (byte_len, memory_size) = gate_list(ctx, ptr, len, element_ty, ty, position)?;
    ctx.charge_copy_budget(len, cost, position, ty)?;
    if let ValueType::Primitive(prim) = element_ty
        && let Some(decode) = numeric_decoder(*prim)
    {
        let bytes = if byte_len == 0 {
            Vec::new()
        } else {
            ctx.read_bytes(ptr, byte_len, position, ty)?
        };
        // Collecting through `Option` hides the length from the
        // vector, which would then grow by doubling; the count is
        // already gated, so it is reserved up front.
        let mut out = Vec::with_capacity(len);
        for chunk in bytes.chunks_exact(element_size) {
            out.push(decode(chunk).ok_or_else(|| {
                invalid_encoding(
                    element_ty,
                    position,
                    "char value is not a valid Unicode scalar",
                )
            })?);
        }
        return Ok(Val::List(out.into_boxed_slice()));
    }
    // The capacity is reserved once the range has passed the gate,
    // and only when the crossing addresses a store of a bounded
    // size, because that size is what the length was measured
    // against. The gated byte range caps it, so an element of zero
    // size reserves nothing at any length and the vector grows as
    // the elements arrive. A crossing that addresses no bounded
    // store reserves nothing either.
    let capacity = match memory_size {
        Some(_) => len.min(byte_len),
        None => 0,
    };
    ctx.lifting_within(ptr, byte_len, position, ty, |ctx| {
        let mut out = Vec::with_capacity(capacity);
        for i in 0..len {
            out.push(lift(ctx, ptr + i * element_size, element_ty, position)?);
        }
        Ok(Val::List(out.into_boxed_slice()))
    })
}

/// Measure the byte range of the `len` elements of `element_ty` a
/// list at `ptr` presents against the memory, in the order
/// [`lift_list`] states, and answer the range's length in bytes with
/// the size of the memory it was measured against. The element count
/// is not charged here: what a crossing holds per element is the
/// caller's to bound.
pub fn gate_list<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    ptr: usize,
    len: usize,
    element_ty: &ValueType,
    ty: &ValueType,
    position: AbiPosition,
) -> Result<(usize, Option<usize>)> {
    let byte_len = len
        .checked_mul(size_of(element_ty))
        .ok_or_else(|| invalid_encoding(ty, position, "list length overflow"))?;
    if !ptr.is_multiple_of(alignment_of(element_ty)) {
        return Err(invalid_encoding(
            ty,
            position,
            "list pointer is not aligned",
        ));
    }
    let memory_size = ctx.memory_size();
    let within_memory = match memory_size {
        Some(size) => ptr.checked_add(byte_len).is_some_and(|end| end <= size) && len <= size,
        None => ptr.checked_add(byte_len).is_some(),
    };
    if !within_memory {
        return Err(invalid_encoding(
            ty,
            position,
            "list pointer/length out of bounds of memory",
        ));
    }
    Ok((byte_len, memory_size))
}

/// Decode one list element from its bytes, or answer `None` for bytes
/// that are no value of the element type.
type ElementDecoder = fn(&[u8]) -> Option<Val>;

/// How to decode one element of a list of `prim` straight from its
/// little-endian bytes, for every primitive but `string`, which
/// points elsewhere. The decoder answers `None` only for a `char`
/// that is not a Unicode scalar.
fn numeric_decoder(prim: PrimitiveType) -> Option<ElementDecoder> {
    fn array<const N: usize>(bytes: &[u8]) -> [u8; N] {
        let mut out = [0u8; N];
        out.copy_from_slice(bytes);
        out
    }
    Some(match prim {
        PrimitiveType::Bool => |b| Some(Val::Bool(b[0] != 0)),
        PrimitiveType::S8 => |b| Some(Val::S8(b[0] as i8)),
        PrimitiveType::U8 => |b| Some(Val::U8(b[0])),
        PrimitiveType::S16 => |b| Some(Val::S16(i16::from_le_bytes(array(b)))),
        PrimitiveType::U16 => |b| Some(Val::U16(u16::from_le_bytes(array(b)))),
        PrimitiveType::S32 => |b| Some(Val::S32(i32::from_le_bytes(array(b)))),
        PrimitiveType::U32 => |b| Some(Val::U32(u32::from_le_bytes(array(b)))),
        PrimitiveType::S64 => |b| Some(Val::S64(i64::from_le_bytes(array(b)))),
        PrimitiveType::U64 => |b| Some(Val::U64(u64::from_le_bytes(array(b)))),
        PrimitiveType::F32 => |b| Some(Val::F32(f32::from_le_bytes(array(b)))),
        PrimitiveType::F64 => |b| Some(Val::F64(f64::from_le_bytes(array(b)))),
        PrimitiveType::Char => |b| char::from_u32(u32::from_le_bytes(array(b))).map(Val::Char),
        PrimitiveType::String => return None,
    })
}

fn lift_primitive<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    offset: usize,
    prim: PrimitiveType,
    position: AbiPosition,
) -> Result<Val> {
    let ty = ValueType::Primitive(prim);
    match prim {
        PrimitiveType::Bool => {
            let bytes = ctx.read_array::<1>(offset, position, &ty)?;
            Ok(Val::Bool(bytes[0] != 0))
        }
        PrimitiveType::S8 => {
            let bytes = ctx.read_array::<1>(offset, position, &ty)?;
            Ok(Val::S8(bytes[0] as i8))
        }
        PrimitiveType::U8 => {
            let bytes = ctx.read_array::<1>(offset, position, &ty)?;
            Ok(Val::U8(bytes[0]))
        }
        PrimitiveType::S16 => {
            let bytes = ctx.read_array(offset, position, &ty)?;
            Ok(Val::S16(i16::from_le_bytes(bytes)))
        }
        PrimitiveType::U16 => {
            let bytes = ctx.read_array(offset, position, &ty)?;
            Ok(Val::U16(u16::from_le_bytes(bytes)))
        }
        PrimitiveType::S32 => {
            let bytes = ctx.read_array(offset, position, &ty)?;
            Ok(Val::S32(i32::from_le_bytes(bytes)))
        }
        PrimitiveType::U32 => {
            let bytes = ctx.read_array(offset, position, &ty)?;
            Ok(Val::U32(u32::from_le_bytes(bytes)))
        }
        PrimitiveType::S64 => {
            let bytes = ctx.read_array(offset, position, &ty)?;
            Ok(Val::S64(i64::from_le_bytes(bytes)))
        }
        PrimitiveType::U64 => {
            let bytes = ctx.read_array(offset, position, &ty)?;
            Ok(Val::U64(u64::from_le_bytes(bytes)))
        }
        PrimitiveType::F32 => {
            let bytes = ctx.read_array(offset, position, &ty)?;
            Ok(Val::F32(f32::from_le_bytes(bytes)))
        }
        PrimitiveType::F64 => {
            let bytes = ctx.read_array(offset, position, &ty)?;
            Ok(Val::F64(f64::from_le_bytes(bytes)))
        }
        PrimitiveType::Char => {
            let raw = u32::from_le_bytes(ctx.read_array(offset, position, &ty)?);
            char::from_u32(raw).map(Val::Char).ok_or_else(|| {
                invalid_encoding(&ty, position, "char value is not a valid Unicode scalar")
            })
        }
        PrimitiveType::String => {
            let (ptr, len) = read_pointer_pair(ctx, offset, position, &ty)?;
            lift_string(ctx, ptr, len, position, &ty).map(Val::String)
        }
    }
}

/// Lift the string of `units` code units at `ptr` under the
/// crossing's encoding, after gating its pointer and its byte range
/// against the memory. The bytes are read in one access and a UTF-8
/// string keeps the buffer they were read into, so the string costs
/// the host one copy of its bytes.
///
/// Once the range has passed the gate, the string charges the
/// crossing's copy budget its code units times the bytes of a code
/// unit, which is the byte length of its range, as Wasmtime charges
/// its host fuel. The charge comes before the bytes are read.
pub fn lift_string<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    ptr: usize,
    units: usize,
    position: AbiPosition,
    ty: &ValueType,
) -> Result<String> {
    let encoding = ctx.string_encoding();
    let units = u32::try_from(units)
        .map_err(|_| invalid_encoding(ty, position, "string length overflow"))?;
    let alignment = strings::alignment(encoding);
    if !ptr.is_multiple_of(alignment) {
        return Err(invalid_encoding(
            ty,
            position,
            &format!("string pointer not aligned to {alignment}"),
        ));
    }
    let byte_len = strings::byte_length(encoding, units)
        .ok_or_else(|| invalid_encoding(ty, position, "string length overflow"))?;
    if !ctx.in_bounds(ptr, byte_len) {
        return Err(invalid_encoding(
            ty,
            position,
            "string pointer/length out of bounds of memory",
        ));
    }
    ctx.charge_copy_budget(byte_len, 1, position, ty)?;
    let raw = ctx.read_bytes(ptr, byte_len, position, ty)?;
    strings::decode_owned(encoding, units, raw)
        .map_err(|message| invalid_encoding(ty, position, message))
}

/// Read the pointer and the length of a string or a list, which sit
/// side by side at `offset`.
pub fn read_pointer_pair<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    offset: usize,
    position: AbiPosition,
    ty: &ValueType,
) -> Result<(usize, usize)> {
    let pair: [u8; 8] = ctx.read_array(offset, position, ty)?;
    let [p0, p1, p2, p3, l0, l1, l2, l3] = pair;
    Ok((
        u32::from_le_bytes([p0, p1, p2, p3]) as usize,
        u32::from_le_bytes([l0, l1, l2, l3]) as usize,
    ))
}

/// Read a discriminant `width` bytes wide at `offset`.
fn read_discriminant<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    offset: usize,
    width: usize,
    position: AbiPosition,
    ty: &ValueType,
) -> Result<usize> {
    Ok(match width {
        1 => usize::from(u8::from_le_bytes(ctx.read_array(offset, position, ty)?)),
        2 => usize::from(u16::from_le_bytes(ctx.read_array(offset, position, ty)?)),
        _ => u32::from_le_bytes(ctx.read_array(offset, position, ty)?) as usize,
    })
}

/// Lift a `own<T>` or `borrow<T>` handle from a 4-byte index that
/// has already been read out of the flat slot or memory location.
///
/// The lift cross-references the index against the per-store handle
/// tables: the polyfill keeps one table per component instance,
/// shared by every handle kind that instance uses, and one table per
/// resource type for the handles the host owns outright, keyed by
/// [`ResourceTypeId`](crate::resource::ResourceTypeId). For an
/// `own<T>` lift the entry is removed from the instance's table and
/// inserted into the host's table for the resource type — ownership
/// transfers to the host. For `borrow<T>` the entry is left in
/// place and the host receives a handle that aliases the live entry.
///
/// The declared `ValueType` names the resource table by its index in
/// the component, and the lift context maps that to the table the
/// instance keeps and the resource type it holds.
pub fn lift_handle<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    index: u32,
    ty: &ValueType,
    position: AbiPosition,
    is_own: bool,
) -> Result<Val> {
    let scope = ctx.scope();
    let tables = ctx.instance().tables().cloned().ok_or_else(|| {
        Error::from(AbiError {
            position,
            valtype: Some(ty.clone()),
            cause: AbiCause::InvalidHandle {
                reason: "no handle-tables ledger available to the lift context".to_owned(),
            },
        })
    })?;

    let mut guard = tables
        .lock()
        .map_err(|_| Error::internal("resource handle tables lock poisoned"))?;

    // The declared type names the resource table by its index in the
    // component; the instance maps that to the table it keeps.
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
    let invalid = |reason: String| {
        Error::from(AbiError {
            position,
            valtype: Some(ty.clone()),
            cause: AbiCause::InvalidHandle { reason },
        })
    };

    if is_own {
        // Ownership transfers to the host: the guest's entry goes
        // away and the host's table for the type takes the resource.
        // An entry that is lent out as a borrow cannot leave until the
        // borrow returns.
        let rep = guard
            .remove_own(table.table, index, table.type_id, table.guest_defined)
            .map_err(|e| invalid(e.to_string()))?;
        let host_index = guard.host_table(table.type_id);
        let host_index = guard.insert_own(host_index, table.type_id, table.guest_defined, rep);
        Ok(Val::Own(
            ResourceHandleParts {
                type_id: table.type_id,
                index: host_index,
                rep,
            }
            .into(),
        ))
    } else {
        // Every instance addresses its own handles by table index, the
        // instance that defines the resource included: a lift of a
        // borrow always reads the entry the index names. Only the
        // lower side short-circuits to the rep, when the instance the
        // borrow is lowered into is the resource's definer.
        //
        // A borrow lifted out of an owning entry lends that entry to
        // the current scope, which gives it back when the scope ends;
        // a borrow of a borrow needs no bookkeeping of its own.
        let entry = guard
            .lookup(table.table, index, table.type_id, table.guest_defined)
            .map_err(|e| invalid(e.to_string()))?;
        if matches!(entry, HandleKind::Own { .. }) {
            guard
                .lend_to(scope, table.table, index)
                .map_err(|e| invalid(e.to_string()))?;
        }
        Ok(Val::Borrow(
            ResourceHandleParts {
                type_id: table.type_id,
                index,
                rep: entry
                    .rep()
                    .expect("lookup only ever returns a resource entry"),
            }
            .into(),
        ))
    }
}
fn invalid_encoding(ty: &ValueType, position: AbiPosition, message: &str) -> Error {
    Error::from(AbiError {
        position,
        valtype: Some(ty.clone()),
        cause: AbiCause::InvalidEncoding {
            message: message.to_owned(),
        },
    })
}

/// The resource index a handle type declares, when it declares one.
pub fn declared_resource_index(ty: &ValueType) -> Option<usize> {
    match ty {
        ValueType::Own(resource) | ValueType::Borrow(resource) => resource.index(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    use wasm_runtime_layer::{
        AsContextMut, Func as RuntimeFunc, FuncType, Memory, MemoryType, Val as RuntimeVal,
        ValType as CoreType,
    };

    use super::*;
    use crate::abi::context::BoundaryContext;
    use crate::abi::flatten::{lift_from_flat_slots, lower_into_flat_slots};
    use crate::abi::instance::BoundaryInstance;
    use crate::abi::instance_flags::InstanceFlags;
    use crate::abi::options::BoundaryOptions;
    use crate::abi::runtime_state::AbiRuntimeState;
    use crate::concurrency::InstanceId;
    use crate::engine::Engine;
    use crate::executor::ir::{CanonOptions, DataModel, StringEncoding};
    use crate::resource::TableId;
    use crate::store::Store;
    use crate::store::{StoreContextInternalExt, StoreInternalExt};
    use crate::types::{FlagsType, ListType, RecordField, RecordType};

    /// The size of the guest memory every crossing below reads
    /// through, in pages. One page is 65 536 bytes, and that count
    /// is what a list's length is measured against.
    const PAGES: u32 = 1;

    /// The `list` whose element occupies no bytes at all: a `flags`
    /// with no labels is one byte-length-zero element type the
    /// canonical ABI admits, and the whole of a `list` of it is
    /// empty however long the guest says it is.
    fn list_of_zero_size_elements() -> ValueType {
        ValueType::List(ListType::new(ValueType::Flags(FlagsType::new(
            Vec::<String>::new(),
        ))))
    }

    /// Give `store` one page of guest memory and return the options
    /// and instance of a crossing that reads through it. The context
    /// itself is built at the call site, because it borrows the
    /// store for as long as it lives.
    fn one_page(store: &mut Store<()>) -> (BoundaryOptions, BoundaryInstance) {
        let memory = Memory::new(
            store.internal().inner_mut().as_context_mut(),
            MemoryType::new(PAGES, None),
        )
        .expect("one page of guest memory");
        let instance = InstanceId::from_index(0);
        let state = Arc::new(Mutex::new(AbiRuntimeState::with_slabs(
            1,
            0,
            0,
            0,
            Vec::new(),
            vec![instance],
            vec![TableId::fresh()],
        )));
        state.lock().expect("runtime state").memories[0] = Some(memory);
        let declared = Arc::new(CanonOptions {
            instance: 0,
            memory: Some(0),
            realloc: None,
            post_return: None,
            async_: false,
            callback: None,
            string_encoding: StringEncoding::Utf8,
            data_model: DataModel::LinearMemory,
        });
        let tables = store.internal().tables_handle();
        BoundaryInstance::resolve(&declared, &state, &tables).expect("resolve")
    }

    #[cfg(target_pointer_width = "64")]
    #[wcmp_macros::test]
    fn it_charges_the_size_of_a_val_on_a_64_bit_host_per_element_and_entry() {
        // The costs are fixed so that every target charges alike, and
        // they are what the host holds per element and per entry
        // where a pointer is 64 bits wide.
        assert_eq!(LIST_ELEMENT_COST, std::mem::size_of::<Val>());
        assert_eq!(MAP_ENTRY_COST, std::mem::size_of::<(Val, Val)>());
    }

    #[wcmp_macros::test]
    fn it_refuses_a_flat_list_whose_elements_outnumber_the_memory() {
        // The pointer and the length arrive in flat slots, and the
        // element spans no bytes: every byte-range trap passes at
        // any length, because the range is empty at any length. The
        // host would still reserve one `Val` per element, which at
        // `0xFFFF_FFFF` elements is about 137 GiB, so the count is
        // measured against the page the length came with.
        let engine = Engine::new().expect("engine");
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");
        let (options, instance) = one_page(&mut store);
        let mut ctx = BoundaryContext::new(
            store.internal().inner_mut().as_context_mut(),
            options,
            instance,
            None,
        );

        let ty = list_of_zero_size_elements();
        let args = [RuntimeVal::I32(0), RuntimeVal::I32(-1)];
        let mut cursor = 0;
        let outcome =
            lift_from_flat_slots(&mut ctx, &args, &mut cursor, &ty, AbiPosition::Argument(0));

        let Err(Error::Abi(error)) = outcome else {
            panic!("a list of `0xFFFF_FFFF` elements is longer than the page holds bytes");
        };
        assert!(
            matches!(
                &error.cause,
                AbiCause::InvalidEncoding { message }
                    if message == "list pointer/length out of bounds of memory"
            ),
            "expected Wasmtime's out-of-bounds wording, got {:?}",
            error.cause
        );
    }

    #[wcmp_macros::test]
    fn it_lifts_a_flat_list_of_zero_size_elements_the_memory_accounts_for() {
        // The same list at a length the page accounts for lifts, so
        // the refusal above is the count and nothing else about an
        // element that occupies no bytes.
        let engine = Engine::new().expect("engine");
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");
        let (options, instance) = one_page(&mut store);
        let mut ctx = BoundaryContext::new(
            store.internal().inner_mut().as_context_mut(),
            options,
            instance,
            None,
        );

        let ty = list_of_zero_size_elements();
        let args = [RuntimeVal::I32(0), RuntimeVal::I32(3)];
        let mut cursor = 0;
        let lifted =
            lift_from_flat_slots(&mut ctx, &args, &mut cursor, &ty, AbiPosition::Argument(0))
                .expect("three elements of no bytes each sit inside any page");

        let Val::List(elements) = lifted else {
            panic!("a `list` lifts to a list");
        };
        assert_eq!(elements.len(), 3);
        assert!(
            elements
                .iter()
                .all(|element| matches!(element, Val::Flags(names) if names.is_empty())),
            "a `flags` with no labels has no flag set"
        );
    }

    /// Where the bump `cabi_realloc` of [`with_realloc`] hands out its
    /// first block. The lists a test writes for a lift sit beneath it.
    const HEAP: u32 = 32 * 1024;

    /// Give `store` one page of guest memory, a bump `cabi_realloc`
    /// over it, and one component instance in its records, and return
    /// the memory with the options and instance of a crossing that
    /// reads, writes, and allocates through them. A lower asks the
    /// realloc for its memory as a real one would, on a task of its
    /// own, which is what the instance's records are for.
    fn with_realloc(store: &mut Store<()>) -> (Memory, BoundaryOptions, BoundaryInstance) {
        let memory = Memory::new(
            store.internal().inner_mut().as_context_mut(),
            MemoryType::new(PAGES, None),
        )
        .expect("one page of guest memory");
        let next = Arc::new(AtomicU32::new(HEAP));
        let realloc = RuntimeFunc::new(
            store.internal().inner_mut().as_context_mut(),
            FuncType::new([CoreType::I32; 4], [CoreType::I32]),
            move |_store, args, results| {
                let [.., RuntimeVal::I32(align), RuntimeVal::I32(size)] = args else {
                    anyhow::bail!("`cabi_realloc` takes four `i32`s");
                };
                let ptr = next.load(Ordering::Relaxed).next_multiple_of(*align as u32);
                next.store(ptr + *size as u32, Ordering::Relaxed);
                results[0] = RuntimeVal::I32(ptr as i32);
                Ok(())
            },
        );
        let instance = store
            .internal()
            .lock_tables()
            .expect("handle tables")
            .tasks
            .insert_instance();
        let flags = InstanceFlags::new(store.internal().context().internal().runtime_mut());
        let state = Arc::new(Mutex::new(
            AbiRuntimeState::with_slabs(
                1,
                1,
                0,
                0,
                Vec::new(),
                vec![instance],
                vec![TableId::fresh()],
            )
            .with_instance_flags(vec![flags]),
        ));
        {
            let mut state = state.lock().expect("runtime state");
            state.memories[0] = Some(memory.clone());
            state.reallocs[0] = Some(realloc);
        }
        let declared = Arc::new(CanonOptions {
            instance: 0,
            memory: Some(0),
            realloc: Some(0),
            post_return: None,
            async_: false,
            callback: None,
            string_encoding: StringEncoding::Utf8,
            data_model: DataModel::LinearMemory,
        });
        let tables = store.internal().tables_handle();
        let (options, instance) =
            BoundaryInstance::resolve(&declared, &state, &tables).expect("resolve");
        (memory, options, instance)
    }

    /// `record point { x: u32, y: u16 }`, whose second field leaves
    /// two bytes of padding at the end of every element.
    fn point() -> ValueType {
        ValueType::Record(RecordType::new([
            RecordField::new("x", ValueType::Primitive(PrimitiveType::U32)),
            RecordField::new("y", ValueType::Primitive(PrimitiveType::U16)),
        ]))
    }

    /// The point at `index` of the lists below.
    fn point_at(index: usize) -> Val {
        Val::Record(Box::new([
            ValField {
                name: "x".to_owned(),
                value: Val::U32(index as u32 * 7),
            },
            ValField {
                name: "y".to_owned(),
                value: Val::U16(index as u16),
            },
        ]))
    }

    /// The bytes the guest holds for the point at `index`.
    fn point_bytes(index: usize) -> [u8; 8] {
        let mut out = [0u8; 8];
        out[..4].copy_from_slice(&(index as u32 * 7).to_le_bytes());
        out[4..6].copy_from_slice(&(index as u16).to_le_bytes());
        out
    }

    /// Lift the list of `ty` whose pointer and length are `ptr` and
    /// `len`, through flat slots.
    fn lift_flat<T: 'static>(
        ctx: &mut BoundaryContext<'_, T>,
        ty: &ValueType,
        ptr: u32,
        len: u32,
    ) -> Val {
        let args = [RuntimeVal::I32(ptr as i32), RuntimeVal::I32(len as i32)];
        let mut cursor = 0;
        lift_from_flat_slots(ctx, &args, &mut cursor, ty, AbiPosition::Argument(0))
            .expect("the list lifts")
    }

    /// Lower `value` of `ty` into flat slots and return its pointer
    /// and length.
    fn lower_flat<T: 'static>(
        ctx: &mut BoundaryContext<'_, T>,
        ty: &ValueType,
        value: &Val,
    ) -> (u32, u32) {
        let mut out = Vec::new();
        lower_into_flat_slots(ctx, value, ty, &mut out, AbiPosition::Argument(0))
            .expect("the list lowers");
        let [RuntimeVal::I32(ptr), RuntimeVal::I32(len)] = out[..] else {
            panic!("a list lowers to a pointer and a length, got {out:?}");
        };
        (ptr as u32, len as u32)
    }

    #[wcmp_macros::test]
    fn it_lifts_a_list_of_u32_in_one_read_of_the_guest() {
        const LEN: usize = 1000;
        let engine = Engine::new().expect("engine");
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");
        let (memory, options, instance) = with_realloc(&mut store);
        let bytes: Vec<u8> = (0..LEN as u32)
            .flat_map(|i| (i * 3).to_le_bytes())
            .collect();
        memory
            .write(store.internal().inner_mut().as_context_mut(), 0, &bytes)
            .expect("write the list");
        let mut ctx = BoundaryContext::new(
            store.internal().inner_mut().as_context_mut(),
            options,
            instance,
            None,
        );

        let ty = ValueType::List(ListType::new(ValueType::Primitive(PrimitiveType::U32)));
        let lifted = lift_flat(&mut ctx, &ty, 0, LEN as u32);

        let expected: Box<[Val]> = (0..LEN as u32).map(|i| Val::U32(i * 3)).collect();
        assert_eq!(lifted, Val::List(expected));
        assert_eq!(
            ctx.substrate_accesses(),
            1,
            "a thousand elements cost one read of the guest"
        );
    }

    #[wcmp_macros::test]
    fn it_lifts_a_list_of_records_in_one_read_of_the_guest() {
        const LEN: usize = 500;
        let engine = Engine::new().expect("engine");
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");
        let (memory, options, instance) = with_realloc(&mut store);
        let bytes: Vec<u8> = (0..LEN).flat_map(point_bytes).collect();
        memory
            .write(store.internal().inner_mut().as_context_mut(), 0, &bytes)
            .expect("write the list");
        let mut ctx = BoundaryContext::new(
            store.internal().inner_mut().as_context_mut(),
            options,
            instance,
            None,
        );

        let ty = ValueType::List(ListType::new(point()));
        let lifted = lift_flat(&mut ctx, &ty, 0, LEN as u32);

        let expected: Box<[Val]> = (0..LEN).map(point_at).collect();
        assert_eq!(lifted, Val::List(expected));
        assert_eq!(
            ctx.substrate_accesses(),
            1,
            "five hundred two-field records cost one read of the guest"
        );
    }

    #[wcmp_macros::test]
    fn it_lowers_a_list_of_u32_in_one_write_to_the_guest() {
        const LEN: usize = 1000;
        let engine = Engine::new().expect("engine");
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");
        let (memory, options, instance) = with_realloc(&mut store);
        let mut ctx = BoundaryContext::new(
            store.internal().inner_mut().as_context_mut(),
            options,
            instance,
            None,
        );

        let ty = ValueType::List(ListType::new(ValueType::Primitive(PrimitiveType::U32)));
        let value = Val::List((0..LEN as u32).map(|i| Val::U32(i * 5)).collect());
        let (ptr, len) = lower_flat(&mut ctx, &ty, &value);
        assert_eq!(
            ctx.substrate_accesses(),
            1,
            "a thousand elements cost one write to the guest"
        );
        drop(ctx);

        assert_eq!((ptr, len), (HEAP, LEN as u32));
        let mut written = vec![0u8; LEN * 4];
        memory
            .read(
                store.internal().inner_mut().as_context_mut(),
                ptr as usize,
                &mut written,
            )
            .expect("read the list back");
        let expected: Vec<u8> = (0..LEN as u32)
            .flat_map(|i| (i * 5).to_le_bytes())
            .collect();
        assert_eq!(written, expected);
    }

    #[wcmp_macros::test]
    fn it_lowers_a_list_of_records_in_one_write_to_the_guest() {
        const LEN: usize = 500;
        let engine = Engine::new().expect("engine");
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");
        let (memory, options, instance) = with_realloc(&mut store);
        // The arena starts dirty, so a padding byte the lower leaves
        // unwritten would show here as 0xa5 rather than as zero.
        memory
            .write(
                store.internal().inner_mut().as_context_mut(),
                HEAP as usize,
                &[0xa5; LEN * 8],
            )
            .expect("dirty the realloc arena");
        let mut ctx = BoundaryContext::new(
            store.internal().inner_mut().as_context_mut(),
            options,
            instance,
            None,
        );

        let ty = ValueType::List(ListType::new(point()));
        let value = Val::List((0..LEN).map(point_at).collect());
        let (ptr, len) = lower_flat(&mut ctx, &ty, &value);
        assert_eq!(
            ctx.substrate_accesses(),
            1,
            "five hundred two-field records cost one write to the guest"
        );
        assert_eq!(
            lift_flat(&mut ctx, &ty, ptr, len),
            value,
            "the list lifts back as it was lowered"
        );
        drop(ctx);

        let mut written = vec![0u8; LEN * 8];
        memory
            .read(
                store.internal().inner_mut().as_context_mut(),
                ptr as usize,
                &mut written,
            )
            .expect("read the list back");
        let expected: Vec<u8> = (0..LEN).flat_map(point_bytes).collect();
        assert_eq!(written, expected, "padding reaches the guest as zeros");
    }

    #[wcmp_macros::test]
    fn it_reaches_the_guest_once_more_for_each_string_a_list_points_to() {
        // The list's own bytes cross once either way; what each
        // element points to lies outside them and crosses on its own.
        let engine = Engine::new().expect("engine");
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");
        let (_memory, options, instance) = with_realloc(&mut store);
        let mut ctx = BoundaryContext::new(
            store.internal().inner_mut().as_context_mut(),
            options,
            instance,
            None,
        );

        let ty = ValueType::List(ListType::new(ValueType::Primitive(PrimitiveType::String)));
        let value = Val::List(
            ["one", "two", "three"]
                .into_iter()
                .map(|s| Val::String(s.to_owned()))
                .collect(),
        );
        let (ptr, len) = lower_flat(&mut ctx, &ty, &value);
        assert_eq!(
            ctx.substrate_accesses(),
            1 + 3,
            "the list, then each string"
        );
        assert_eq!(lift_flat(&mut ctx, &ty, ptr, len), value);
        assert_eq!(
            ctx.substrate_accesses(),
            4 + 1 + 3,
            "the same on the way back"
        );
    }

    /// Write `bytes` at the start of a fresh guest memory and lift
    /// them as a list of `len` elements of `prim`.
    fn lift_written(prim: PrimitiveType, bytes: &[u8], len: u32) -> Result<Val> {
        let engine = Engine::new().expect("engine");
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");
        let (memory, options, instance) = with_realloc(&mut store);
        memory
            .write(store.internal().inner_mut().as_context_mut(), 0, bytes)
            .expect("write the list");
        let mut ctx = BoundaryContext::new(
            store.internal().inner_mut().as_context_mut(),
            options,
            instance,
            None,
        );
        let ty = ValueType::List(ListType::new(ValueType::Primitive(prim)));
        let args = [RuntimeVal::I32(0), RuntimeVal::I32(len as i32)];
        let mut cursor = 0;
        lift_from_flat_slots(&mut ctx, &args, &mut cursor, &ty, AbiPosition::Argument(0))
    }

    /// Whether `result` failed as bytes that are no value of the type.
    fn is_invalid_encoding(result: &Result<Val>) -> bool {
        matches!(
            result,
            Err(Error::Abi(abi)) if matches!(abi.cause, AbiCause::InvalidEncoding { .. })
        )
    }

    /// Lower `value` as a list of `prim`, then lift it back, and return
    /// the lifted list with the bytes the lower wrote.
    fn round_trip(prim: PrimitiveType, value: &Val, element_size: usize) -> (Val, Vec<u8>) {
        let engine = Engine::new().expect("engine");
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");
        let (memory, options, instance) = with_realloc(&mut store);
        let mut ctx = BoundaryContext::new(
            store.internal().inner_mut().as_context_mut(),
            options,
            instance,
            None,
        );
        let ty = ValueType::List(ListType::new(ValueType::Primitive(prim)));
        let (ptr, len) = lower_flat(&mut ctx, &ty, value);
        let lifted = lift_flat(&mut ctx, &ty, ptr, len);
        drop(ctx);
        let mut written = vec![0u8; len as usize * element_size];
        memory
            .read(
                store.internal().inner_mut().as_context_mut(),
                ptr as usize,
                &mut written,
            )
            .expect("read the list back");
        (lifted, written)
    }

    #[wcmp_macros::test]
    fn it_refuses_a_list_of_char_holding_a_surrogate() {
        let bytes: Vec<u8> = [0x41u32, 0xd800]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect();
        let lifted = lift_written(PrimitiveType::Char, &bytes, 2);
        assert!(
            is_invalid_encoding(&lifted),
            "a surrogate is no Unicode scalar, got {lifted:?}"
        );
    }

    #[wcmp_macros::test]
    fn it_refuses_a_list_of_char_holding_a_value_past_the_last_scalar() {
        let bytes: Vec<u8> = [0x41u32, 0x11_0000]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect();
        let lifted = lift_written(PrimitiveType::Char, &bytes, 2);
        assert!(
            is_invalid_encoding(&lifted),
            "0x110000 is past the last Unicode scalar, got {lifted:?}"
        );
    }

    #[wcmp_macros::test]
    fn it_lifts_any_nonzero_byte_of_a_list_of_bool_as_true() {
        let lifted =
            lift_written(PrimitiveType::Bool, &[0, 1, 2, 0xff], 4).expect("the list lifts");
        assert_eq!(
            lifted,
            Val::List(Box::new([
                Val::Bool(false),
                Val::Bool(true),
                Val::Bool(true),
                Val::Bool(true),
            ]))
        );
    }

    #[wcmp_macros::test]
    fn it_round_trips_a_list_of_s16_through_its_little_endian_bytes() {
        let numbers = [i16::MIN, -2, -1, 0, 1, i16::MAX];
        let value = Val::List(numbers.iter().copied().map(Val::S16).collect());
        let (lifted, written) = round_trip(PrimitiveType::S16, &value, 2);
        assert_eq!(lifted, value);
        let expected: Vec<u8> = numbers.iter().flat_map(|n| n.to_le_bytes()).collect();
        assert_eq!(written, expected);
    }

    #[wcmp_macros::test]
    fn it_round_trips_a_list_of_f64_bit_for_bit() {
        let numbers = [
            -0.0,
            1.5,
            f64::NEG_INFINITY,
            f64::MIN_POSITIVE / 2.0,
            f64::from_bits(0x7ff8_0000_dead_beef),
        ];
        let value = Val::List(numbers.iter().copied().map(Val::F64).collect());
        let (lifted, written) = round_trip(PrimitiveType::F64, &value, 8);
        let expected: Vec<u8> = numbers.iter().flat_map(|n| n.to_le_bytes()).collect();
        assert_eq!(written, expected);
        let Val::List(elements) = lifted else {
            panic!("a `list` lifts to a list");
        };
        let bits: Vec<u64> = elements
            .iter()
            .map(|element| match element {
                Val::F64(n) => n.to_bits(),
                other => panic!("expected an f64, got {other:?}"),
            })
            .collect();
        let expected: Vec<u64> = numbers.iter().map(|n| n.to_bits()).collect();
        assert_eq!(bits, expected, "sign, subnormal, and NaN payload survive");
    }
}
