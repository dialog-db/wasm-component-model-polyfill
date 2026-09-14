//! Per-slot canonical-ABI flattening for parameters at flat-arg
//! position (and, by symmetry, returns at flat-result position
//! when the type's flat count fits in one slot).
//!
//! When a function call's total flat parameter count is at most
//! `MAX_FLAT_PARAMS` (= 16), the canonical ABI passes each
//! parameter as a sequence of *flat slots* — one core-Wasm value
//! per slot — rather than packing every parameter into a single
//! memory tuple and passing a pointer. The flat-slot encoding is
//! recursive: a record of two i32 fields uses two flat slots; a
//! variant uses one i32 for the discriminant followed by the
//! "joined" payload slots common to every case.
//!
//! [`lower_into_flat_slots`] writes a host-supplied [`Val`] into
//! a `Vec<RuntimeVal>`; [`lift_from_flat_slots`] reads slots from a
//! caller-supplied cursor and constructs a host-side [`Val`]. Both
//! recurse through compound types and dispatch the heap-allocating
//! valtypes (string, list) through the existing memory-resident
//! lift / lower paths.

use wasm_runtime_layer::Val as RuntimeVal;

use super::context::{LiftContext, LowerContext};
use super::layout::{FlatType, flags_chunk_count, flat_types, join_flat_slots, size_of};
use super::{lift, lower};
use crate::error::{AbiCause, AbiError, AbiPosition, Error, Result};
use crate::executor::ir::StringEncoding;
use crate::resource::ResourceHandle;
use crate::types::{PrimitiveType, ValueType};
use crate::value::{Val, ValField};

