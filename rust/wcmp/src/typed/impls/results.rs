// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A `result<T, E>` as a Rust `Result<T, E>`.
//!
//! The unit type `()` stands for a case with no payload, as Wasmtime's
//! `IS_RUST_UNIT_TYPE` makes it: `Result<(), E>` is `result<_, E>`, and
//! `Result<(), ()>` is `result`.

use wasmtime_environ::component::CanonicalAbiInfo;

use crate::error::{Result, TypePathStep};
use crate::internal::LiftContextInternal;
use crate::runtime_layer::Val as CoreVal;
use crate::store::StoreId;
use crate::types::ValueType;

use super::super::component_type::ComponentType;
use super::super::flat::{
    declared_mismatch, lift_case_flat, lift_case_memory, lower_case_flat, lower_case_memory,
};
use super::super::lift::Lift;
use super::super::lift_context::LiftContext;
use super::super::lower::Lower;
use super::super::lower_context::LowerContext;
use super::super::type_check_error::TypeCheckError;
use super::super::type_checker::TypeChecker;

/// The layout of a case whose payload is a `T`: none for the unit
/// type.
const fn payload<T: ComponentType>() -> Option<CanonicalAbiInfo> {
    if T::IS_RUST_UNIT_TYPE {
        None
    } else {
        Some(T::ABI)
    }
}

/// Compare the payload `T` with the declared payload of one case of a
/// `result`, which `step` names: the unit type matches a case with no
/// payload, and any other type matches the payload's type.
fn typecheck_payload<T: ComponentType>(
    declared: Option<&ValueType>,
    result: &ValueType,
    types: &TypeChecker<'_>,
    step: TypePathStep,
) -> core::result::Result<(), TypeCheckError> {
    match (declared, T::IS_RUST_UNIT_TYPE) {
        (None, true) => Ok(()),
        (Some(declared), false) => T::typecheck(declared, types).map_err(|error| error.at(step)),
        (Some(declared), true) => Err(TypeCheckError::mismatch::<T>(declared).at(step)),
        (None, false) => Err(TypeCheckError::mismatch::<T>(result).at(step)),
    }
}

impl<T: ComponentType, E: ComponentType> ComponentType for core::result::Result<T, E> {
    const ABI: CanonicalAbiInfo =
        CanonicalAbiInfo::variant_static(&[payload::<T>(), payload::<E>()]);

    fn typecheck(
        ty: &ValueType,
        types: &TypeChecker<'_>,
    ) -> core::result::Result<(), TypeCheckError> {
        match ty {
            ValueType::Result(result) => {
                typecheck_payload::<T>(result.ok(), ty, types, TypePathStep::ResultOk)?;
                typecheck_payload::<E>(result.err(), ty, types, TypePathStep::ResultErr)
            }
            _ => Err(TypeCheckError::mismatch::<Self>(ty)),
        }
    }
}

impl<T: Lower, E: Lower> Lower for core::result::Result<T, E> {
    fn lower_flat<D: 'static>(
        &self,
        cx: &mut LowerContext<'_, '_, D>,
        ty: &ValueType,
        out: &mut Vec<CoreVal>,
    ) -> Result<()> {
        match self {
            Ok(value) => lower_case_flat(cx, ty, 0, out, |cx, payload, out| {
                value.lower_flat(cx, payload, out)
            }),
            Err(value) => lower_case_flat(cx, ty, 1, out, |cx, payload, out| {
                value.lower_flat(cx, payload, out)
            }),
        }
    }

    fn lower_memory<D: 'static>(
        &self,
        cx: &mut LowerContext<'_, '_, D>,
        ty: &ValueType,
        offset: usize,
    ) -> Result<()> {
        match self {
            Ok(value) => {
                let (payload, payload_ty) = lower_case_memory(cx, ty, 0, offset)?;
                payload_ty.map_or(Ok(()), |payload_ty| {
                    value.lower_memory(cx, payload_ty, payload)
                })
            }
            Err(value) => {
                let (payload, payload_ty) = lower_case_memory(cx, ty, 1, offset)?;
                payload_ty.map_or(Ok(()), |payload_ty| {
                    value.lower_memory(cx, payload_ty, payload)
                })
            }
        }
    }

    fn check_store(&self, store: StoreId) -> Result<()> {
        match self {
            Ok(value) => value.check_store(store),
            Err(value) => value.check_store(store),
        }
    }
}

/// Lift the payload `T` of one case from its flat values, or, for a
/// case with no payload, the unit value `T` is.
fn lift_payload_flat<D: 'static, T: Lift>(
    cx: &mut LiftContext<'_, '_, D>,
    payload: Option<(&ValueType, &[CoreVal])>,
) -> Result<T> {
    let mut cursor = 0;
    match payload {
        Some((ty, slots)) => T::lift_flat(cx, ty, slots, &mut cursor),
        None => T::lift_flat(cx, UNIT, &[], &mut cursor),
    }
}

/// Lift the payload `T` of one case from memory, or, for a case with no
/// payload, the unit value `T` is.
fn lift_payload_memory<D: 'static, T: Lift>(
    cx: &mut LiftContext<'_, '_, D>,
    payload_ty: Option<&ValueType>,
    offset: usize,
) -> Result<T> {
    match payload_ty {
        Some(ty) => T::lift_memory(cx, ty, offset),
        None => T::lift_memory(cx, UNIT, offset),
    }
}

/// The type the unit value of a case with no payload lifts against:
/// `()` crosses as nothing whatever the type says.
const UNIT: &ValueType = &ValueType::ErrorContext;

impl<T: Lift, E: Lift> Lift for core::result::Result<T, E> {
    fn lift_flat<D: 'static>(
        cx: &mut LiftContext<'_, '_, D>,
        ty: &ValueType,
        slots: &[CoreVal],
        cursor: &mut usize,
    ) -> Result<Self> {
        lift_case_flat(cx, ty, slots, cursor, |cx, case, payload| match case {
            0 => lift_payload_flat(cx, payload).map(Ok),
            1 => lift_payload_flat(cx, payload).map(Err),
            _ => Err(declared_mismatch(ty, cx.position())),
        })
    }

    fn lift_memory<D: 'static>(
        cx: &mut LiftContext<'_, '_, D>,
        ty: &ValueType,
        offset: usize,
    ) -> Result<Self> {
        match lift_case_memory(cx, ty, offset)? {
            (0, payload, payload_ty) => lift_payload_memory(cx, payload_ty, payload).map(Ok),
            (1, payload, payload_ty) => lift_payload_memory(cx, payload_ty, payload).map(Err),
            _ => Err(declared_mismatch(ty, cx.position())),
        }
    }
}
