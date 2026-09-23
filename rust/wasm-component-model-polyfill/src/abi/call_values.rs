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

use wasm_runtime_layer::Val as RuntimeVal;

use super::context::BoundaryContext;
use super::flatten::{lift_from_flat_slots, lower_into_flat_slots};
use super::layout::{alignment_of, params_spill, result_spills, size_of, spill_layout};
use super::{lift, lower};
use crate::component::FunctionType;
use crate::error::{AbiCause, AbiError, AbiPosition, Error, Result};
use crate::types::{PrimitiveType, ValueType};
use crate::value::Val;

/// Lower `args` as the parameter tuple of `signature` and answer the
/// core arguments the export's core function takes.
pub fn lower_arguments<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    signature: &FunctionType,
    args: &[Val],
) -> Result<Vec<RuntimeVal>> {
    if let Some((base, offsets)) = parameter_spill(ctx, signature)? {
        // The whole parameter tuple is written into guest memory at
        // the canonical ABI's record layout, and the core function
        // receives its address.
        for (i, ((param, val), offset)) in signature
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
    for (i, (param, val)) in signature.parameters.iter().zip(args.iter()).enumerate() {
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
/// sits at address zero.
pub fn parameter_spill<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    signature: &FunctionType,
) -> Result<Option<(usize, Vec<usize>)>> {
    if !params_spill(signature) {
        return Ok(None);
    }
    let types: Vec<ValueType> = signature.parameters.iter().map(|p| p.ty.clone()).collect();
    let layout = spill_layout(&types);
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
    Ok(Some((base, layout.offsets)))
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
