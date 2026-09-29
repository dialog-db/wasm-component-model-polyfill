//! The faithfulness suite on the Wasmi backend: the specification test
//! suite at the pinned revision, for the floor and for each capability the
//! backend declares, run through the runtime layer.

#![cfg(not(target_arch = "wasm32"))]

use wcmp_wasm_core::Engine;
use wcmp_wasm_core_wasmi::Wasmi;

fn engine() -> Engine {
    Engine::with_backend(Wasmi::new())
}

/// The directives Wasmi fails, each with the defect that explains it.
const EXPECTED_FAILURES: &str = include_str!("faithfulness/expected-failures.txt");

wcmp_wasm_core_faithfulness::faithfulness_tests!(engine, EXPECTED_FAILURES);
