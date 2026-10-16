// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Rust tuples of 0 to 17 values: a WIT `tuple`, and the parameter list
//! or the result list of a function.
//!
//! A tuple is laid out as a record of its members, so the same code
//! crosses it in both roles. As a list it crosses member by member
//! against the types the function declares, and a member's failure
//! names the parameter or the result it is. Inside another value it
//! crosses against the members of the `tuple` type declared there.

use wasmtime_environ::component::CanonicalAbiInfo;

use crate::abi::layout::align_to;
use crate::error::{AbiPosition, Result, TypePathStep};
use crate::internal::{LiftContextInternal, LowerContextInternal, TypeCheckerInternal};
use crate::runtime_layer::Val as CoreVal;
use crate::store::StoreId;
use crate::types::ValueType;

use super::super::component_named_list::ComponentNamedList;
use super::super::component_type::ComponentType;
use super::super::flat::declared_mismatch;
use super::super::lift::Lift;
use super::super::lift_context::LiftContext;
use super::super::lower::Lower;
use super::super::lower_context::LowerContext;
use super::super::named_types::NamedTypes;
use super::super::type_check_error::TypeCheckError;
use super::super::type_checker::TypeChecker;

/// The declared types of a tuple's members: the function's list, when
/// the tuple is one, or the members of the declared `tuple`.
enum Members<'t> {
    Named(NamedTypes<'t>),
    Tuple(&'t [ValueType]),
}

impl<'t> Members<'t> {
    /// The members a tuple of `count` crosses against, in a crossing
    /// that reports at `position`.
    fn of(
        named: Option<NamedTypes<'t>>,
        ty: &'t ValueType,
        count: usize,
        position: AbiPosition,
    ) -> Result<Self> {
        let members = match (named, ty) {
            (Some(named), _) => Self::Named(named),
            (None, ValueType::Tuple(tuple)) => Self::Tuple(tuple.elements()),
            (None, _) => return Err(declared_mismatch(ty, position)),
        };
        if members.len() != count {
            return Err(declared_mismatch(ty, position));
        }
        Ok(members)
    }

    fn len(&self) -> usize {
        match self {
            Self::Named(named) => named.count(),
            Self::Tuple(elements) => elements.len(),
        }
    }

    /// The declared type of member `index`.
    fn get(&self, index: usize) -> &'t ValueType {
        match self {
            Self::Named(named) => named
                .get(index)
                .expect("a list's length was checked against the tuple's"),
            Self::Tuple(elements) => &elements[index],
        }
    }

    /// The slot member `index` reports a failure at: its own, for a
    /// member of a function's list, and the tuple's otherwise.
    fn position(&self, index: usize, outer: AbiPosition) -> AbiPosition {
        match self {
            Self::Named(named) => named.position(index),
            Self::Tuple(_) => outer,
        }
    }
}

/// Check the members of a tuple of `count` against `ty`, or against
/// the function's list the check starts at, with `check` comparing
/// member `index` and its declared type.
fn typecheck_members(
    ty: &ValueType,
    types: &TypeChecker<'_>,
    count: usize,
    rust: impl FnOnce() -> TypeCheckError,
    mut check: impl FnMut(
        usize,
        &ValueType,
        &TypeChecker<'_>,
    ) -> core::result::Result<(), TypeCheckError>,
) -> core::result::Result<(), TypeCheckError> {
    if let Some(named) = types.take_named() {
        if named.count() != count {
            return Err(TypeCheckError::count(named.count(), count));
        }
        for index in 0..count {
            let member = named
                .get(index)
                .ok_or_else(|| TypeCheckError::count(named.count(), count))?;
            check(index, member, types).map_err(|error| error.at(named.step(index)))?;
        }
        return Ok(());
    }
    let ValueType::Tuple(tuple) = ty else {
        return Err(rust());
    };
    if tuple.elements().len() != count {
        return Err(TypeCheckError::count(tuple.elements().len(), count));
    }
    for (index, member) in tuple.elements().iter().enumerate() {
        check(index, member, types).map_err(|error| error.at(TypePathStep::TupleMember(index)))?;
    }
    Ok(())
}

