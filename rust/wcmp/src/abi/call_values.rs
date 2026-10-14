// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The arguments and the result of a host call into an export, as
//! they cross the canonical ABI.
//!
//! A call lowers its parameter tuple either into flat core slots or,
//! when the tuple is too wide for them, into one spilled record whose
//! address is the only slot. Its result comes back the same two ways:
//! in flat slots, or through a pointer the core function returns.
//! The functions here decide which, allocate the spill, and gate the
//! returned pointer. They serve the untyped call, which moves one
//! [`Val`] per value, and the typed one, which moves native Rust
//! values through the same decisions.

use super::context::BoundaryContext;
use super::flatten::{lift_from_flat_slots, lower_into_flat_slots};
use super::layout::{alignment_of, result_spills, size_of};
use super::signature::Signature;
use super::{lift, lower};
use crate::component::FunctionType;
use crate::error::{AbiCause, AbiError, AbiPosition, Error, InstantiationError, Result};
use crate::internal::ErrorContextAnyInternal;
use crate::runtime_layer::Val as RuntimeVal;
use crate::store::StoreId;
use crate::types::{PrimitiveType, ValueType};
use crate::value::Val;

/// Refuse `arg` with the wrong-store cause when it holds, at any
/// depth, an error context lifted in a store other than `store`. An
/// error context is the one host value that carries its store's
/// identity.
pub fn check_store(arg: &Val, store: StoreId) -> Result<()> {
    fn walk(val: &Val, store: StoreId) -> Result<()> {
        match val {
            Val::ErrorContext(context) if context.store_id() != store => {
                Err(Error::from(InstantiationError::WrongStore))
            }
            Val::List(items) | Val::FixedLengthList(items) | Val::Tuple(items) => {
                items.iter().try_for_each(|item| walk(item, store))
            }
            Val::Map(entries) => entries.iter().try_for_each(|(key, value)| {
                walk(key, store)?;
                walk(value, store)
            }),
            Val::Record(fields) => fields
                .iter()
                .try_for_each(|field| walk(&field.value, store)),
            Val::Variant {
                payload: Some(inner),
                ..
            }
            | Val::Option(Some(inner))
            | Val::Result(Ok(Some(inner)) | Err(Some(inner))) => walk(inner, store),
            _ => Ok(()),
        }
    }
    walk(arg, store)
}

/// Lower `args` as the parameter tuple of `signature` and answer the
/// core arguments the export's core function takes.
pub fn lower_arguments<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    signature: &Signature,
    args: &[Val],
) -> Result<Vec<RuntimeVal>> {
    if let Some((base, offsets)) = parameter_spill(ctx, signature)? {
        // The whole parameter tuple is written into guest memory at
        // the canonical ABI's record layout, and the core function
        // receives its address.
        for (i, ((param, val), offset)) in signature
            .ty()
            .parameters
            .iter()
            .zip(args.iter())
            .zip(offsets.iter())
            .enumerate()
        {
            lower(ctx, base + offset, val, &param.ty, AbiPosition::Argument(i))?;
        }
        return Ok(vec![RuntimeVal::I32(base as i32)]);
    }

    let mut out: Vec<RuntimeVal> = Vec::new();
    for (i, (param, val)) in signature
        .ty()
        .parameters
        .iter()
        .zip(args.iter())
        .enumerate()
    {
        lower_into_flat_slots(ctx, val, &param.ty, &mut out, AbiPosition::Argument(i))?;
    }
    Ok(out)
}

/// The spilled record of `signature`'s parameter tuple, when the
/// tuple is too wide for flat slots: its address in the guest and the
/// offset of each parameter inside it. `None` when the parameters
/// travel flat.
///
/// The record is allocated through the guest's `cabi_realloc` at the
/// tuple's size and alignment. An empty record allocates nothing and
/// sits at address zero. The layout is the one the signature computed
/// when the component was translated, so a call lays nothing out.
pub fn parameter_spill<'s, T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    signature: &'s Signature,
) -> Result<Option<(usize, &'s [usize])>> {
    if !signature.params_spill() {
        return Ok(None);
    }
    let layout = signature.parameter_layout();
    let spill_ty = ValueType::Primitive(PrimitiveType::U32);
    let base = if layout.size == 0 {
        0
    } else {
        ctx.allocate_aligned(
            layout.size,
            layout.alignment,
            &spill_ty,
            AbiPosition::Argument(0),
        )?
    };
    Ok(Some((base, &layout.offsets)))
}

