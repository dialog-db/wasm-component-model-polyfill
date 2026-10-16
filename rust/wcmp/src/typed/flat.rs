// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The pieces every typed crossing shares: reading and writing one
//! flat value, the discriminant and payload of a variant, and the
//! failures of a crossing whose declared type is not the one the type
//! check compared.

use crate::abi::flatten::{matches_flat, reinterpret_flat, zero_of_flat};
use crate::abi::layout::{FlatType, discriminant_size, flat_types};
use crate::abi::{read_discriminant, write_discriminant};
use crate::error::{AbiCause, AbiError, AbiPosition, Error, Result};
use crate::internal::ErrorInternal;
use crate::internal::{LiftContextInternal, LowerContextInternal};
use crate::runtime_layer::Val as CoreVal;
use crate::types::ValueType;

use super::lift_context::LiftContext;
use super::lower_context::LowerContext;

/// The element type of the list `ty` declares.
pub fn declared_list(ty: &ValueType) -> Option<&ValueType> {
    match ty {
        ValueType::List(list) => Some(list.element()),
        _ => None,
    }
}

/// The failure of a crossing whose declared type is not the shape the
/// Rust type stands for. The type check compares the two when the host
/// acquires the function, so only a value paired with a type of the
/// caller's own reaches it.
pub fn list_layout_error(ty: &ValueType, position: AbiPosition) -> Error {
    declared_mismatch(ty, position)
}

/// The failure of a crossing whose declared type `ty` is not the type
/// the Rust value stands for.
pub fn declared_mismatch(ty: &ValueType, position: AbiPosition) -> Error {
    Error::from(AbiError {
        position,
        valtype: Some(ty.clone()),
        cause: AbiCause::HostValueMismatch,
    })
}

/// The failure of a crossing that read bytes that are no value of
/// `ty`.
pub fn invalid_encoding(ty: &ValueType, position: AbiPosition, message: &str) -> Error {
    Error::from(AbiError {
        position,
        valtype: Some(ty.clone()),
        cause: AbiCause::InvalidEncoding {
            message: message.to_owned(),
        },
    })
}

/// Take the flat value at `cursor`, of the flat type `want`, and move
/// `cursor` past it.
pub fn take(
    slots: &[CoreVal],
    cursor: &mut usize,
    want: FlatType,
    ty: &ValueType,
    position: AbiPosition,
) -> Result<CoreVal> {
    match slots.get(*cursor) {
        Some(slot) if matches_flat(slot, want) => {
            *cursor += 1;
            Ok(*slot)
        }
        _ => Err(declared_mismatch(ty, position)),
    }
}

/// Take the `i32` flat value at `cursor`.
pub fn take_i32(
    slots: &[CoreVal],
    cursor: &mut usize,
    ty: &ValueType,
    position: AbiPosition,
) -> Result<i32> {
    match take(slots, cursor, FlatType::I32, ty, position)? {
        CoreVal::I32(value) => Ok(value),
        _ => Err(declared_mismatch(ty, position)),
    }
}

/// The discriminant and the payload type of case `case` of a type with
/// cases: a variant, an option, or a result.
pub fn case_payload(ty: &ValueType, case: usize) -> Option<Option<&ValueType>> {
    match ty {
        ValueType::Variant(variant) => variant.cases().get(case).map(|case| case.payload()),
        ValueType::Option(option) => match case {
            0 => Some(None),
            1 => Some(Some(option.payload())),
            _ => None,
        },
        ValueType::Result(result) => match case {
            0 => Some(result.ok()),
            1 => Some(result.err()),
            _ => None,
        },
        ValueType::Enum(en) => (case < en.cases().len()).then_some(None),
        _ => None,
    }
}

/// How many cases a type with cases declares.
pub fn case_count(ty: &ValueType) -> usize {
    match ty {
        ValueType::Variant(variant) => variant.cases().len(),
        ValueType::Enum(en) => en.cases().len(),
        ValueType::Option(_) | ValueType::Result(_) => 2,
        _ => 0,
    }
}

/// Lower case `case` of the type with cases `ty` into its flat values:
/// the discriminant, then the payload `lower` writes, carried into the
/// slots the cases share and padded with zeros, as the Canonical ABI's
/// `lower_flat_variant` does.
pub fn lower_case_flat<D: 'static>(
    cx: &mut LowerContext<'_, '_, D>,
    ty: &ValueType,
    case: usize,
    out: &mut Vec<CoreVal>,
    lower: impl FnOnce(&mut LowerContext<'_, '_, D>, &ValueType, &mut Vec<CoreVal>) -> Result<()>,
) -> Result<()> {
    let position = cx.position();
    let payload_ty = case_payload(ty, case).ok_or_else(|| declared_mismatch(ty, position))?;
    let joined = flat_types(ty);
    let joined = joined.get(1..).unwrap_or_default();
    out.push(CoreVal::I32(case as i32));
    let Some(payload_ty) = payload_ty else {
        out.extend(joined.iter().copied().map(zero_of_flat));
        return Ok(());
    };
    let start = out.len();
    lower(cx, payload_ty, out)?;
    let own = flat_types(payload_ty);
    if out.len() - start != own.len() || own.len() > joined.len() {
        return Err(Error::internal(
            "a variant payload's flat values disagree with its declared flat shape",
        ));
    }
    for (index, joined_ty) in joined.iter().copied().enumerate() {
        match own.get(index) {
            Some(own_ty) => {
                let slot = out[start + index];
                out[start + index] = reinterpret_flat(&slot, *own_ty, joined_ty, ty, position)?;
            }
            None => out.push(zero_of_flat(joined_ty)),
        }
    }
    Ok(())
}

