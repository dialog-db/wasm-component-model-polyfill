// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A `list<T>` as a Rust vector, and a `list<T, N>` as a Rust array.
//!
//! A vector crosses through its element type's list methods: a list of
//! numbers as the list's bytes, and a list of anything else element by
//! element, out of one read of the list's range and into one write.

use wasmtime_environ::component::CanonicalAbiInfo;

use crate::abi::{read_pointer_pair, write_pointer_pair};
use crate::error::{Result, TypePathStep};
use crate::internal::{LiftContextInternal, LowerContextInternal};
use crate::runtime_layer::Val as CoreVal;
use crate::store::StoreId;
use crate::types::ValueType;

use super::super::component_type::ComponentType;
use super::super::flat::{declared_mismatch, take_i32};
use super::super::lift::Lift;
use super::super::lift_context::LiftContext;
use super::super::lower::Lower;
use super::super::lower_context::LowerContext;
use super::super::type_check_error::TypeCheckError;
use super::super::type_checker::TypeChecker;

impl<T: ComponentType> ComponentType for Vec<T> {
    const ABI: CanonicalAbiInfo = CanonicalAbiInfo::POINTER_PAIR;

    fn typecheck(
        ty: &ValueType,
        types: &TypeChecker<'_>,
    ) -> core::result::Result<(), TypeCheckError> {
        match ty {
            ValueType::List(list) => T::typecheck(list.element(), types)
                .map_err(|error| error.at(TypePathStep::ListElement)),
            _ => Err(TypeCheckError::mismatch::<Self>(ty)),
        }
    }
}

impl<T: Lower> Lower for Vec<T> {
    fn lower_flat<D: 'static>(
        &self,
        cx: &mut LowerContext<'_, '_, D>,
        ty: &ValueType,
        out: &mut Vec<CoreVal>,
    ) -> Result<()> {
        let ptr = T::lower_list(self, cx, ty)?;
        out.push(CoreVal::I32(ptr as i32));
        out.push(CoreVal::I32(self.len() as i32));
        Ok(())
    }

    fn lower_memory<D: 'static>(
        &self,
        cx: &mut LowerContext<'_, '_, D>,
        ty: &ValueType,
        offset: usize,
    ) -> Result<()> {
        let ptr = T::lower_list(self, cx, ty)?;
        let position = cx.position();
        write_pointer_pair(cx.boundary(), offset, ptr, self.len(), position, ty)
    }

    fn check_store(&self, store: StoreId) -> Result<()> {
        self.iter().try_for_each(|item| item.check_store(store))
    }
}

impl<T: Lift> Lift for Vec<T> {
    fn lift_flat<D: 'static>(
        cx: &mut LiftContext<'_, '_, D>,
        ty: &ValueType,
        slots: &[CoreVal],
        cursor: &mut usize,
    ) -> Result<Self> {
        let position = cx.position();
        let ptr = take_i32(slots, cursor, ty, position)? as u32 as usize;
        let len = take_i32(slots, cursor, ty, position)? as u32 as usize;
        T::lift_list(cx, ty, ptr, len)
    }

    fn lift_memory<D: 'static>(
        cx: &mut LiftContext<'_, '_, D>,
        ty: &ValueType,
        offset: usize,
    ) -> Result<Self> {
        let position = cx.position();
        let (ptr, len) = read_pointer_pair(cx.boundary(), offset, position, ty)?;
        T::lift_list(cx, ty, ptr, len)
    }
}

impl<T: ComponentType, const N: usize> ComponentType for [T; N] {
    const ABI: CanonicalAbiInfo = CanonicalAbiInfo::fixed_length_list_static(&T::ABI, N);

    fn typecheck(
        ty: &ValueType,
        types: &TypeChecker<'_>,
    ) -> core::result::Result<(), TypeCheckError> {
        match ty {
            ValueType::FixedLengthList(list) if list.length() as usize == N => {
                T::typecheck(list.element(), types)
                    .map_err(|error| error.at(TypePathStep::ListElement))
            }
            _ => Err(TypeCheckError::mismatch::<Self>(ty)),
        }
    }
}

/// The element type of the fixed-length list `ty` declares.
fn fixed_element(ty: &ValueType) -> Option<&ValueType> {
    match ty {
        ValueType::FixedLengthList(list) => Some(list.element()),
        _ => None,
    }
}

impl<T: Lower, const N: usize> Lower for [T; N] {
    fn lower_flat<D: 'static>(
        &self,
        cx: &mut LowerContext<'_, '_, D>,
        ty: &ValueType,
        out: &mut Vec<CoreVal>,
    ) -> Result<()> {
        let element = fixed_element(ty).ok_or_else(|| declared_mismatch(ty, cx.position()))?;
        self.iter()
            .try_for_each(|item| item.lower_flat(cx, element, out))
    }

    fn lower_memory<D: 'static>(
        &self,
        cx: &mut LowerContext<'_, '_, D>,
        ty: &ValueType,
        offset: usize,
    ) -> Result<()> {
        let element = fixed_element(ty).ok_or_else(|| declared_mismatch(ty, cx.position()))?;
        for (index, item) in self.iter().enumerate() {
            item.lower_memory(cx, element, offset + index * T::SIZE32)?;
        }
        Ok(())
    }

    fn check_store(&self, store: StoreId) -> Result<()> {
        self.iter().try_for_each(|item| item.check_store(store))
    }
}

impl<T: Lift, const N: usize> Lift for [T; N] {
    fn lift_flat<D: 'static>(
        cx: &mut LiftContext<'_, '_, D>,
        ty: &ValueType,
        slots: &[CoreVal],
        cursor: &mut usize,
    ) -> Result<Self> {
        let position = cx.position();
        let element = fixed_element(ty).ok_or_else(|| declared_mismatch(ty, position))?;
        cx.boundary()
            .charge_copy_budget(N, core::mem::size_of::<T>(), position, ty)?;
        let mut items = Vec::with_capacity(N);
        for _ in 0..N {
            items.push(T::lift_flat(cx, element, slots, cursor)?);
        }
        items
            .try_into()
            .map_err(|_| declared_mismatch(ty, position))
    }

    fn lift_memory<D: 'static>(
        cx: &mut LiftContext<'_, '_, D>,
        ty: &ValueType,
        offset: usize,
    ) -> Result<Self> {
        let position = cx.position();
        let element = fixed_element(ty).ok_or_else(|| declared_mismatch(ty, position))?;
        cx.boundary()
            .charge_copy_budget(N, core::mem::size_of::<T>(), position, ty)?;
        let mut items = Vec::with_capacity(N);
        for index in 0..N {
            items.push(T::lift_memory(cx, element, offset + index * T::SIZE32)?);
        }
        items
            .try_into()
            .map_err(|_| declared_mismatch(ty, position))
    }
}
