// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The resource handles: [`Resource<T>`] and [`ResourceDynamic`], a
//! host's typed handles, and [`ResourceAny`], the handle of any
//! resource type.
//!
//! Each crosses as the `u32` index of a handle table's entry, or as
//! the rep itself where the instance receiving a borrow defines the
//! resource. The type check compares resource types by identity, as
//! Wasmtime's does: a typed handle crosses where the component
//! declares an `own` or a `borrow` of its own identity, and a
//! [`ResourceAny`] wherever the component declares a handle, its
//! lowering checking the identity instead.

use wasmtime_environ::component::CanonicalAbiInfo;

use crate::abi::{lift_host_resource, lift_resource_any, lower_host_resource, lower_resource_any};
use crate::error::Result;
use crate::internal::{
    LiftContextInternal, LowerContextInternal, ResourceInternal, ResourceTypeInternal,
};
use crate::resource::{Resource, ResourceAny, ResourceDynamic};
use crate::runtime_layer::Val as CoreVal;
use crate::types::{ResourceType, ValueType};

use super::super::component_type::ComponentType;
use super::super::flat::take_i32;
use super::super::lift::Lift;
use super::super::lift_context::LiftContext;
use super::super::lower::Lower;
use super::super::lower_context::LowerContext;
use super::super::type_check_error::TypeCheckError;
use super::super::type_checker::TypeChecker;

/// Read the `u32` index of a handle out of guest memory at `offset`.
fn index_memory<D: 'static>(
    cx: &mut LiftContext<'_, '_, D>,
    ty: &ValueType,
    offset: usize,
) -> Result<u32> {
    let position = cx.position();
    Ok(u32::from_le_bytes(
        cx.boundary().read_array(offset, position, ty)?,
    ))
}

/// Write the `u32` index of a handle into guest memory at `offset`.
fn write_index<D: 'static>(
    cx: &mut LowerContext<'_, '_, D>,
    ty: &ValueType,
    offset: usize,
    index: u32,
) -> Result<()> {
    let position = cx.position();
    cx.boundary()
        .write_bytes(offset, &index.to_le_bytes(), position, ty)
}

/// The resource type a handle type declares, behind the identity the
/// instantiation gave it.
fn declared(ty: &ValueType, types: &TypeChecker<'_>) -> Option<ResourceType> {
    match ty {
        ValueType::Own(resource) | ValueType::Borrow(resource) => Some(types.resource(*resource)),
        _ => None,
    }
}

/// A typed host handle: what identity it crosses as, given the
/// identity the component declares.
trait HostResource: ResourceInternal {
    /// Whether a handle of this Rust type crosses as `identity`.
    fn admits(identity: ResourceType) -> bool;

    /// The identity this handle crosses as.
    fn identity(&self) -> ResourceType;
}

impl<T: 'static> HostResource for Resource<T> {
    fn admits(identity: ResourceType) -> bool {
        identity.is_host::<T>()
    }

    fn identity(&self) -> ResourceType {
        ResourceType::host::<T>()
    }
}

impl HostResource for ResourceDynamic {
    fn admits(identity: ResourceType) -> bool {
        identity.as_host_dynamic().is_some()
    }

    fn identity(&self) -> ResourceType {
        ResourceType::host_dynamic(self.ty())
    }
}

