//! Canonical-ABI lift: read a [`Val`] out of guest memory.
//!
//! The entry point [`lift`] takes a [`LiftContext`], a memory
//! offset, and the destination [`ValueType`]. It recurses through
//! compound shapes, dispatching to per-variant primitives at the
//! leaves.

use crate::abi::context::LiftContext;
use crate::abi::layout::{align_to, alignment_of, discriminant_size, size_of};
use crate::abi::strings;
use crate::error::{AbiCause, AbiError, AbiPosition, Error, Result};
use crate::resource::{HandleKind, ResourceHandle};
use crate::types::{PrimitiveType, ValueType};
use crate::value::{Val, ValField};

/// Lift the value of type `ty` out of the guest's linear memory at
/// `offset`. The lift is recursive; compound types read field- or
/// element-wise after computing per-element offsets via
/// [`crate::abi::layout`].
pub fn lift<T: 'static>(
    ctx: &mut LiftContext<'_, T>,
    offset: usize,
    ty: &ValueType,
    position: AbiPosition,
) -> Result<Val> {
    match ty {
        ValueType::Primitive(prim) => lift_primitive(ctx, offset, *prim, position),
        ValueType::List(list) => {
            let element_ty = list.element().clone();
            let ptr_bytes = ctx.read_bytes(offset, 4, position, ty)?;
            let len_bytes = ctx.read_bytes(offset + 4, 4, position, ty)?;
            let ptr = read_u32(&ptr_bytes) as usize;
            let len = read_u32(&len_bytes) as usize;
            let element_size = size_of(&element_ty);
            let byte_len = len
                .checked_mul(element_size)
                .ok_or_else(|| invalid_encoding(ty, position, "list length overflow"))?;
            if !ctx.in_bounds(ptr, byte_len) {
                return Err(invalid_encoding(
                    ty,
                    position,
                    "list pointer/length out of bounds of memory",
                ));
            }
            let mut out = Vec::with_capacity(len);
            for i in 0..len {
                let elem = lift(ctx, ptr + i * element_size, &element_ty, position)?;
                out.push(elem);
            }
            Ok(Val::List(out.into_boxed_slice()))
        }
        ValueType::FixedLengthList(fixed) => {
            // Elements sit inline, one element size apart.
            let element_ty = fixed.element();
            let element_size = size_of(element_ty);
            let mut out = Vec::with_capacity(fixed.length() as usize);
            for i in 0..fixed.length() as usize {
                out.push(lift(ctx, offset + i * element_size, element_ty, position)?);
            }
            Ok(Val::FixedLengthList(out.into_boxed_slice()))
        }
        ValueType::Map(map) => {
            // A map is laid out as the list of its entry tuples.
            let entries = lift(ctx, offset, &crate::abi::map_entries_type(map), position)?;
            crate::abi::entries_to_map(entries, ty, position)
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
            let disc_bytes = ctx.read_bytes(offset, disc_size, position, ty)?;
            let discriminant = read_discriminant(&disc_bytes);
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
            let disc_bytes = ctx.read_bytes(offset, 1, position, ty)?;
            let discriminant = disc_bytes[0] as usize;
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
            let disc_bytes = ctx.read_bytes(offset, 1, position, ty)?;
            let discriminant = disc_bytes[0] as usize;
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
            let disc_bytes = ctx.read_bytes(offset, disc_size, position, ty)?;
            let discriminant = read_discriminant(&disc_bytes);
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
            let bytes = ctx.read_bytes(offset, 4, position, ty)?;
            let index = read_u32(&bytes);
            lift_handle(ctx, index, ty, position, matches!(ty, ValueType::Own(_)))
        }
    }
}

fn lift_primitive<T: 'static>(
    ctx: &mut LiftContext<'_, T>,
    offset: usize,
    prim: PrimitiveType,
    position: AbiPosition,
) -> Result<Val> {
    let ty = ValueType::Primitive(prim);
    match prim {
        PrimitiveType::Bool => {
            let bytes = ctx.read_bytes(offset, 1, position, &ty)?;
            Ok(Val::Bool(bytes[0] != 0))
        }
        PrimitiveType::S8 => {
            let bytes = ctx.read_bytes(offset, 1, position, &ty)?;
            Ok(Val::S8(bytes[0] as i8))
        }
        PrimitiveType::U8 => {
            let bytes = ctx.read_bytes(offset, 1, position, &ty)?;
            Ok(Val::U8(bytes[0]))
        }
        PrimitiveType::S16 => {
            let bytes = ctx.read_bytes(offset, 2, position, &ty)?;
            Ok(Val::S16(i16::from_le_bytes([bytes[0], bytes[1]])))
        }
        PrimitiveType::U16 => {
            let bytes = ctx.read_bytes(offset, 2, position, &ty)?;
            Ok(Val::U16(u16::from_le_bytes([bytes[0], bytes[1]])))
        }
        PrimitiveType::S32 => {
            let bytes = ctx.read_bytes(offset, 4, position, &ty)?;
            Ok(Val::S32(i32::from_le_bytes(bytes_4(&bytes))))
        }
        PrimitiveType::U32 => {
            let bytes = ctx.read_bytes(offset, 4, position, &ty)?;
            Ok(Val::U32(u32::from_le_bytes(bytes_4(&bytes))))
        }
        PrimitiveType::S64 => {
            let bytes = ctx.read_bytes(offset, 8, position, &ty)?;
            Ok(Val::S64(i64::from_le_bytes(bytes_8(&bytes))))
        }
        PrimitiveType::U64 => {
            let bytes = ctx.read_bytes(offset, 8, position, &ty)?;
            Ok(Val::U64(u64::from_le_bytes(bytes_8(&bytes))))
        }
        PrimitiveType::F32 => {
            let bytes = ctx.read_bytes(offset, 4, position, &ty)?;
            Ok(Val::F32(f32::from_le_bytes(bytes_4(&bytes))))
        }
        PrimitiveType::F64 => {
            let bytes = ctx.read_bytes(offset, 8, position, &ty)?;
            Ok(Val::F64(f64::from_le_bytes(bytes_8(&bytes))))
        }
        PrimitiveType::Char => {
            let bytes = ctx.read_bytes(offset, 4, position, &ty)?;
            let raw = u32::from_le_bytes(bytes_4(&bytes));
            char::from_u32(raw).map(Val::Char).ok_or_else(|| {
                invalid_encoding(&ty, position, "char value is not a valid Unicode scalar")
            })
        }
        PrimitiveType::String => {
            let ptr_bytes = ctx.read_bytes(offset, 4, position, &ty)?;
            let len_bytes = ctx.read_bytes(offset + 4, 4, position, &ty)?;
            let ptr = read_u32(&ptr_bytes) as usize;
            let len = read_u32(&len_bytes) as usize;
            lift_string(ctx, ptr, len, position, &ty)
        }
    }
}

