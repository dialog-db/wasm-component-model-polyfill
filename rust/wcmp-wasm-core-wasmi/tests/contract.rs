// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The backend contract of the runtime layer, on the Wasmi backend.
//!
//! Wasmi declares neither `gc` nor `exceptions`, so the tests of the
//! contract that need either pass here without a check. `wasmi.rs` checks
//! what the backend does in their place: it refuses each of their modules
//! with `Unsupported`, naming the capability.

#![cfg(not(target_arch = "wasm32"))]

use wcmp_wasm_core::Engine;
use wcmp_wasm_core_wasmi::Wasmi;

fn engine() -> Engine {
    Engine::with_backend(Wasmi::new())
}

wcmp_wasm_core_contract::contract_tests!(engine);