/// Lower a value into the slot-by-slot flat encoding the canonical
/// ABI specifies for function parameters.
pub fn lower_into_flat_slots<T: 'static>(
    ctx: &mut LowerContext<'_, T>,
    value: &Val,
    ty: &ValueType,
    out: &mut Vec<RuntimeVal>,
    position: AbiPosition,
) -> Result<()> {
    match (ty, value) {
        (ValueType::Primitive(PrimitiveType::String), Val::String(s)) => {
            let (ptr, units) = lower_string(ctx, s, position, ty)?;
            out.push(RuntimeVal::I32(ptr as i32));
            out.push(RuntimeVal::I32(units as i32));
            Ok(())
        }
        (ValueType::Primitive(prim), _) => {
            out.push(primitive_to_flat(*prim, value, position, ty)?);
            Ok(())
        }
        (ValueType::List(list), Val::List(elements)) => {
            let element_ty = list.element().clone();
            let element_size = size_of(&element_ty);
            let total_size = element_size.saturating_mul(elements.len());
            let ptr = if total_size == 0 {
                0
            } else {
                ctx.allocate(total_size, ty, position)?
            };
            for (i, elem) in elements.iter().enumerate() {
                lower(ctx, ptr + i * element_size, elem, &element_ty, position)?;
            }
            out.push(RuntimeVal::I32(ptr as i32));
            out.push(RuntimeVal::I32(elements.len() as i32));
            Ok(())
        }
        (ValueType::Record(record), Val::Record(fields)) => {
            if fields.len() != record.fields().len() {
                return Err(host_value_mismatch(ty, position));
            }
            for (record_field, val_field) in record.fields().iter().zip(fields.iter()) {
                if record_field.name() != val_field.name {
                    return Err(host_value_mismatch(ty, position));
                }
                lower_into_flat_slots(ctx, &val_field.value, record_field.ty(), out, position)?;
            }
            Ok(())
        }
        (ValueType::Tuple(tuple), Val::Tuple(elements)) => {
            if elements.len() != tuple.elements().len() {
                return Err(host_value_mismatch(ty, position));
            }
            for (elem_ty, elem_val) in tuple.elements().iter().zip(elements.iter()) {
                lower_into_flat_slots(ctx, elem_val, elem_ty, out, position)?;
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
            let payload_value = payload.as_deref();
            let payload_ty = case.payload();
            lower_variant_flat(
                ctx,
                tag,
                payload_ty,
                payload_value,
                variant.cases().iter().map(|c| c.payload().cloned()),
                out,
                ty,
                position,
            )
        }
        (ValueType::Option(option), Val::Option(payload)) => {
            let (tag, payload_value) = match payload.as_deref() {
                None => (0usize, None),
                Some(v) => (1usize, Some(v)),
            };
            let active_payload_ty = if tag == 1 {
                Some(option.payload())
            } else {
                None
            };
            lower_variant_flat(
                ctx,
                tag,
                active_payload_ty,
                payload_value,
                [None, Some(option.payload().clone())].into_iter(),
                out,
                ty,
                position,
            )
        }
        (ValueType::Result(result), Val::Result(arm)) => {
            let (tag, payload_value, active_payload_ty) = match arm {
                Ok(payload) => (0usize, payload.as_deref(), result.ok()),
                Err(payload) => (1usize, payload.as_deref(), result.err()),
            };
            lower_variant_flat(
                ctx,
                tag,
                active_payload_ty,
                payload_value,
                [result.ok().cloned(), result.err().cloned()].into_iter(),
                out,
                ty,
                position,
            )
        }
        (ValueType::Enum(en), Val::Enum(name)) => {
            let tag = en
                .cases()
                .iter()
                .position(|c| c == name)
                .ok_or_else(|| host_value_mismatch(ty, position))?;
            out.push(RuntimeVal::I32(tag as i32));
            Ok(())
        }
        (ValueType::Flags(flags_ty), Val::Flags(active)) => {
            let mut bits = vec![0u32; flags_chunk_count(flags_ty)];
            for name in active.iter() {
                let idx = flags_ty
                    .names()
                    .iter()
                    .position(|f| f == name)
                    .ok_or_else(|| host_value_mismatch(ty, position))?;
                bits[idx / 32] |= 1u32 << (idx % 32);
            }
            for chunk in bits {
                out.push(RuntimeVal::I32(chunk as i32));
            }
            Ok(())
        }
        (ValueType::Own(_), Val::Own(handle))
        | (ValueType::Borrow(_), Val::Borrow(handle))
        | (ValueType::Borrow(_), Val::Own(handle)) => {
            validate_handle_in_table(ctx, handle, ty, position)?;
            out.push(RuntimeVal::I32(handle.index as i32));
            Ok(())
        }
        _ => Err(host_value_mismatch(ty, position)),
    }
}

/// Lift a value out of the slot-by-slot flat encoding starting at
/// `cursor`. The cursor advances by the value's flat-slot count.
pub fn lift_from_flat_slots<T: 'static>(
    ctx: &mut LiftContext<'_, T>,
    args: &[RuntimeVal],
    cursor: &mut usize,
    ty: &ValueType,
    position: AbiPosition,
) -> Result<Val> {
    match ty {
        ValueType::Primitive(PrimitiveType::String) => {
            let ptr = take_i32(args, cursor, ty, position)? as usize;
            let len = take_i32(args, cursor, ty, position)? as usize;
            lift_string_from_memory(ctx, ptr, len, ty, position)
        }
        ValueType::Primitive(prim) => primitive_from_flat(*prim, args, cursor, ty, position),
        ValueType::List(list) => {
            let ptr = take_i32(args, cursor, ty, position)? as usize;
            let len = take_i32(args, cursor, ty, position)? as usize;
            let element_ty = list.element().clone();
            let element_size = size_of(&element_ty);
            let mut out: Vec<Val> = Vec::with_capacity(len);
            for i in 0..len {
                out.push(lift(ctx, ptr + i * element_size, &element_ty, position)?);
            }
            Ok(Val::List(out.into_boxed_slice()))
        }
        ValueType::Record(record) => {
            let mut fields: Vec<ValField> = Vec::with_capacity(record.fields().len());
            for record_field in record.fields() {
                let value = lift_from_flat_slots(ctx, args, cursor, record_field.ty(), position)?;
                fields.push(ValField {
                    name: record_field.name().to_owned(),
                    value,
                });
            }
            Ok(Val::Record(fields.into_boxed_slice()))
        }
        ValueType::Tuple(tuple) => {
            let mut elements: Vec<Val> = Vec::with_capacity(tuple.elements().len());
            for elem_ty in tuple.elements() {
                elements.push(lift_from_flat_slots(ctx, args, cursor, elem_ty, position)?);
            }
            Ok(Val::Tuple(elements.into_boxed_slice()))
        }
        ValueType::Variant(variant) => {
            let tag = take_i32(args, cursor, ty, position)? as usize;
            let case = variant.cases().get(tag).ok_or_else(|| {
                invalid_encoding(ty, position, "variant discriminant out of range")
            })?;
            let payload_ty = case.payload().cloned();
            let case_name = case.name().to_owned();
            let case_payloads: Vec<Option<ValueType>> = variant
                .cases()
                .iter()
                .map(|c| c.payload().cloned())
                .collect();
            let payload = lift_variant_payload_flat(
                ctx,
                args,
                cursor,
                payload_ty.as_ref(),
                &case_payloads,
                ty,
                position,
            )?;
            Ok(Val::Variant {
                discriminant: case_name,
                payload: payload.map(Box::new),
            })
        }
        ValueType::Option(option) => {
            let tag = take_i32(args, cursor, ty, position)? as usize;
            let case_payloads: Vec<Option<ValueType>> = vec![None, Some(option.payload().clone())];
            match tag {
                0 => {
                    // Skip the joined payload slots without reading
                    // them as values.
                    let _ = lift_variant_payload_flat(
                        ctx,
                        args,
                        cursor,
                        None,
                        &case_payloads,
                        ty,
                        position,
                    )?;
                    Ok(Val::Option(None))
                }
                1 => {
                    let inner = lift_variant_payload_flat(
                        ctx,
                        args,
                        cursor,
                        Some(option.payload()),
                        &case_payloads,
                        ty,
                        position,
                    )?;
                    Ok(Val::Option(inner.map(Box::new)))
                }
                _ => Err(invalid_encoding(
                    ty,
                    position,
                    "option discriminant must be 0 or 1",
                )),
            }
        }
        ValueType::Result(result) => {
            let tag = take_i32(args, cursor, ty, position)? as usize;
            let case_payloads: Vec<Option<ValueType>> =
                vec![result.ok().cloned(), result.err().cloned()];
            match tag {
                0 => {
                    let payload = lift_variant_payload_flat(
                        ctx,
                        args,
                        cursor,
                        result.ok(),
                        &case_payloads,
                        ty,
                        position,
                    )?;
                    Ok(Val::Result(Ok(payload.map(Box::new))))
                }
                1 => {
                    let payload = lift_variant_payload_flat(
                        ctx,
                        args,
                        cursor,
                        result.err(),
                        &case_payloads,
                        ty,
                        position,
                    )?;
                    Ok(Val::Result(Err(payload.map(Box::new))))
                }
                _ => Err(invalid_encoding(
                    ty,
                    position,
                    "result discriminant must be 0 or 1",
                )),
            }
        }
        ValueType::Enum(en) => {
            let tag = take_i32(args, cursor, ty, position)? as usize;
            let case = en
                .cases()
                .get(tag)
                .ok_or_else(|| invalid_encoding(ty, position, "enum discriminant out of range"))?;
            Ok(Val::Enum(case.clone()))
        }
        ValueType::Flags(flags_ty) => {
            let chunks = flags_chunk_count(flags_ty);
            let mut bits: Vec<u32> = Vec::with_capacity(chunks);
            for _ in 0..chunks {
                bits.push(take_i32(args, cursor, ty, position)? as u32);
            }
            let mut active: Vec<String> = Vec::new();
            for (i, name) in flags_ty.names().iter().enumerate() {
                let chunk = bits.get(i / 32).copied().unwrap_or(0);
                if (chunk >> (i % 32)) & 1 == 1 {
                    active.push(name.clone());
                }
            }
            Ok(Val::Flags(active.into_boxed_slice()))
        }
        ValueType::Own(_) | ValueType::Borrow(_) => {
            let index = take_i32(args, cursor, ty, position)? as u32;
            crate::abi::lift_handle(ctx, index, ty, position, matches!(ty, ValueType::Own(_)))
        }
    }
}