/// Lift a value of the type with cases `ty` out of its flat values:
/// read the discriminant, carry the shared slots back into the flat
/// shape of the case's payload, and hand them to `lift`, as the
/// Canonical ABI's `lift_flat_variant` does. `cursor` moves past every
/// shared slot whatever the case.
pub fn lift_case_flat<D: 'static, R>(
    cx: &mut LiftContext<'_, '_, D>,
    ty: &ValueType,
    slots: &[CoreVal],
    cursor: &mut usize,
    lift: impl FnOnce(&mut LiftContext<'_, '_, D>, usize, Option<(&ValueType, &[CoreVal])>) -> Result<R>,
) -> Result<R> {
    let position = cx.position();
    let case = take_i32(slots, cursor, ty, position)? as u32 as usize;
    let joined = flat_types(ty);
    let joined = joined.get(1..).unwrap_or_default();
    let mut shared = Vec::with_capacity(joined.len());
    for joined_ty in joined.iter().copied() {
        shared.push(take(slots, cursor, joined_ty, ty, position)?);
    }
    let payload_ty = case_payload(ty, case).ok_or_else(|| {
        invalid_encoding(
            ty,
            position,
            &format!("discriminant {case} out of range [0..{})", case_count(ty)),
        )
    })?;
    let Some(payload_ty) = payload_ty else {
        return lift(cx, case, None);
    };
    let own = flat_types(payload_ty);
    let mut decoded = Vec::with_capacity(own.len());
    for (index, own_ty) in own.iter().copied().enumerate() {
        let joined_ty = joined.get(index).copied().ok_or_else(|| {
            invalid_encoding(
                ty,
                position,
                "active case has more flat slots than the joined payload",
            )
        })?;
        decoded.push(reinterpret_flat(
            &shared[index],
            joined_ty,
            own_ty,
            ty,
            position,
        )?);
    }
    lift(cx, case, Some((payload_ty, &decoded)))
}

/// The offset of the payload of the type with cases `ty`, from the
/// start of the value, and the width of its discriminant.
pub fn case_layout(ty: &ValueType) -> (usize, usize) {
    let width = discriminant_size(case_count(ty).max(1));
    let align = (0..case_count(ty))
        .filter_map(|case| case_payload(ty, case).flatten())
        .map(crate::abi::layout::alignment_of)
        .max()
        .unwrap_or(1);
    (crate::abi::layout::align_to(width, align), width)
}

/// Write the discriminant `case` of the type with cases `ty` at
/// `offset`, and answer the offset of the payload.
pub fn lower_case_memory<'t, D: 'static>(
    cx: &mut LowerContext<'_, '_, D>,
    ty: &'t ValueType,
    case: usize,
    offset: usize,
) -> Result<(usize, Option<&'t ValueType>)> {
    let position = cx.position();
    let payload_ty = case_payload(ty, case).ok_or_else(|| declared_mismatch(ty, position))?;
    let (payload, width) = case_layout(ty);
    write_discriminant(cx.boundary(), offset, case, width, ty, position)?;
    Ok((offset + payload, payload_ty))
}

/// Read the discriminant of the type with cases `ty` at `offset`, and
/// answer the case, the offset of its payload, and the payload's type.
pub fn lift_case_memory<'t, D: 'static>(
    cx: &mut LiftContext<'_, '_, D>,
    ty: &'t ValueType,
    offset: usize,
) -> Result<(usize, usize, Option<&'t ValueType>)> {
    let position = cx.position();
    let (payload, width) = case_layout(ty);
    let case = read_discriminant(cx.boundary(), offset, width, position, ty)?;
    let payload_ty = case_payload(ty, case).ok_or_else(|| {
        invalid_encoding(
            ty,
            position,
            &format!("discriminant {case} out of range [0..{})", case_count(ty)),
        )
    })?;
    Ok((case, offset + payload, payload_ty))
}

/// `offset` rounded up to a multiple of `align`, in a constant: where a
/// field of alignment `align` that follows `offset` bytes starts.
pub const fn align_to(offset: usize, align: u32) -> usize {
    let align = align as usize;
    offset.div_ceil(align) * align
}
