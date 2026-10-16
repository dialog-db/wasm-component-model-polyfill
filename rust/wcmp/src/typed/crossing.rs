// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Where a typed call starts: the parameter list and the result list
//! of a function, crossing as a whole, and the type check of a
//! function.

use crate::abi::call_values::{parameter_spill, result_pointer};
use crate::abi::context::BoundaryContext;
use crate::abi::flatten::take_i32;
use crate::abi::layout::result_spills;
use crate::abi::signature::Signature;
use crate::component::FunctionType;
use crate::error::{AbiCause, AbiError, AbiPosition, Error, Result};
use crate::internal::{LiftContextInternal, LowerContextInternal, TypeCheckerInternal};
use crate::runtime_layer::Val as CoreVal;
use crate::types::{PrimitiveType, ResourceType, ValueType};

use super::component_named_list::ComponentNamedList;
use super::lift::Lift;
use super::lift_context::LiftContext;
use super::lower::Lower;
use super::lower_context::LowerContext;
use super::named_types::NamedTypes;
use super::type_check_error::TypeCheckError;
use super::type_checker::TypeChecker;

/// The type a list crosses against when it is a function's list: the
/// tuple that is the list takes its members' types from the function
/// instead, so this type is never read.
const LIST: &ValueType = &ValueType::ErrorContext;

/// Compare the parameter list `P` and the result list `R` with the
/// function type `ty` a component declares, reading each resource type
/// the component declares as the identity `resources` gives it. A
/// mismatch names the parameter or the result it is in.
pub fn typecheck_function<P: ComponentNamedList, R: ComponentNamedList>(
    ty: &FunctionType,
    resources: &dyn Fn(ResourceType) -> ResourceType,
) -> core::result::Result<(), TypeCheckError> {
    let checker = TypeChecker::new(resources);
    checker.set_named(NamedTypes::parameters(ty));
    P::typecheck(LIST, &checker)?;
    checker.set_named(NamedTypes::results(ty));
    R::typecheck(LIST, &checker)
}

/// Lower `params` as the parameter list of `signature`, for a call
/// into an export, and answer the core arguments of the call: the flat
/// values, or the address of the spilled list.
pub fn lower_parameters<D: 'static, P: Lower>(
    cx: &mut BoundaryContext<'_, D>,
    signature: &Signature,
    params: &P,
) -> Result<Vec<CoreVal>> {
    let named = NamedTypes::parameters(signature.ty());
    if let Some((base, _)) = parameter_spill(cx, signature)? {
        // The whole list is written into guest memory at the record
        // layout of the parameter tuple, and the core function
        // receives its address.
        params.lower_memory(&mut LowerContext::named(cx, named), LIST, base)?;
        return Ok(vec![CoreVal::I32(base as i32)]);
    }
    let mut out = Vec::new();
    params.lower_flat(&mut LowerContext::named(cx, named), LIST, &mut out)?;
    Ok(out)
}

/// Lift the result list of `ty` out of the core results an export
/// returned.
pub fn lift_results<D: 'static, R: Lift>(
    cx: &mut BoundaryContext<'_, D>,
    core_results: &[CoreVal],
    ty: &FunctionType,
) -> Result<R> {
    let named = NamedTypes::results(ty);
    match &ty.result {
        Some(result_ty) if result_spills(ty) => {
            let ptr = result_pointer(cx, core_results, result_ty)?;
            R::lift_memory(&mut LiftContext::named(cx, named), LIST, ptr)
        }
        _ => {
            let mut cursor = 0;
            R::lift_flat(
                &mut LiftContext::named(cx, named),
                LIST,
                core_results,
                &mut cursor,
            )
        }
    }
}

/// Lift the result list of `ty` out of the arguments of `task.return`:
/// the flat values, or, when `spilled`, the address of the result in
/// guest memory.
pub fn lift_returned<D: 'static, R: Lift>(
    cx: &mut BoundaryContext<'_, D>,
    args: &[CoreVal],
    ty: &FunctionType,
    spilled: bool,
) -> Result<R> {
    let named = NamedTypes::results(ty);
    let mut cursor = 0;
    if spilled {
        let ptr = pointer(args, &mut cursor, AbiPosition::Result)?;
        return R::lift_memory(&mut LiftContext::named(cx, named), LIST, ptr);
    }
    R::lift_flat(&mut LiftContext::named(cx, named), LIST, args, &mut cursor)
}

/// Lift the parameter list of `ty` out of the core arguments of a call
/// into the host, from `cursor`: the flat values, or, when `spilled`,
/// the address of the list in guest memory.
pub fn lift_parameters<D: 'static, P: Lift>(
    cx: &mut BoundaryContext<'_, D>,
    ty: &FunctionType,
    args: &[CoreVal],
    cursor: &mut usize,
    spilled: bool,
) -> Result<P> {
    let named = NamedTypes::parameters(ty);
    if spilled {
        let ptr = pointer(args, cursor, AbiPosition::Argument(0))?;
        return P::lift_memory(&mut LiftContext::named(cx, named), LIST, ptr);
    }
    P::lift_flat(&mut LiftContext::named(cx, named), LIST, args, cursor)
}

/// Lower `results` as the result list of `ty`, for a call into the
/// host that returns: into the return area the guest passed, or into
/// the flat result values of the call.
pub fn lower_results<D: 'static, R: Lower>(
    cx: &mut BoundaryContext<'_, D>,
    ty: &FunctionType,
    results: &R,
    return_area: Option<usize>,
    flat: &mut [CoreVal],
) -> Result<()> {
    let named = NamedTypes::results(ty);
    if let Some(ptr) = return_area {
        return results.lower_memory(&mut LowerContext::named(cx, named), LIST, ptr);
    }
    let mut slots = Vec::with_capacity(flat.len());
    results.lower_flat(&mut LowerContext::named(cx, named), LIST, &mut slots)?;
    if slots.len() != flat.len() {
        return Err(Error::from(AbiError {
            position: AbiPosition::Result,
            valtype: ty.result.clone(),
            cause: AbiCause::InvalidEncoding {
                message: format!(
                    "lowered {} flat result slots for a core signature with {}",
                    slots.len(),
                    flat.len()
                ),
            },
        }));
    }
    flat.copy_from_slice(&slots);
    Ok(())
}

/// Take the address argument at `cursor`, which a list that does not
/// fit in flat values travels as.
fn pointer(args: &[CoreVal], cursor: &mut usize, position: AbiPosition) -> Result<usize> {
    const ADDRESS: &ValueType = &ValueType::Primitive(PrimitiveType::U32);
    Ok(take_i32(args, cursor, ADDRESS, position)? as u32 as usize)
}
