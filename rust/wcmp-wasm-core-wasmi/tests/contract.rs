// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The backend contract of the runtime layer, on the Wasmi backend.
//!
//! Wasmi declares neither `gc` nor `exceptions`, nor typed function
//! references, threads, or stack switching. Each test of the contract that
//! needs one of them checks here that the backend refuses its modules with
//! `Unsupported`, naming a capability it lacks.

#![cfg(not(target_arch = "wasm32"))]

use wcmp_wasm_core_wasmi::Wasmi;

wcmp_wasm_core_contract::contract_tests!(Wasmi::new);
