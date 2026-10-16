// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A typed value read out of a guest.

use crate::abi::gate_list;
use crate::error::Result;
use crate::internal::LiftContextInternal;
use crate::runtime_layer::Val as CoreVal;
use crate::types::ValueType;

use super::component_type::ComponentType;
use super::flat::{declared_list, list_layout_error};
use super::lift_context::LiftContext;

/// A typed value the host can read out of a guest, as Wasmtime's
/// `Lift` is.
///
/// Its methods are hidden. They lift the value straight out of the
/// guest's flat values or memory, against the type the component
/// declares, which the type check has already compared with the Rust
/// type. No [`Val`](crate::Val) is built at any depth.
pub trait Lift: Sized + ComponentType {
    /// Lift a value declared `ty` out of the flat values at `cursor`,
    /// and move `cursor` past them.
    #[doc(hidden)]
    fn lift_flat<D: 'static>(
        cx: &mut LiftContext<'_, '_, D>,
        ty: &ValueType,
        slots: &[CoreVal],
        cursor: &mut usize,
    ) -> Result<Self>;

    /// Lift a value declared `ty` out of guest memory at `offset`.
    #[doc(hidden)]
    fn lift_memory<D: 'static>(
        cx: &mut LiftContext<'_, '_, D>,
        ty: &ValueType,
        offset: usize,
    ) -> Result<Self>;

    /// Lift the `len` elements of the list `ty` at `ptr`.
    ///
    /// The list's range is checked against the memory and read in one
    /// access, and each element lifts out of that copy. The list
    /// charges the crossing's copy budget the size of one `Self` for
    /// each element, which is what the host holds for it, as Wasmtime
    /// charges a typed list. A list of numbers overrides this and
    /// decodes the range as it is.
    #[doc(hidden)]
    fn lift_list<D: 'static>(
        cx: &mut LiftContext<'_, '_, D>,
        ty: &ValueType,
        ptr: usize,
        len: usize,
    ) -> Result<Vec<Self>> {
        let position = cx.position();
        let element = declared_list(ty).ok_or_else(|| list_layout_error(ty, position))?;
        let boundary = cx.boundary();
        let (byte_len, _) = gate_list(boundary, ptr, len, element, ty, position)?;
        boundary.charge_copy_budget(len, core::mem::size_of::<Self>(), position, ty)?;
        boundary.lifting_within(ptr, byte_len, position, ty, |boundary| {
            let mut cx = LiftContext::new(boundary, position);
            let mut items = Vec::with_capacity(len);
            for index in 0..len {
                items.push(Self::lift_memory(
                    &mut cx,
                    element,
                    ptr + index * Self::SIZE32,
                )?);
            }
            Ok(items)
        })
    }
}
