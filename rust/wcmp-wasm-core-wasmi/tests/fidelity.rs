// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The fidelity suite on the Wasmi backend: the specification test
//! suite at the pinned revision, for the floor and for each capability the
//! backend declares, run through the runtime layer.

#![cfg(not(target_arch = "wasm32"))]

use wcmp_wasm_core::Engine;
use wcmp_wasm_core_wasmi::Wasmi;

fn engine() -> Engine {
    Engine::with_backend(Wasmi::new())
}

/// The directives Wasmi fails, each with the defect that explains it.
const EXPECTED_FAILURES: &str = include_str!("fidelity/expected-failures.txt");

wcmp_wasm_core_fidelity::fidelity_tests!(engine, EXPECTED_FAILURES);