/// Lower a single variant arm into the flat-slot encoding. The
/// joined-flat-slot list is computed from `case_payloads`; the
/// active case's payload is filled in at the matching slot
/// positions, and remaining slots are zero-filled (or
/// reinterpreted) per the canonical ABI's join rules.
#[allow(clippy::too_many_arguments)]
fn lower_variant_flat<T: 'static, I: Iterator<Item = Option<ValueType>>>(
    ctx: &mut LowerContext<'_, T>,
    tag: usize,
    payload_ty: Option<&ValueType>,
    payload_value: Option<&Val>,
    case_payloads: I,
    out: &mut Vec<RuntimeVal>,
    ty: &ValueType,
    position: AbiPosition,
) -> Result<()> {
    let joined = join_flat_slots(case_payloads);
    out.push(RuntimeVal::I32(tag as i32));

    // Flatten the active payload (if any) into a parallel vector.
    let mut active_slots: Vec<RuntimeVal> = Vec::new();
    if let (Some(payload_ty), Some(payload_value)) = (payload_ty, payload_value) {
        lower_into_flat_slots(ctx, payload_value, payload_ty, &mut active_slots, position)?;
    }
    // Cross-reference the active slots with the joined flat shape;
    // pad with zeros / reinterpret where the active case's flat
    // type does not match the joined slot.
    let active_types: Vec<FlatType> = match payload_ty {
        Some(t) => flat_types(t),
        None => Vec::new(),
    };
    if active_slots.len() != active_types.len() {
        return Err(Error::internal(
            "variant payload flat-slot count disagreed with declared flat shape",
        ));
    }
    for (i, joined_ty) in joined.iter().copied().enumerate() {
        match active_slots.get(i) {
            Some(slot) => {
                let active_ty = active_types[i];
                out.push(reinterpret_flat(slot, active_ty, joined_ty, ty, position)?);
            }
            None => out.push(zero_of_flat(joined_ty)),
        }
    }
    Ok(())
}

