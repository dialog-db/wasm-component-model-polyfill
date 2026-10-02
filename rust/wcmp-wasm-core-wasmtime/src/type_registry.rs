// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The concrete heap types a backend has described.

use std::sync::{Mutex, PoisonError};

use wcmp_wasm_core::backend::RawTypeHandle;
use wcmp_wasm_core::{Error, Result, TypeHandle};

/// The concrete heap types a backend has described, each under the number
/// its [`TypeHandle`] carries.
///
/// Wasmtime canonicalizes types across its engine, so the registry gives
/// two types the same number exactly when Wasmtime's `HeapType::eq` takes
/// them for one type. It keeps each type for the life of the backend: a
/// handle the host holds must still name its type later, when the host
/// makes a function or a global of it.
#[derive(Debug, Default)]
pub struct TypeRegistry {
    types: Mutex<Vec<wasmtime::HeapType>>,
}

impl TypeRegistry {
    /// The handle of the concrete type `ty`, which the registry numbers the
    /// first time it sees it.
    pub fn handle(&self, ty: &wasmtime::HeapType) -> TypeHandle {
        let mut types = self.types.lock().unwrap_or_else(PoisonError::into_inner);
        let index = match types
            .iter()
            .position(|known| wasmtime::HeapType::eq(known, ty))
        {
            Some(index) => index,
            None => {
                types.push(ty.clone());
                types.len() - 1
            }
        };
        TypeHandle::from_raw(index as u64)
    }

    /// The concrete type that `handle` names.
    ///
    /// A handle this registry did not make, such as one from another
    /// engine, is [`Error::TypeMismatch`].
    pub fn get(&self, handle: TypeHandle) -> Result<wasmtime::HeapType> {
        let types = self.types.lock().unwrap_or_else(PoisonError::into_inner);
        usize::try_from(handle.raw())
            .ok()
            .and_then(|index| types.get(index))
            .cloned()
            .ok_or_else(|| Error::TypeMismatch {
                message: format!("{handle} is not a type of this engine"),
            })
    }
}
