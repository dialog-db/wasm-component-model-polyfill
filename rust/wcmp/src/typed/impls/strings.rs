// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A `string` as a Rust `String`.
//!
//! A string crosses as its encoded bytes. Under UTF-8 it is written
//! into the guest straight from the string, and read back into the
//! buffer that becomes the lifted string, so each direction costs the
//! host one copy of the bytes.

use wasmtime_environ::component::CanonicalAbiInfo;

use crate::abi::{lift_string, lower_str, read_pointer_pair, write_pointer_pair};
use crate::error::Result;
use crate::internal::{LiftContextInternal, LowerContextInternal};
use crate::runtime_layer::Val as CoreVal;
use crate::types::{PrimitiveType, ValueType};

use super::super::component_type::ComponentType;
use super::super::flat::take_i32;
use super::super::lift::Lift;
use super::super::lift_context::LiftContext;
use super::super::lower::Lower;
use super::super::lower_context::LowerContext;
use super::super::type_check_error::TypeCheckError;
use super::super::type_checker::TypeChecker;

impl ComponentType for String {
    const ABI: CanonicalAbiInfo = CanonicalAbiInfo::POINTER_PAIR;

    fn typecheck(ty: &ValueType, _: &TypeChecker<'_>) -> core::result::Result<(), TypeCheckError> {
        match ty {
            ValueType::Primitive(PrimitiveType::String) => Ok(()),
            _ => Err(TypeCheckError::mismatch::<Self>(ty)),
        }
    }
}

impl Lower for String {
    fn lower_flat<D: 'static>(
        &self,
        cx: &mut LowerContext<'_, '_, D>,
        ty: &ValueType,
        out: &mut Vec<CoreVal>,
    ) -> Result<()> {
        let position = cx.position();
        let (ptr, units) = lower_str(cx.boundary(), self, position, ty)?;
        out.push(CoreVal::I32(ptr as i32));
        out.push(CoreVal::I32(units as i32));
        Ok(())
    }

    fn lower_memory<D: 'static>(
        &self,
        cx: &mut LowerContext<'_, '_, D>,
        ty: &ValueType,
        offset: usize,
    ) -> Result<()> {
        let position = cx.position();
        let boundary = cx.boundary();
        let (ptr, units) = lower_str(boundary, self, position, ty)?;
        write_pointer_pair(boundary, offset, ptr, units as usize, position, ty)
    }
}

impl Lift for String {
    fn lift_flat<D: 'static>(
        cx: &mut LiftContext<'_, '_, D>,
        ty: &ValueType,
        slots: &[CoreVal],
        cursor: &mut usize,
    ) -> Result<Self> {
        let position = cx.position();
        // A slot is an `i32`, and the pointer and the length it
        // carries are unsigned.
        let ptr = take_i32(slots, cursor, ty, position)? as u32 as usize;
        let units = take_i32(slots, cursor, ty, position)? as u32 as usize;
        lift_string(cx.boundary(), ptr, units, position, ty)
    }

    fn lift_memory<D: 'static>(
        cx: &mut LiftContext<'_, '_, D>,
        ty: &ValueType,
        offset: usize,
    ) -> Result<Self> {
        let position = cx.position();
        let boundary = cx.boundary();
        let (ptr, units) = read_pointer_pair(boundary, offset, position, ty)?;
        lift_string(boundary, ptr, units, position, ty)
    }
}
