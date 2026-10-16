// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The other Rust forms of a `string` and a `list<T>`, and the types
//! that forward to the type they hold, as Wasmtime gives them.
//!
//! `str` and `[T]` lower only, because nothing owns what a lift would
//! answer. `Box<str>` and `Arc<str>` cross as a `String` does, and
//! `Box<[T]>` and `Arc<[T]>` as a `Vec<T>` does. `&T`, `Box<T>`, and
//! `Arc<T>` lower as the `T` they hold.

use std::sync::Arc;

use wasmtime_environ::component::CanonicalAbiInfo;

use crate::abi::{lower_str, write_pointer_pair};
use crate::error::{Result, TypePathStep};
use crate::internal::LowerContextInternal;
use crate::runtime_layer::Val as CoreVal;
use crate::store::StoreId;
use crate::types::{PrimitiveType, ValueType};

use super::super::component_type::ComponentType;
use super::super::lift::Lift;
use super::super::lift_context::LiftContext;
use super::super::lower::Lower;
use super::super::lower_context::LowerContext;
use super::super::type_check_error::TypeCheckError;
use super::super::type_checker::TypeChecker;

impl ComponentType for str {
    const ABI: CanonicalAbiInfo = CanonicalAbiInfo::POINTER_PAIR;

    fn typecheck(ty: &ValueType, _: &TypeChecker<'_>) -> core::result::Result<(), TypeCheckError> {
        match ty {
            ValueType::Primitive(PrimitiveType::String) => Ok(()),
            _ => Err(TypeCheckError::mismatch::<Self>(ty)),
        }
    }
}

impl Lower for str {
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

impl<T: ComponentType> ComponentType for [T] {
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

impl<T: Lower> Lower for [T] {
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

/// A pointer type that crosses as the value it points to, as
/// Wasmtime's forwarding impls do: its type and its lowering are the
/// pointee's.
macro_rules! forward {
    ($($pointer:ident),+) => {$(
        impl<T: ComponentType + ?Sized> ComponentType for $pointer!(T) {
            const ABI: CanonicalAbiInfo = T::ABI;
            const IS_RUST_UNIT_TYPE: bool = T::IS_RUST_UNIT_TYPE;

            fn typecheck(
                ty: &ValueType,
                types: &TypeChecker<'_>,
            ) -> core::result::Result<(), TypeCheckError> {
                T::typecheck(ty, types)
            }
        }

        impl<T: Lower + ?Sized> Lower for $pointer!(T) {
            fn lower_flat<D: 'static>(
                &self,
                cx: &mut LowerContext<'_, '_, D>,
                ty: &ValueType,
                out: &mut Vec<CoreVal>,
            ) -> Result<()> {
                (**self).lower_flat(cx, ty, out)
            }

            fn lower_memory<D: 'static>(
                &self,
                cx: &mut LowerContext<'_, '_, D>,
                ty: &ValueType,
                offset: usize,
            ) -> Result<()> {
                (**self).lower_memory(cx, ty, offset)
            }

            fn check_store(&self, store: StoreId) -> Result<()> {
                (**self).check_store(store)
            }
        }
    )+};
}

macro_rules! reference {
    ($t:ty) => {
        &$t
    };
}
macro_rules! boxed {
    ($t:ty) => { Box<$t> };
}
macro_rules! shared {
    ($t:ty) => { Arc<$t> };
}

forward!(reference, boxed, shared);

impl Lift for Box<str> {
    fn lift_flat<D: 'static>(
        cx: &mut LiftContext<'_, '_, D>,
        ty: &ValueType,
        slots: &[CoreVal],
        cursor: &mut usize,
    ) -> Result<Self> {
        String::lift_flat(cx, ty, slots, cursor).map(String::into_boxed_str)
    }

    fn lift_memory<D: 'static>(
        cx: &mut LiftContext<'_, '_, D>,
        ty: &ValueType,
        offset: usize,
    ) -> Result<Self> {
        String::lift_memory(cx, ty, offset).map(String::into_boxed_str)
    }
}

impl Lift for Arc<str> {
    fn lift_flat<D: 'static>(
        cx: &mut LiftContext<'_, '_, D>,
        ty: &ValueType,
        slots: &[CoreVal],
        cursor: &mut usize,
    ) -> Result<Self> {
        String::lift_flat(cx, ty, slots, cursor).map(Arc::from)
    }

    fn lift_memory<D: 'static>(
        cx: &mut LiftContext<'_, '_, D>,
        ty: &ValueType,
        offset: usize,
    ) -> Result<Self> {
        String::lift_memory(cx, ty, offset).map(Arc::from)
    }
}

impl<T: Lift> Lift for Box<[T]> {
    fn lift_flat<D: 'static>(
        cx: &mut LiftContext<'_, '_, D>,
        ty: &ValueType,
        slots: &[CoreVal],
        cursor: &mut usize,
    ) -> Result<Self> {
        Vec::<T>::lift_flat(cx, ty, slots, cursor).map(Vec::into_boxed_slice)
    }

    fn lift_memory<D: 'static>(
        cx: &mut LiftContext<'_, '_, D>,
        ty: &ValueType,
        offset: usize,
    ) -> Result<Self> {
        Vec::<T>::lift_memory(cx, ty, offset).map(Vec::into_boxed_slice)
    }
}

impl<T: Lift> Lift for Arc<[T]> {
    fn lift_flat<D: 'static>(
        cx: &mut LiftContext<'_, '_, D>,
        ty: &ValueType,
        slots: &[CoreVal],
        cursor: &mut usize,
    ) -> Result<Self> {
        Vec::<T>::lift_flat(cx, ty, slots, cursor).map(Arc::from)
    }

    fn lift_memory<D: 'static>(
        cx: &mut LiftContext<'_, '_, D>,
        ty: &ValueType,
        offset: usize,
    ) -> Result<Self> {
        Vec::<T>::lift_memory(cx, ty, offset).map(Arc::from)
    }
}