/// Lift a variant arm's payload from the flat-slot encoding. The
/// caller has already consumed the discriminant; this routine
/// consumes the joined payload slots and decodes them against the
/// active case's flat shape.
fn lift_variant_payload_flat<T: 'static>(
    ctx: &mut LiftContext<'_, T>,
    args: &[RuntimeVal],
    cursor: &mut usize,
    payload_ty: Option<&ValueType>,
    case_payloads: &[Option<ValueType>],
    ty: &ValueType,
    position: AbiPosition,
) -> Result<Option<Val>> {
    let joined = join_flat_slots(case_payloads.iter().cloned());
    let active_types: Vec<FlatType> = match payload_ty {
        Some(t) => flat_types(t),
        None => Vec::new(),
    };
    // Capture the joined slots first.
    let mut joined_slots: Vec<RuntimeVal> = Vec::with_capacity(joined.len());
    for joined_ty in joined.iter().copied() {
        let raw = take_runtime_val(args, cursor, ty, position)?;
        if !matches_flat(&raw, joined_ty) {
            return Err(invalid_encoding(
                ty,
                position,
                "core flat slot does not match joined variant flat shape",
            ));
        }
        joined_slots.push(raw);
    }
    let Some(payload_ty) = payload_ty else {
        return Ok(None);
    };
    // Reinterpret the joined slots back into the active case's
    // flat shape and lift the value through a fresh sub-cursor.
    let mut decoded: Vec<RuntimeVal> = Vec::with_capacity(active_types.len());
    for (i, active_ty) in active_types.iter().copied().enumerate() {
        let slot = joined_slots.get(i).ok_or_else(|| {
            invalid_encoding(
                ty,
                position,
                "active case has more flat slots than the joined payload",
            )
        })?;
        let joined_ty = joined.get(i).copied().unwrap_or(FlatType::I32);
        decoded.push(reinterpret_flat(slot, joined_ty, active_ty, ty, position)?);
    }
    let mut sub_cursor = 0;
    let inner = lift_from_flat_slots(ctx, &decoded, &mut sub_cursor, payload_ty, position)?;
    Ok(Some(inner))
}

