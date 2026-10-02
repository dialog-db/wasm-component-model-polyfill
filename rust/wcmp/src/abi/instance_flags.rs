// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The may-leave flag of one component instance.
//!
//! The reference keeps a may-leave flag on every component
//! instance. It is clear for the length of a call the polyfill
//! itself makes into the guest — the `cabi_realloc` a crossing asks
//! for memory with and the `post-return` of an export — and every
//! built-in that would leave the instance traps with the
//! cannot-leave cause while it is clear.
//!
//! The flag is a core-Wasm global, because the fused adapters are
//! generated code that reads and writes one: an adapter clears the
//! global while it translates values across a component boundary,
//! restores it afterwards, and traps when it is asked to call a
//! component that may not be left. The polyfill cannot change that
//! code, so the built-ins read and write the same global rather
//! than a second copy of the state beside it. One flag per
//! component instance, the one the generated code already uses.
//!
//! The global is an `i32` that holds 0 or 1, which is how the
//! adapters spell it, so no masking is needed to read it.

use crate::error::{Error, Result};
use crate::internal::ErrorInternal;
use crate::runtime_layer::{
    AsContextMut, Global as RuntimeGlobal, GlobalType, Mutability, Val as RuntimeVal, ValType,
    substrate_failure,
};

/// The may-leave flag of one component instance, as the core global
/// the instance's adapters import.
#[derive(Clone)]
pub struct InstanceFlags {
    /// The global itself. Mutable, because both the adapters and the
    /// polyfill write it.
    global: RuntimeGlobal,
}

impl InstanceFlags {
    /// Mint the flag of a fresh component instance, set: every
    /// instance may be left until an adapter is in the middle of
    /// translating values across its boundary, or the polyfill is in
    /// the middle of a call of its own into the guest.
    pub fn new(mut store: impl AsContextMut) -> Result<Self> {
        let global = RuntimeGlobal::new(
            store.as_context_mut(),
            GlobalType::new(ValType::I32, Mutability::Var),
            RuntimeVal::I32(1),
        )
        .map_err(substrate_failure)?;
        Ok(Self { global })
    }

    /// The global itself, for the import table of an adapter module
    /// that compiles against it.
    pub fn global(&self) -> &RuntimeGlobal {
        &self.global
    }

    /// Whether the instance may be left.
    pub fn may_leave(&self, mut store: impl AsContextMut) -> Result<bool> {
        match self.global.get(store.as_context_mut()) {
            Ok(RuntimeVal::I32(value)) => Ok(value != 0),
            Ok(other) => Err(Error::internal(format!(
                "the may-leave flag of a component instance holds {other:?} rather than an i32"
            ))),
            Err(error) => Err(substrate_failure(error)),
        }
    }

    /// Set the flag and answer with the value it had, which is what
    /// a call the polyfill makes into the guest puts back when it
    /// ends.
    pub fn set_may_leave(&self, mut store: impl AsContextMut, value: bool) -> Result<bool> {
        let old = self.may_leave(store.as_context_mut())?;
        self.global
            .set(store.as_context_mut(), RuntimeVal::I32(i32::from(value)))
            .map_err(|error| {
                Error::internal(format!(
                    "the may-leave flag of a component instance could not be written: {error}"
                ))
            })?;
        Ok(old)
    }
}