/// Lift the result of `signature` out of the core results the export
/// returned: `None` for a function that declares no result.
pub fn lift_result_value<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    core_results: &[RuntimeVal],
    signature: &FunctionType,
) -> Result<Option<Val>> {
    let position = AbiPosition::Result;
    let Some(result_ty) = &signature.result else {
        return Ok(None);
    };
    let lifted = if result_spills(signature) {
        let ptr = result_pointer(ctx, core_results, result_ty)?;
        lift(ctx, ptr, result_ty, position)?
    } else {
        let mut cursor = 0usize;
        lift_from_flat_slots(ctx, core_results, &mut cursor, result_ty, position)?
    };
    Ok(Some(lifted))
}

/// The address a core function returned a spilled result of type
/// `result_ty` at.
///
/// The pointer is the guest's, so it is gated before the result is
/// read: aligned as the result type's layout demands, and addressing
/// a region of the result's size that the memory owns.
pub fn result_pointer<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    core_results: &[RuntimeVal],
    result_ty: &ValueType,
) -> Result<usize> {
    let fail = |message: &str| {
        Error::from(AbiError {
            position: AbiPosition::Result,
            valtype: Some(result_ty.clone()),
            cause: AbiCause::InvalidEncoding {
                message: message.to_owned(),
            },
        })
    };
    let ptr = match core_results.first() {
        Some(RuntimeVal::I32(p)) => *p as u32 as usize,
        _ => return Err(fail("missing or non-i32 result-pointer slot")),
    };
    if !ptr.is_multiple_of(alignment_of(result_ty)) {
        return Err(fail("return pointer not aligned"));
    }
    if !ctx.in_bounds(ptr, size_of(result_ty)) {
        return Err(fail("pointer out of bounds of memory"));
    }
    Ok(ptr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::concurrency::{ErrorContextAny, ErrorContextId};
    use crate::internal::StoreIdInternal;
    use crate::value::ValField;

    /// Every container shape the walk descends into, each holding
    /// `inner` once.
    fn shapes(inner: &Val) -> Vec<(&'static str, Val)> {
        let boxed = || Some(Box::new(inner.clone()));
        vec![
            ("bare", inner.clone()),
            ("list", Val::List(vec![Val::U32(1), inner.clone()].into())),
            (
                "fixed-length list",
                Val::FixedLengthList(vec![inner.clone()].into()),
            ),
            ("tuple", Val::Tuple(vec![Val::U32(1), inner.clone()].into())),
            (
                "map key",
                Val::Map(vec![(inner.clone(), Val::U32(1))].into()),
            ),
            (
                "map value",
                Val::Map(vec![(Val::U32(1), inner.clone())].into()),
            ),
            (
                "record",
                Val::Record(
                    vec![ValField {
                        name: "e".to_owned(),
                        value: inner.clone(),
                    }]
                    .into(),
                ),
            ),
            (
                "variant",
                Val::Variant {
                    discriminant: "failed".to_owned(),
                    payload: boxed(),
                },
            ),
            ("option", Val::Option(boxed())),
            ("result ok", Val::Result(Ok(boxed()))),
            ("result err", Val::Result(Err(boxed()))),
        ]
    }

    #[wcmp_macros::test]
    fn it_finds_another_stores_error_context_in_every_container() {
        let store = StoreId::fresh();
        let id = ErrorContextId::new(0, 0);
        let foreign = Val::ErrorContext(ErrorContextAny::new(StoreId::fresh(), id));
        let own = Val::ErrorContext(ErrorContextAny::new(store, id));
        for (shape, value) in shapes(&foreign) {
            let error = check_store(&value, store).expect_err(shape);
            assert!(
                matches!(&error, Error::Instantiation(inner) if matches!(**inner, InstantiationError::WrongStore)),
                "{shape}: {error:?}"
            );
        }
        for (shape, value) in shapes(&own) {
            check_store(&value, store).unwrap_or_else(|error| panic!("{shape}: {error:?}"));
        }
    }
}
