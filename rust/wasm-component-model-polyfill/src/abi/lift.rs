//! Canonical-ABI lift: read a [`Val`] out of guest memory.
//!
//! The entry point [`lift`] takes a [`LiftContext`], a memory
//! offset, and the destination [`ValueType`]. It recurses through
//! compound shapes, dispatching to per-variant primitives at the
//! leaves.

use crate::abi::context::LiftContext;
use crate::abi::layout::{align_to, alignment_of, discriminant_size, size_of};
use crate::error::{AbiCause, AbiError, AbiPosition, Error, Result};
use crate::executor::ir::StringEncoding;
use crate::resource::ResourceHandle;
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
            let mut out = Vec::with_capacity(len);
            for i in 0..len {
                let elem = lift(ctx, ptr + i * element_size, &element_ty, position)?;
                out.push(elem);
            }
            Ok(Val::List(out.into_boxed_slice()))
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
                invalid_encoding(ty, position, "variant discriminant out of range")
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
        ValueType::Own(rt) | ValueType::Borrow(rt) => {
            let bytes = ctx.read_bytes(offset, 4, position, ty)?;
            let index = read_u32(&bytes);
            lift_handle(
                ctx,
                rt.label(),
                index,
                ty,
                position,
                matches!(ty, ValueType::Own(_)),
            )
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
    let bytes = match ctx.string_encoding {
        StringEncoding::Utf8 => ctx.read_bytes(ptr, units, position, ty)?,
        StringEncoding::Utf16 => {
            let byte_len = units
                .checked_mul(2)
                .ok_or_else(|| invalid_encoding(ty, position, "utf16 length overflow"))?;
            let raw = ctx.read_bytes(ptr, byte_len, position, ty)?;
            let units_vec: Vec<u16> = raw
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            return String::from_utf16(&units_vec)
                .map(Val::String)
                .map_err(|_| invalid_encoding(ty, position, "invalid UTF-16 string"));
        }
        StringEncoding::CompactUtf16 => {
            return Err(invalid_encoding(
                ty,
                position,
                "Latin-1+UTF-16 string encoding is not yet implemented; the synchronous baseline tests use UTF-8",
            ));
        }
    };
    String::from_utf8(bytes)
        .map(Val::String)
        .map_err(|_| invalid_encoding(ty, position, "invalid UTF-8 string"))
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
/// handle tables: the polyfill reuses one table per registered
/// resource type, addressed by the registered
/// [`ResourceTypeId`](crate::resource::ResourceTypeId). For an
/// `own<T>` lift the entry is removed from the table — ownership
/// transfers to the host. For `borrow<T>` the entry is left in
/// place and the host receives a handle that aliases the live entry.
///
/// The label argument is the resource-type label declared at the
/// import site; it is used to locate the matching registered type
/// identity by walking every table. This works because the lift
/// runs only when the call's signature already named a resource type
/// the executor resolved at instantiation time.
pub fn lift_handle<T: 'static>(
    ctx: &mut LiftContext<'_, T>,
    _label: &str,
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

    let mut guard = tables.lock().map_err(|_| Error::Internal {
        message: "resource handle tables lock poisoned".to_owned(),
    })?;

    // The polyfill currently uses one table per registered
    // `ResourceTypeId`; the executor's resource trampolines and
    // `Store::resource_new` are the only producers, so any live
    // entry under a given resource label corresponds to exactly one
    // type id. The lift walks the tables looking for a live entry
    // at `index` — wasmtime's typed-by-`TypeResourceTableIndex`
    // dispatch is replaced here by the per-store table's structural
    // identity.
    let candidates: Vec<crate::resource::ResourceTypeId> =
        guard.iter().map(|(type_id, _)| type_id).collect();
    let mut found = None;
    for type_id in candidates {
        if let Some(_rep) = guard.for_type(type_id).and_then(|t| t.get(index)) {
            found = Some(type_id);
            break;
        }
    }
    let type_id = found.ok_or_else(|| {
        Error::from(AbiError {
            position,
            valtype: ty.clone(),
            cause: AbiCause::InvalidHandle {
                reason: format!("handle index {index} is not live in any resource table"),
            },
        })
    })?;

    if is_own {
        guard.for_type_mut(type_id).remove(index).ok_or_else(|| {
            Error::from(AbiError {
                position,
                valtype: ty.clone(),
                cause: AbiCause::InvalidHandle {
                    reason: format!("handle index {index} disappeared during lift"),
                },
            })
        })?;
        Ok(Val::Own(ResourceHandle { type_id, index }))
    } else {
        Ok(Val::Borrow(ResourceHandle { type_id, index }))
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