/// The traits of [`Resource<T>`] and of [`ResourceDynamic`], which
/// differ only in the identities they admit.
macro_rules! host_resource {
    ($handle:ty $(, $param:ident)?) => {
        impl$(<$param: 'static>)? ComponentType for $handle {
            const ABI: CanonicalAbiInfo = CanonicalAbiInfo::SCALAR4;

            fn typecheck(
                ty: &ValueType,
                types: &TypeChecker<'_>,
            ) -> core::result::Result<(), TypeCheckError> {
                match declared(ty, types) {
                    Some(identity) if <Self as HostResource>::admits(identity) => Ok(()),
                    _ => Err(TypeCheckError::mismatch::<Self>(ty)),
                }
            }
        }

        impl$(<$param: 'static>)? Lower for $handle {
            fn lower_flat<D: 'static>(
                &self,
                cx: &mut LowerContext<'_, '_, D>,
                ty: &ValueType,
                out: &mut Vec<CoreVal>,
            ) -> Result<()> {
                let position = cx.position();
                let index =
                    lower_host_resource(cx.boundary(), self.handle(), self.identity(), ty, position)?;
                out.push(CoreVal::I32(index as i32));
                Ok(())
            }

            fn lower_memory<D: 'static>(
                &self,
                cx: &mut LowerContext<'_, '_, D>,
                ty: &ValueType,
                offset: usize,
            ) -> Result<()> {
                let position = cx.position();
                let index =
                    lower_host_resource(cx.boundary(), self.handle(), self.identity(), ty, position)?;
                write_index(cx, ty, offset, index)
            }
        }

        impl$(<$param: 'static>)? Lift for $handle {
            fn lift_flat<D: 'static>(
                cx: &mut LiftContext<'_, '_, D>,
                ty: &ValueType,
                slots: &[CoreVal],
                cursor: &mut usize,
            ) -> Result<Self> {
                let position = cx.position();
                let index = take_i32(slots, cursor, ty, position)? as u32;
                let (handle, identity) = lift_host_resource(cx.boundary(), index, ty, position)?;
                Ok(Self::from_lift(handle, identity))
            }

            fn lift_memory<D: 'static>(
                cx: &mut LiftContext<'_, '_, D>,
                ty: &ValueType,
                offset: usize,
            ) -> Result<Self> {
                let position = cx.position();
                let index = index_memory(cx, ty, offset)?;
                let (handle, identity) = lift_host_resource(cx.boundary(), index, ty, position)?;
                Ok(Self::from_lift(handle, identity))
            }
        }
    };
}

host_resource!(Resource<T>, T);
host_resource!(ResourceDynamic);

impl ComponentType for ResourceAny {
    const ABI: CanonicalAbiInfo = CanonicalAbiInfo::SCALAR4;

    fn typecheck(ty: &ValueType, _: &TypeChecker<'_>) -> core::result::Result<(), TypeCheckError> {
        match ty {
            ValueType::Own(_) | ValueType::Borrow(_) => Ok(()),
            _ => Err(TypeCheckError::mismatch::<Self>(ty)),
        }
    }
}

impl Lower for ResourceAny {
    fn lower_flat<D: 'static>(
        &self,
        cx: &mut LowerContext<'_, '_, D>,
        ty: &ValueType,
        out: &mut Vec<CoreVal>,
    ) -> Result<()> {
        let position = cx.position();
        let index = lower_resource_any(cx.boundary(), self, ty, position)?;
        out.push(CoreVal::I32(index as i32));
        Ok(())
    }

    fn lower_memory<D: 'static>(
        &self,
        cx: &mut LowerContext<'_, '_, D>,
        ty: &ValueType,
        offset: usize,
    ) -> Result<()> {
        let position = cx.position();
        let index = lower_resource_any(cx.boundary(), self, ty, position)?;
        write_index(cx, ty, offset, index)
    }
}

impl Lift for ResourceAny {
    fn lift_flat<D: 'static>(
        cx: &mut LiftContext<'_, '_, D>,
        ty: &ValueType,
        slots: &[CoreVal],
        cursor: &mut usize,
    ) -> Result<Self> {
        let position = cx.position();
        let index = take_i32(slots, cursor, ty, position)? as u32;
        lift_resource_any(cx.boundary(), index, ty, position)
    }

    fn lift_memory<D: 'static>(
        cx: &mut LiftContext<'_, '_, D>,
        ty: &ValueType,
        offset: usize,
    ) -> Result<Self> {
        let position = cx.position();
        let index = index_memory(cx, ty, offset)?;
        lift_resource_any(cx.boundary(), index, ty, position)
    }
}
