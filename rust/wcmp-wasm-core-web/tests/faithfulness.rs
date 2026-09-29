//! The faithfulness suite on the browser backend: the specification test
//! suite at the pinned revision, for the floor and for each capability the
//! browser declares, run through the runtime layer.

#![cfg(target_arch = "wasm32")]

use wcmp_wasm_core::Engine;
use wcmp_wasm_core_web::Web;

wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

fn engine() -> Engine {
    Engine::with_backend(Web::new())
}

/// The directives the browser's engine fails, each with what explains it:
/// a defect of the engine, or a limit that the web embedding requires.
const EXPECTED_FAILURES: &str = include_str!("faithfulness/expected-failures.txt");

wcmp_wasm_core_faithfulness::faithfulness_tests!(engine, EXPECTED_FAILURES);
