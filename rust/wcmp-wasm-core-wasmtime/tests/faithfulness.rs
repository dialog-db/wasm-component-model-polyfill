//! The faithfulness suite on the Wasmtime backend: the specification test
//! suite at the pinned revision, for the floor and for each capability the
//! backend declares, run through the runtime layer.

#![cfg(not(target_arch = "wasm32"))]

use wcmp_wasm_core::Engine;
use wcmp_wasm_core_wasmtime::Wasmtime;

fn engine() -> Engine {
    Engine::with_backend(Wasmtime::new().expect("Wasmtime makes an engine"))
}

/// The directives Wasmtime fails, each with the defect that explains it.
const EXPECTED_FAILURES: &str = include_str!("faithfulness/expected-failures.txt");

wcmp_wasm_core_faithfulness::faithfulness_tests!(engine, EXPECTED_FAILURES);

/// The runner reports a directive the engine does not pass, on its line,
/// so a script's test cannot pass by running nothing.
#[wcmp_macros::test]
async fn it_reports_each_directive_the_engine_does_not_pass() {
    let run = wcmp_wasm_core_faithfulness::run_script(
        &engine(),
        r#"
        (module (func (export "one") (result i32) (i32.const 1)))
        (assert_return (invoke "one") (i32.const 1))
        (assert_return (invoke "one") (i32.const 2))
        (assert_trap (invoke "one") "unreachable")
        (assert_invalid (module (func (result i32) (i32.const 1))) "type mismatch")
        "#,
    )
    .await;

    assert_eq!(run.directives(), 5);
    assert_eq!(run.passed(), 2);
    let lines = run
        .failures()
        .iter()
        .map(|(line, _)| *line)
        .collect::<Vec<_>>();
    assert_eq!(lines, [4, 5, 6]);
}

/// A listed directive that passes fails its script's test, so the list of
/// expected failures stays current.
#[wcmp_macros::test]
#[should_panic(expected = "stale expectation: fac.wast:89")]
async fn it_fails_a_script_whose_listed_directive_passes() {
    wcmp_wasm_core_faithfulness::check_script(
        &engine(),
        "fac.wast",
        "fac.wast:89 https://github.com/bytecodealliance/wasmtime/issues/1 a directive that passes\n",
    )
    .await;
}

/// An expected failure without a citation of a defect of the engine fails
/// the check of the list.
#[wcmp_macros::test]
#[should_panic(expected = "`fac.wast:89` cites no defect of the engine")]
fn it_fails_the_check_of_an_expected_failure_without_a_citation() {
    wcmp_wasm_core_faithfulness::check_expected_failures("fac.wast:89 the engine fails it\n");
}
