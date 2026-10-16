// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A `map<K, V>` as a Rust hash map, as Wasmtime maps it.
//!
//! A map is laid out as the list of its `(key, value)` entries. Entries
//! cross in the map's iteration order, out of one read of the list's
//! range and into one write, and a map lifted with a duplicate key
//! keeps the last entry.

use std::collections::HashMap;
use std::hash::Hash;

use wasmtime_environ::component::CanonicalAbiInfo;

use crate::abi::layout::align_to;
use crate::abi::{gate_list_layout, read_pointer_pair, write_pointer_pair};
use crate::error::{AbiPosition, Result, TypePathStep};
use crate::internal::{LiftContextInternal, LowerContextInternal};
use crate::runtime_layer::Val as CoreVal;
use crate::store::StoreId;
use crate::types::{MapType, ValueType};

use super::super::component_type::ComponentType;
use super::super::flat::{declared_mismatch, take_i32};
use super::super::lift::Lift;
use super::super::lift_context::LiftContext;
use super::super::lower::Lower;
use super::super::lower_context::LowerContext;
use super::super::type_check_error::TypeCheckError;
use super::super::type_checker::TypeChecker;

impl<K: ComponentType, V: ComponentType> ComponentType for HashMap<K, V> {
    const ABI: CanonicalAbiInfo = CanonicalAbiInfo::POINTER_PAIR;

    fn typecheck(
        ty: &ValueType,
        types: &TypeChecker<'_>,
    ) -> core::result::Result<(), TypeCheckError> {
        match ty {
            ValueType::Map(map) => {
                K::typecheck(map.key(), types).map_err(|error| error.at(TypePathStep::MapKey))?;
                V::typecheck(map.value(), types).map_err(|error| error.at(TypePathStep::MapValue))
            }
            _ => Err(TypeCheckError::mismatch::<Self>(ty)),
        }
    }
}

/// The layout of one entry of a map of `K` to `V`: its size, its
/// alignment, and the offset of the value from the key.
fn entry_layout<K: ComponentType, V: ComponentType>() -> (usize, usize, usize) {
    let entry = CanonicalAbiInfo::record_static(&[K::ABI, V::ABI]);
    (
        entry.size32 as usize,
        entry.align32 as usize,
        align_to(K::SIZE32, V::ALIGN32 as usize),
    )
}

/// The map type `ty` declares.
fn declared_map(ty: &ValueType, position: AbiPosition) -> Result<&MapType> {
    match ty {
        ValueType::Map(map) => Ok(map),
        _ => Err(declared_mismatch(ty, position)),
    }
}

/// Lower the entries of `map` into memory the guest's `cabi_realloc`
/// gives, and answer the list's pointer.
fn lower_entries<D: 'static, K: Lower, V: Lower>(
    map: &HashMap<K, V>,
    cx: &mut LowerContext<'_, '_, D>,
    ty: &ValueType,
) -> Result<usize> {
    let position = cx.position();
    let declared = declared_map(ty, position)?;
    let (size, align, value_offset) = entry_layout::<K, V>();
    let total = size.saturating_mul(map.len());
    let boundary = cx.boundary();
    // `cabi_realloc` runs even for an empty map, as it does for an
    // empty list.
    let ptr = boundary.allocate_aligned(total, align, ty, position)?;
    boundary.lowering_within(ptr, total, position, ty, |boundary| {
        let mut cx = LowerContext::new(boundary, position);
        for (index, (key, value)) in map.iter().enumerate() {
            let offset = ptr + index * size;
            key.lower_memory(&mut cx, declared.key(), offset)?;
            value.lower_memory(&mut cx, declared.value(), offset + value_offset)?;
        }
        Ok(())
    })?;
    Ok(ptr)
}

impl<K: Lower, V: Lower> Lower for HashMap<K, V> {
    fn lower_flat<D: 'static>(
        &self,
        cx: &mut LowerContext<'_, '_, D>,
        ty: &ValueType,
        out: &mut Vec<CoreVal>,
    ) -> Result<()> {
        let ptr = lower_entries(self, cx, ty)?;
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
        let ptr = lower_entries(self, cx, ty)?;
        let position = cx.position();
        write_pointer_pair(cx.boundary(), offset, ptr, self.len(), position, ty)
    }

    fn check_store(&self, store: StoreId) -> Result<()> {
        self.iter().try_for_each(|(key, value)| {
            key.check_store(store)?;
            value.check_store(store)
        })
    }
}

/// Lift the `len` entries of the map `ty` at `ptr`, out of one read of
/// their range. The map charges the crossing's copy budget the size of
/// one entry for each entry.
fn lift_entries<D: 'static, K: Lift + Eq + Hash, V: Lift>(
    cx: &mut LiftContext<'_, '_, D>,
    ty: &ValueType,
    ptr: usize,
    len: usize,
) -> Result<HashMap<K, V>> {
    let position = cx.position();
    let declared = declared_map(ty, position)?;
    let (size, align, value_offset) = entry_layout::<K, V>();
    let boundary = cx.boundary();
    let (byte_len, _) = gate_list_layout(boundary, ptr, len, size, align, ty, position)?;
    boundary.charge_copy_budget(len, core::mem::size_of::<(K, V)>(), position, ty)?;
    boundary.lifting_within(ptr, byte_len, position, ty, |boundary| {
        let mut cx = LiftContext::new(boundary, position);
        let mut map = HashMap::with_capacity(len);
        for index in 0..len {
            let offset = ptr + index * size;
            let key = K::lift_memory(&mut cx, declared.key(), offset)?;
            let value = V::lift_memory(&mut cx, declared.value(), offset + value_offset)?;
            map.insert(key, value);
        }
        Ok(map)
    })
}

impl<K: Lift + Eq + Hash, V: Lift> Lift for HashMap<K, V> {
    fn lift_flat<D: 'static>(
        cx: &mut LiftContext<'_, '_, D>,
        ty: &ValueType,
        slots: &[CoreVal],
        cursor: &mut usize,
    ) -> Result<Self> {
        let position = cx.position();
        let ptr = take_i32(slots, cursor, ty, position)? as u32 as usize;
        let len = take_i32(slots, cursor, ty, position)? as u32 as usize;
        lift_entries(cx, ty, ptr, len)
    }

    fn lift_memory<D: 'static>(
        cx: &mut LiftContext<'_, '_, D>,
        ty: &ValueType,
        offset: usize,
    ) -> Result<Self> {
        let position = cx.position();
        let (ptr, len) = read_pointer_pair(cx.boundary(), offset, position, ty)?;
        lift_entries(cx, ty, ptr, len)
    }
}
