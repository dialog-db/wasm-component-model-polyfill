// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The backend contract of the runtime layer, on the Wasmtime backend.

#![cfg(not(target_arch = "wasm32"))]

use wcmp_wasm_core_wasmtime::Wasmtime;

fn backend() -> Wasmtime {
    Wasmtime::new().expect("Wasmtime makes an engine")
}

wcmp_wasm_core_contract::contract_tests!(backend);