fn zero_of_flat(t: FlatType) -> RuntimeVal {
    match t {
        FlatType::I32 => RuntimeVal::I32(0),
        FlatType::I64 => RuntimeVal::I64(0),
        FlatType::F32 => RuntimeVal::F32(0.0),
        FlatType::F64 => RuntimeVal::F64(0.0),
    }
}

/// Convert a slot of one flat type to another per the canonical
/// ABI's join rules. Identical types pass through; widening is
/// zero-extension; bit-cast pairs (i32 ↔ f32, i64 ↔ f64) reinterpret
/// the bits.
fn reinterpret_flat(
    slot: &RuntimeVal,
    from: FlatType,
    to: FlatType,
    ty: &ValueType,
    position: AbiPosition,
) -> Result<RuntimeVal> {
    if from == to {
        return Ok(slot.clone());
    }
    let mismatch = || invalid_encoding(ty, position, "flat slot reinterpretation not supported");
    Ok(match (slot, from, to) {
        (RuntimeVal::I32(v), FlatType::I32, FlatType::F32) => {
            RuntimeVal::F32(f32::from_bits(*v as u32))
        }
        (RuntimeVal::F32(v), FlatType::F32, FlatType::I32) => RuntimeVal::I32(v.to_bits() as i32),
        (RuntimeVal::I32(v), FlatType::I32, FlatType::I64) => RuntimeVal::I64(i64::from(*v as u32)),
        (RuntimeVal::I64(v), FlatType::I64, FlatType::I32) => RuntimeVal::I32(*v as i32),
        (RuntimeVal::F32(v), FlatType::F32, FlatType::F64) => RuntimeVal::F64(f64::from(*v)),
        (RuntimeVal::F64(v), FlatType::F64, FlatType::F32) => RuntimeVal::F32(*v as f32),
        (RuntimeVal::I64(v), FlatType::I64, FlatType::F64) => {
            RuntimeVal::F64(f64::from_bits(*v as u64))
        }
        (RuntimeVal::F64(v), FlatType::F64, FlatType::I64) => RuntimeVal::I64(v.to_bits() as i64),
        _ => return Err(mismatch()),
    })
}

fn matches_flat(slot: &RuntimeVal, ty: FlatType) -> bool {
    matches!(
        (slot, ty),
        (RuntimeVal::I32(_), FlatType::I32)
            | (RuntimeVal::I64(_), FlatType::I64)
            | (RuntimeVal::F32(_), FlatType::F32)
            | (RuntimeVal::F64(_), FlatType::F64)
    )
}

fn primitive_to_flat(
    prim: PrimitiveType,
    val: &Val,
    position: AbiPosition,
    ty: &ValueType,
) -> Result<RuntimeVal> {
    let mismatch = || host_value_mismatch(ty, position);
    Ok(match (prim, val) {
        (PrimitiveType::Bool, Val::Bool(b)) => RuntimeVal::I32(i32::from(*b)),
        (PrimitiveType::S8, Val::S8(v)) => RuntimeVal::I32(i32::from(*v)),
        (PrimitiveType::U8, Val::U8(v)) => RuntimeVal::I32(i32::from(*v)),
        (PrimitiveType::S16, Val::S16(v)) => RuntimeVal::I32(i32::from(*v)),
        (PrimitiveType::U16, Val::U16(v)) => RuntimeVal::I32(i32::from(*v)),
        (PrimitiveType::S32, Val::S32(v)) => RuntimeVal::I32(*v),
        (PrimitiveType::U32, Val::U32(v)) => RuntimeVal::I32(*v as i32),
        (PrimitiveType::S64, Val::S64(v)) => RuntimeVal::I64(*v),
        (PrimitiveType::U64, Val::U64(v)) => RuntimeVal::I64(*v as i64),
        (PrimitiveType::F32, Val::F32(v)) => RuntimeVal::F32(*v),
        (PrimitiveType::F64, Val::F64(v)) => RuntimeVal::F64(*v),
        (PrimitiveType::Char, Val::Char(c)) => RuntimeVal::I32(*c as i32),
        (PrimitiveType::String, _) => {
            return Err(Error::internal(
                "primitive_to_flat reached the string arm; strings are handled by lower_into_flat_slots",
            ));
        }
        _ => return Err(mismatch()),
    })
}

