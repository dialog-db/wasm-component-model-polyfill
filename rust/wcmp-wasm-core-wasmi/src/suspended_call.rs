// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A resumable call that Wasmi set aside.

use core::any::Any;

use wcmp_wasm_core::backend::BackendSuspendedCall;

/// A resumable call that waits in a suspending host function, as Wasmi
/// set it aside.
///
/// Wasmi's handle owns the stack of the call, and not the store, so any
/// number of calls can wait at once in one store. The handle does not
/// reach the store when it drops: it gives its stack back to the engine.
/// So a call that waits can outlive its store.
pub struct WasmiSuspendedCall {
    call: wasmi::ResumableCallHostTrap,
    results: Vec<wasmi::ValType>,
}

impl WasmiSuspendedCall {
    /// The call `call`, whose function gives results of `results` types.
    pub fn new(call: wasmi::ResumableCallHostTrap, results: Vec<wasmi::ValType>) -> Self {
        Self { call, results }
    }

    /// The number of results of the call.
    pub fn result_count(&self) -> usize {
        self.results.len()
    }

    /// Wasmi's handle of the call, and the types of the call's results.
    pub fn into_parts(self) -> (wasmi::ResumableCallHostTrap, Vec<wasmi::ValType>) {
        (self.call, self.results)
    }
}

impl BackendSuspendedCall for WasmiSuspendedCall {
    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}
