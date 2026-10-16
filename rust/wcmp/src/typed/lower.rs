// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A typed value written into a guest.

use crate::error::Result;
use crate::internal::LowerContextInternal;
use crate::runtime_layer::Val as CoreVal;
use crate::store::StoreId;
use crate::types::ValueType;

use super::component_type::ComponentType;
use super::flat::{declared_list, list_layout_error};
use super::lower_context::LowerContext;

/// A typed value the host can write into a guest, as Wasmtime's
/// `Lower` is.
///
/// Its methods are hidden. They lower the value straight into the
/// guest's flat values or memory, against the type the component
/// declares, which the type check has already compared with the Rust
/// type. No [`Val`](crate::Val) is built at any depth.
pub trait Lower: ComponentType {
    /// Lower the value, declared `ty`, into the flat values it takes,
    /// appended to `out`.
    #[doc(hidden)]
    fn lower_flat<D: 'static>(
        &self,
        cx: &mut LowerContext<'_, '_, D>,
        ty: &ValueType,
        out: &mut Vec<CoreVal>,
    ) -> Result<()>;

    /// Lower the value, declared `ty`, into guest memory at `offset`.
    #[doc(hidden)]
    fn lower_memory<D: 'static>(
        &self,
        cx: &mut LowerContext<'_, '_, D>,
        ty: &ValueType,
        offset: usize,
    ) -> Result<()>;

    /// Lower `items` as the elements of the list `ty`, into memory the
    /// guest's `cabi_realloc` gives, and answer the list's pointer.
    ///
    /// The list's bytes are built on the host and reach the guest in
    /// one write. A list of numbers overrides this and writes the
    /// numbers' bytes as they are.
    #[doc(hidden)]
    fn lower_list<D: 'static>(
        items: &[Self],
        cx: &mut LowerContext<'_, '_, D>,
        ty: &ValueType,
    ) -> Result<usize>
    where
        Self: Sized,
    {
        let position = cx.position();
        let element = declared_list(ty).ok_or_else(|| list_layout_error(ty, position))?;
        let total = Self::SIZE32.saturating_mul(items.len());
        let boundary = cx.boundary();
        // `cabi_realloc` runs even for an empty list, as the Canonical
        // ABI prescribes, so a guest allocator that misbehaves traps.
        let ptr = boundary.allocate_aligned(total, Self::ALIGN32 as usize, ty, position)?;
        boundary.lowering_within(ptr, total, position, ty, |boundary| {
            let mut cx = LowerContext::new(boundary, position);
            for (index, item) in items.iter().enumerate() {
                item.lower_memory(&mut cx, element, ptr + index * Self::SIZE32)?;
            }
            Ok(())
        })?;
        Ok(ptr)
    }

    /// Refuse the value, with the wrong-store cause, when it holds a
    /// host value of a store other than `store`, before a call into
    /// that store creates anything. The value types that hold an error
    /// context check it; every other type holds none.
    #[doc(hidden)]
    fn check_store(&self, store: StoreId) -> Result<()> {
        let _ = store;
        Ok(())
    }
}