fn primitive_from_flat(
    prim: PrimitiveType,
    args: &[RuntimeVal],
    cursor: &mut usize,
    ty: &ValueType,
    position: AbiPosition,
) -> Result<Val> {
    let mismatch = || host_value_mismatch(ty, position);
    let take = |cursor: &mut usize| {
        let v = args.get(*cursor).cloned();
        *cursor += 1;
        v
    };
    match prim {
        PrimitiveType::Bool => match take(cursor) {
            Some(RuntimeVal::I32(v)) => Ok(Val::Bool(v != 0)),
            _ => Err(mismatch()),
        },
        PrimitiveType::S8 => match take(cursor) {
            Some(RuntimeVal::I32(v)) => Ok(Val::S8(v as i8)),
            _ => Err(mismatch()),
        },
        PrimitiveType::U8 => match take(cursor) {
            Some(RuntimeVal::I32(v)) => Ok(Val::U8(v as u8)),
            _ => Err(mismatch()),
        },
        PrimitiveType::S16 => match take(cursor) {
            Some(RuntimeVal::I32(v)) => Ok(Val::S16(v as i16)),
            _ => Err(mismatch()),
        },
        PrimitiveType::U16 => match take(cursor) {
            Some(RuntimeVal::I32(v)) => Ok(Val::U16(v as u16)),
            _ => Err(mismatch()),
        },
        PrimitiveType::S32 => match take(cursor) {
            Some(RuntimeVal::I32(v)) => Ok(Val::S32(v)),
            _ => Err(mismatch()),
        },
        PrimitiveType::U32 => match take(cursor) {
            Some(RuntimeVal::I32(v)) => Ok(Val::U32(v as u32)),
            _ => Err(mismatch()),
        },
        PrimitiveType::S64 => match take(cursor) {
            Some(RuntimeVal::I64(v)) => Ok(Val::S64(v)),
            _ => Err(mismatch()),
        },
        PrimitiveType::U64 => match take(cursor) {
            Some(RuntimeVal::I64(v)) => Ok(Val::U64(v as u64)),
            _ => Err(mismatch()),
        },
        PrimitiveType::F32 => match take(cursor) {
            Some(RuntimeVal::F32(v)) => Ok(Val::F32(v)),
            _ => Err(mismatch()),
        },
        PrimitiveType::F64 => match take(cursor) {
            Some(RuntimeVal::F64(v)) => Ok(Val::F64(v)),
            _ => Err(mismatch()),
        },
        PrimitiveType::Char => match take(cursor) {
            Some(RuntimeVal::I32(v)) => char::from_u32(v as u32).map(Val::Char).ok_or_else(|| {
                invalid_encoding(ty, position, "char arg is not a valid Unicode scalar")
            }),
            _ => Err(mismatch()),
        },
        PrimitiveType::String => Err(Error::internal(
            "primitive_from_flat reached the string arm; strings are handled by lift_from_flat_slots",
        )),
    }
}

fn lift_string_from_memory<T: 'static>(
    ctx: &mut LiftContext<'_, T>,
    ptr: usize,
    units: usize,
    ty: &ValueType,
    position: AbiPosition,
) -> Result<Val> {
    match ctx.string_encoding {
        StringEncoding::Utf8 => {
            let bytes = ctx.read_bytes(ptr, units, position, ty)?;
            String::from_utf8(bytes)
                .map(Val::String)
                .map_err(|_| invalid_encoding(ty, position, "invalid UTF-8 string"))
        }
        StringEncoding::Utf16 => {
            let byte_len = units
                .checked_mul(2)
                .ok_or_else(|| invalid_encoding(ty, position, "utf16 length overflow"))?;
            let raw = ctx.read_bytes(ptr, byte_len, position, ty)?;
            let units_vec: Vec<u16> = raw
                .chunks_exact(2)
                .map(|p| u16::from_le_bytes([p[0], p[1]]))
                .collect();
            String::from_utf16(&units_vec)
                .map(Val::String)
                .map_err(|_| invalid_encoding(ty, position, "invalid UTF-16 string"))
        }
        StringEncoding::CompactUtf16 => Err(invalid_encoding(
            ty,
            position,
            "Latin-1+UTF-16 string encoding is not yet implemented; the synchronous baseline tests use UTF-8",
        )),
    }
}

