//! Canonical-ABI lower: write a [`Val`] into guest memory.
//!
//! The entry point [`lower`] takes a [`LowerContext`], a memory
//! offset, and the value plus its declared type. It recurses through
//! compound shapes, calling `cabi_realloc` via the context for
//! heap-allocating value types (string and list).

use crate::abi::context::LowerContext;
use crate::abi::layout::{align_to, alignment_of, discriminant_size, size_of};
use crate::abi::lift::declared_resource_index;
use crate::error::{AbiCause, AbiError, AbiPosition, Error, Result};
use crate::executor::ir::StringEncoding;
use crate::resource::ResourceHandle;
use crate::types::{PrimitiveType, ValueType};
use crate::value::Val;

/// Lower `value` of declared type `ty` into the guest's linear
/// memory at `offset`. Heap-allocating types call into `cabi_realloc`
/// via [`LowerContext::allocate`].
pub fn lower<T: 'static>(
    ctx: &mut LowerContext<'_, T>,
    offset: usize,
    value: &Val,
    ty: &ValueType,
    position: AbiPosition,
) -> Result<()> {
    match (ty, value) {
        (ValueType::Primitive(prim), _) => lower_primitive(ctx, offset, value, *prim, position),
        (ValueType::List(list), Val::List(elements)) => {
            let element_ty = list.element();
            let element_size = size_of(element_ty);
            let total_size = element_size.saturating_mul(elements.len());
            let ptr = if total_size == 0 {
                0
            } else {
                ctx.allocate(total_size, ty, position)?
            };
            for (i, element) in elements.iter().enumerate() {
                lower(ctx, ptr + i * element_size, element, element_ty, position)?;
            }
            ctx.write_bytes(offset, &(ptr as u32).to_le_bytes(), position, ty)?;
            ctx.write_bytes(
                offset + 4,
                &(elements.len() as u32).to_le_bytes(),
                position,
                ty,
            )?;
            Ok(())
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
        _ => Err(host_value_mismatch(ty, position)),
    }
}

fn lower_primitive<T: 'static>(
    ctx: &mut LowerContext<'_, T>,
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
    ctx: &mut LowerContext<'_, T>,
    offset: usize,
    s: &str,
    position: AbiPosition,
    ty: &ValueType,
) -> Result<()> {
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
            return Err(Error::from(AbiError {
                position,
                valtype: ty.clone(),
                cause: AbiCause::InvalidEncoding {
                    message:
                        "Latin-1+UTF-16 string encoding is not yet implemented; the synchronous baseline tests use UTF-8"
                            .to_owned(),
                },
            }));
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
    ctx.write_bytes(offset, &(ptr as u32).to_le_bytes(), position, ty)?;
    ctx.write_bytes(offset + 4, &(units as u32).to_le_bytes(), position, ty)?;
    Ok(())
}

fn write_discriminant<T: 'static>(
    ctx: &mut LowerContext<'_, T>,
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
            valtype: ty.clone(),
            cause: AbiCause::InvalidEncoding {
                message: format!("unexpected discriminant width: {width}"),
            },
        })),
    }
}

/// Lower a handle into the guest and return the index to write.
///
/// The declared parameter names a resource by its index in the
/// component; a handle whose identity is not the one the instance
/// uses for that resource (one minted by another instance of the
/// same component, say) is rejected with the unregistered-resource-
/// type cause. A handle whose entry is live in the table lowers as
/// that index: the host minted it with `Store::resource_new`, or
/// lends it as a borrow. A handle whose entry is gone is one the
/// host owns outright (an `own<T>` lifted out of a guest); its rep
/// is inserted again and the fresh index is written, which is the
/// canonical ABI's transfer of ownership back into the guest.
pub fn lower_handle<T: 'static>(
    ctx: &LowerContext<'_, T>,
    handle: &ResourceHandle,
    ty: &ValueType,
    position: AbiPosition,
) -> Result<u32> {
    let tables = ctx.tables.as_ref().ok_or_else(|| {
        Error::from(AbiError {
            position,
            valtype: ty.clone(),
            cause: AbiCause::InvalidHandle {
                reason: "no handle-tables ledger available to the lower context".to_owned(),
            },
        })
    })?;
    if let Some(expected) =
        declared_resource_index(ty).and_then(|i| ctx.resource_types.get(i).copied())
        && expected != handle.type_id
    {
        return Err(Error::from(AbiError {
            position,
            valtype: ty.clone(),
            cause: AbiCause::UnregisteredResourceType,
        }));
    }
    let mut guard = tables.lock().map_err(|_| Error::Internal {
        message: "resource handle tables lock poisoned".to_owned(),
    })?;
    if guard.for_type(handle.type_id).is_none() {
        return Err(Error::from(AbiError {
            position,
            valtype: ty.clone(),
            cause: AbiCause::UnregisteredResourceType,
        }));
    }
    let table = guard.for_type_mut(handle.type_id);
    if table.get(handle.index).is_some() {
        Ok(handle.index)
    } else {
        Ok(table.insert(handle.rep))
    }
}

fn host_value_mismatch(ty: &ValueType, position: AbiPosition) -> Error {
    Error::from(AbiError {
        position,
        valtype: ty.clone(),
        cause: AbiCause::HostValueMismatch,
    })
}