macro_rules! tuple {
    ($count:literal $(; $($index:tt $name:ident),+)?) => {
        impl<$($($name: ComponentType),+)?> ComponentType for ($($($name,)+)?) {
            const ABI: CanonicalAbiInfo =
                CanonicalAbiInfo::record_static(&[$($($name::ABI),+)?]);

            const IS_RUST_UNIT_TYPE: bool = $count == 0;

            #[allow(unused_variables)]
            fn typecheck(ty: &ValueType, types: &TypeChecker<'_>) -> core::result::Result<(), TypeCheckError> {
                typecheck_members(
                    ty,
                    types,
                    $count,
                    || TypeCheckError::mismatch::<Self>(ty),
                    |index, member, types| match index {
                        $($($index => $name::typecheck(member, types),)+)?
                        _ => Err(TypeCheckError::count(index, $count)),
                    },
                )
            }
        }

        impl<$($($name: ComponentType),+)?> ComponentNamedList for ($($($name,)+)?) {}

        impl<$($($name: Lower),+)?> Lower for ($($($name,)+)?) {
            #[allow(unused_variables)]
            fn lower_flat<D: 'static>(
                &self,
                cx: &mut LowerContext<'_, '_, D>,
                ty: &ValueType,
                out: &mut Vec<CoreVal>,
            ) -> Result<()> {
                let outer = cx.position();
                let members = Members::of(cx.take_named(), ty, $count, outer)?;
                $($(
                    cx.set_position(members.position($index, outer));
                    self.$index.lower_flat(cx, members.get($index), out)?;
                )+)?
                cx.set_position(outer);
                Ok(())
            }

            #[allow(unused_variables, unused_mut, unused_assignments)]
            fn lower_memory<D: 'static>(
                &self,
                cx: &mut LowerContext<'_, '_, D>,
                ty: &ValueType,
                offset: usize,
            ) -> Result<()> {
                let outer = cx.position();
                let members = Members::of(cx.take_named(), ty, $count, outer)?;
                let mut field = 0usize;
                $($(
                    field = align_to(field, $name::ALIGN32 as usize);
                    cx.set_position(members.position($index, outer));
                    self.$index.lower_memory(cx, members.get($index), offset + field)?;
                    field += $name::SIZE32;
                )+)?
                cx.set_position(outer);
                Ok(())
            }

            #[allow(unused_variables)]
            fn check_store(&self, store: StoreId) -> Result<()> {
                $($( self.$index.check_store(store)?; )+)?
                Ok(())
            }
        }

        impl<$($($name: Lift),+)?> Lift for ($($($name,)+)?) {
            #[allow(unused_variables)]
            fn lift_flat<D: 'static>(
                cx: &mut LiftContext<'_, '_, D>,
                ty: &ValueType,
                slots: &[CoreVal],
                cursor: &mut usize,
            ) -> Result<Self> {
                let outer = cx.position();
                let members = Members::of(cx.take_named(), ty, $count, outer)?;
                let lifted = ($($(
                    {
                        cx.set_position(members.position($index, outer));
                        $name::lift_flat(cx, members.get($index), slots, cursor)?
                    },
                )+)?);
                cx.set_position(outer);
                Ok(lifted)
            }

            #[allow(unused_variables, unused_mut, unused_assignments)]
            fn lift_memory<D: 'static>(
                cx: &mut LiftContext<'_, '_, D>,
                ty: &ValueType,
                offset: usize,
            ) -> Result<Self> {
                let outer = cx.position();
                let members = Members::of(cx.take_named(), ty, $count, outer)?;
                let mut field = 0usize;
                let lifted = ($($(
                    {
                        field = align_to(field, $name::ALIGN32 as usize);
                        cx.set_position(members.position($index, outer));
                        let member = $name::lift_memory(cx, members.get($index), offset + field)?;
                        field += $name::SIZE32;
                        member
                    },
                )+)?);
                cx.set_position(outer);
                Ok(lifted)
            }
        }
    };
}

tuple!(0);
tuple!(1; 0 A);
tuple!(2; 0 A, 1 B);
tuple!(3; 0 A, 1 B, 2 C);
tuple!(4; 0 A, 1 B, 2 C, 3 E);
tuple!(5; 0 A, 1 B, 2 C, 3 E, 4 F);
tuple!(6; 0 A, 1 B, 2 C, 3 E, 4 F, 5 G);
tuple!(7; 0 A, 1 B, 2 C, 3 E, 4 F, 5 G, 6 H);
tuple!(8; 0 A, 1 B, 2 C, 3 E, 4 F, 5 G, 6 H, 7 I);
tuple!(9; 0 A, 1 B, 2 C, 3 E, 4 F, 5 G, 6 H, 7 I, 8 J);
tuple!(10; 0 A, 1 B, 2 C, 3 E, 4 F, 5 G, 6 H, 7 I, 8 J, 9 K);
tuple!(11; 0 A, 1 B, 2 C, 3 E, 4 F, 5 G, 6 H, 7 I, 8 J, 9 K, 10 L);
tuple!(12; 0 A, 1 B, 2 C, 3 E, 4 F, 5 G, 6 H, 7 I, 8 J, 9 K, 10 L, 11 M);
tuple!(13; 0 A, 1 B, 2 C, 3 E, 4 F, 5 G, 6 H, 7 I, 8 J, 9 K, 10 L, 11 M, 12 N);
tuple!(14; 0 A, 1 B, 2 C, 3 E, 4 F, 5 G, 6 H, 7 I, 8 J, 9 K, 10 L, 11 M, 12 N, 13 O);
tuple!(15; 0 A, 1 B, 2 C, 3 E, 4 F, 5 G, 6 H, 7 I, 8 J, 9 K, 10 L, 11 M, 12 N, 13 O, 14 P);
tuple!(16; 0 A, 1 B, 2 C, 3 E, 4 F, 5 G, 6 H, 7 I, 8 J, 9 K, 10 L, 11 M, 12 N, 13 O, 14 P, 15 Q);
tuple!(17; 0 A, 1 B, 2 C, 3 E, 4 F, 5 G, 6 H, 7 I, 8 J, 9 K, 10 L, 11 M, 12 N, 13 O, 14 P, 15 Q, 16 R);