fn lower_string<T: 'static>(
    ctx: &mut LowerContext<'_, T>,
    s: &str,
    position: AbiPosition,
    ty: &ValueType,
) -> Result<(usize, usize)> {
    let (bytes, units) = match ctx.string_encoding {
        StringEncoding::Utf8 => (s.as_bytes().to_vec(), s.len()),
        StringEncoding::Utf16 => {
            let units: Vec<u16> = s.encode_utf16().collect();
            let mut bytes = Vec::with_capacity(units.len() * 2);
            for u in &units {
                bytes.extend_from_slice(&u.to_le_bytes());
            }
            (bytes, units.len())
        }
        StringEncoding::CompactUtf16 => {
            return Err(invalid_encoding(
                ty,
                position,
                "Latin-1+UTF-16 string encoding is not yet implemented; the synchronous baseline tests use UTF-8",
            ));
        }
    };
    let ptr = if bytes.is_empty() {
        0
    } else {
        ctx.allocate(bytes.len(), ty, position)?
    };
    if !bytes.is_empty() {
        ctx.write_bytes(ptr, &bytes, position, ty)?;
    }
    Ok((ptr, units))
}

fn validate_handle_in_table<T: 'static>(
    ctx: &LowerContext<'_, T>,
    handle: &ResourceHandle,
    ty: &ValueType,
    position: AbiPosition,
) -> Result<()> {
    let tables = ctx.tables.as_ref().ok_or_else(|| {
        Error::from(AbiError {
            position,
            valtype: ty.clone(),
            cause: AbiCause::InvalidHandle {
                reason: "no handle-tables ledger available to the lower context".to_owned(),
            },
        })
    })?;
    let guard = tables
        .lock()
        .map_err(|_| Error::internal("resource handle tables lock poisoned"))?;
    let table = guard.for_type(handle.type_id).ok_or_else(|| {
        Error::from(AbiError {
            position,
            valtype: ty.clone(),
            cause: AbiCause::UnregisteredResourceType,
        })
    })?;
    if table.get(handle.index).is_none() {
        return Err(Error::from(AbiError {
            position,
            valtype: ty.clone(),
            cause: AbiCause::InvalidHandle {
                reason: format!(
                    "handle index {} is not live in the resource table",
                    handle.index
                ),
            },
        }));
    }
    Ok(())
}

fn take_i32(
    args: &[RuntimeVal],
    cursor: &mut usize,
    ty: &ValueType,
    position: AbiPosition,
) -> Result<i32> {
    match args.get(*cursor) {
        Some(RuntimeVal::I32(v)) => {
            *cursor += 1;
            Ok(*v)
        }
        _ => Err(host_value_mismatch(ty, position)),
    }
}

fn take_runtime_val(
    args: &[RuntimeVal],
    cursor: &mut usize,
    ty: &ValueType,
    position: AbiPosition,
) -> Result<RuntimeVal> {
    let v = args.get(*cursor).cloned().ok_or_else(|| {
        invalid_encoding(
            ty,
            position,
            "ran out of flat slots while lifting variant payload",
        )
    })?;
    *cursor += 1;
    Ok(v)
}

fn invalid_encoding(ty: &ValueType, position: AbiPosition, message: &str) -> Error {
    Error::from(AbiError {
        position,
        valtype: ty.clone(),
        cause: AbiCause::InvalidEncoding {
            message: message.to_owned(),
        },
    })
}

fn host_value_mismatch(ty: &ValueType, position: AbiPosition) -> Error {
    Error::from(AbiError {
        position,
        valtype: ty.clone(),
        cause: AbiCause::HostValueMismatch,
    })
}