fn lift_string<T: 'static>(
    ctx: &mut LiftContext<'_, T>,
    ptr: usize,
    units: usize,
    position: AbiPosition,
    ty: &ValueType,
) -> Result<Val> {
    let encoding = ctx.string_encoding;
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
    let raw = ctx.read_bytes(ptr, byte_len, position, ty)?;
    strings::decode(encoding, units, &raw)
        .map(Val::String)
        .map_err(|message| invalid_encoding(ty, position, message))
}

fn read_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes_4(bytes))
}

fn bytes_4(bytes: &[u8]) -> [u8; 4] {
    let mut out = [0u8; 4];
    out.copy_from_slice(&bytes[..4]);
    out
}

fn bytes_8(bytes: &[u8]) -> [u8; 8] {
    let mut out = [0u8; 8];
    out.copy_from_slice(&bytes[..8]);
    out
}

fn read_discriminant(bytes: &[u8]) -> usize {
    match bytes.len() {
        1 => bytes[0] as usize,
        2 => u16::from_le_bytes([bytes[0], bytes[1]]) as usize,
        4 => u32::from_le_bytes(bytes_4(bytes)) as usize,
        _ => 0,
    }
}

/// Lift a `own<T>` or `borrow<T>` handle from a 4-byte index that
/// has already been read out of the flat slot or memory location.
///
/// The lift cross-references the index against the per-store
/// handle tables: the polyfill keeps one table per registered
/// resource type, addressed by the registered
/// [`ResourceTypeId`](crate::resource::ResourceTypeId). For an
/// `own<T>` lift the entry is removed from the table — ownership
/// transfers to the host. For `borrow<T>` the entry is left in
/// place and the host receives a handle that aliases the live entry.
///
/// The declared `ValueType` names the resource table by its index in
/// the component, and the lift context maps that to the table the
/// instance keeps and the resource type it holds.
pub fn lift_handle<T: 'static>(
    ctx: &mut LiftContext<'_, T>,
    index: u32,
    ty: &ValueType,
    position: AbiPosition,
    is_own: bool,
) -> Result<Val> {
    let tables = ctx.tables.clone().ok_or_else(|| {
        Error::from(AbiError {
            position,
            valtype: ty.clone(),
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
        .and_then(|i| ctx.resource_tables.get(i).copied().flatten())
        .ok_or_else(|| {
            Error::from(AbiError {
                position,
                valtype: ty.clone(),
                cause: AbiCause::InvalidHandle {
                    reason: "the handle's type names no resource table of the instance".to_owned(),
                },
            })
        })?;
    let invalid = |reason: String| {
        Error::from(AbiError {
            position,
            valtype: ty.clone(),
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
        Ok(Val::Own(ResourceHandle {
            type_id: table.type_id,
            index: host_index,
            rep,
        }))
    } else if table.defining {
        // The defining instance passes its own resource's rep for a
        // borrow, with no table entry behind it.
        Ok(Val::Borrow(ResourceHandle {
            type_id: table.type_id,
            index,
            rep: index,
        }))
    } else {
        // A borrow lifted out of an owning entry lends that entry to
        // the host for the rest of the call; a borrow of a borrow
        // needs no bookkeeping of its own.
        let entry = guard
            .lookup(table.table, index, table.type_id, table.guest_defined)
            .map_err(|e| invalid(e.to_string()))?;
        if matches!(entry, HandleKind::Own { .. }) {
            guard.lend(table.table, index);
        }
        Ok(Val::Borrow(ResourceHandle {
            type_id: table.type_id,
            index,
            rep: entry
                .rep()
                .expect("lookup only ever returns a resource entry"),
        }))
    }
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

/// The resource index a handle type declares, when it declares one.
pub fn declared_resource_index(ty: &ValueType) -> Option<usize> {
    match ty {
        ValueType::Own(resource) | ValueType::Borrow(resource) => resource.index(),
        _ => None,
    }
}
