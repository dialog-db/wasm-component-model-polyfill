// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! An `option<T>` as a Rust `Option<T>`.

use wasmtime_environ::component::CanonicalAbiInfo;

use crate::error::{Result, TypePathStep};
use crate::internal::{LiftContextInternal, LowerContextInternal};
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

impl<T: ComponentType> ComponentType for Option<T> {
    const ABI: CanonicalAbiInfo = CanonicalAbiInfo::variant_static(&[None, Some(T::ABI)]);

    fn typecheck(
        ty: &ValueType,
        types: &TypeChecker<'_>,
    ) -> core::result::Result<(), TypeCheckError> {
        match ty {
            ValueType::Option(option) => T::typecheck(option.payload(), types)
                .map_err(|error| error.at(TypePathStep::OptionPayload)),
            _ => Err(TypeCheckError::mismatch::<Self>(ty)),
        }
    }
}

impl<T: Lower> Lower for Option<T> {
    fn lower_flat<D: 'static>(
        &self,
        cx: &mut LowerContext<'_, '_, D>,
        ty: &ValueType,
        out: &mut Vec<CoreVal>,
    ) -> Result<()> {
        match self {
            None => lower_case_flat(cx, ty, 0, out, |_, _, _| Ok(())),
            Some(value) => lower_case_flat(cx, ty, 1, out, |cx, payload, out| {
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
            None => lower_case_memory(cx, ty, 0, offset).map(|_| ()),
            Some(value) => {
                let (payload, payload_ty) = lower_case_memory(cx, ty, 1, offset)?;
                let payload_ty = payload_ty.ok_or_else(|| declared_mismatch(ty, cx.position()))?;
                value.lower_memory(cx, payload_ty, payload)
            }
        }
    }

    fn check_store(&self, store: StoreId) -> Result<()> {
        self.as_ref()
            .map_or(Ok(()), |value| value.check_store(store))
    }
}

impl<T: Lift> Lift for Option<T> {
    fn lift_flat<D: 'static>(
        cx: &mut LiftContext<'_, '_, D>,
        ty: &ValueType,
        slots: &[CoreVal],
        cursor: &mut usize,
    ) -> Result<Self> {
        lift_case_flat(cx, ty, slots, cursor, |cx, case, payload| {
            match (case, payload) {
                (0, None) => Ok(None),
                (1, Some((payload_ty, payload))) => {
                    let mut cursor = 0;
                    T::lift_flat(cx, payload_ty, payload, &mut cursor).map(Some)
                }
                _ => Err(declared_mismatch(ty, cx.position())),
            }
        })
    }

    fn lift_memory<D: 'static>(
        cx: &mut LiftContext<'_, '_, D>,
        ty: &ValueType,
        offset: usize,
    ) -> Result<Self> {
        match lift_case_memory(cx, ty, offset)? {
            (0, _, None) => Ok(None),
            (1, payload, Some(payload_ty)) => T::lift_memory(cx, payload_ty, payload).map(Some),
            _ => Err(declared_mismatch(ty, cx.position())),
        }
    }
}
