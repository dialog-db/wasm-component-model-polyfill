#![warn(missing_docs)]

//! The faithfulness suite of the runtime layer of the Wasm Component Model
//! Polyfill.
//!
//! A backend of `wcmp_wasm_core` declares a capability only where its
//! engine implements the feature faithfully. This suite is the proof: the
//! official WebAssembly specification test suite, at the revision the
//! flake pins, run on one backend through the types of `wcmp_wasm_core`
//! alone. Nothing here names an engine, so every backend runs the same
//! runner with an engine of its own. A backend's test file invokes
//! [`faithfulness_tests!`] with a function that makes its engine and the
//! backend's list of expected failures:
//!
//! ```ignore
//! use wcmp_wasm_core::Engine;
//!
//! fn engine() -> Engine {
//!     Engine::with_backend(MyBackend::new())
//! }
//!
//! const EXPECTED_FAILURES: &str = include_str!("faithfulness/expected-failures.txt");
//!
//! wcmp_wasm_core_faithfulness::faithfulness_tests!(engine, EXPECTED_FAILURES);
//! ```
//!
//! # The scripts and their suites
//!
//! The suite holds the scripts at the top of the specification test suite,
//! which are the core specification, and the scripts of each proposal
//! directory whose feature the capability lexicon names. Each script
//! belongs to one [`Suite`]: the floor, Wasm 2.0, where every module the
//! script expects to be valid validates with the features of Wasm 2.0
//! alone, or the capabilities its modules need above the floor. The
//! validator decides, with the least set of features above the floor that
//! validates every such module; the names of the capability lexicon are
//! the names of the validator's features. A script whose modules need a
//! feature the lexicon does not name belongs to no capability, and no
//! backend runs it.
//!
//! A backend runs the floor scripts and the scripts of each capability it
//! declares. A script that needs a capability the backend does not declare
//! passes without a run.
//!
//! # A run
//!
//! A script runs in one store, directive by directive, as the reference
//! interpreter runs it. Modules compile and instantiate through the
//! runtime layer, and a function is called through it. The `spectest`
//! module is a set of host externs the runner makes in the store. A
//! module's imports are resolved by name against those and the instances
//! the script registered: there is no linker in the runtime layer.
//!
//! The runner checks what the host can see through the runtime layer:
//!
//! - A result value is compared with its pattern. A float compares by its
//!   bits, or by the class of NaN the pattern names. A reference compares
//!   by its hierarchy and whether it is null. The host reads the integer of
//!   an `i31ref` and the value of an `externref` it made, and it cannot look
//!   inside a GC object, so `ref.struct` and `ref.array` hold for any
//!   non-null internal reference that is not an `i31ref`.
//! - A trap must be a trap, and not a host error. Where the specification's
//!   message names one core trap, the trap must be that kind. An exhaustion
//!   must be a stack overflow, and an exception an uncaught exception.
//! - A malformed or an invalid module must fail to compile, and an
//!   unlinkable one must fail to link: the engine refuses its imports, or
//!   an import names nothing the script made importable, which the runner
//!   finds as it resolves the imports. The engine's words are not compared
//!   with the specification's, as Wasmtime's own run of the suite does not
//!   compare them.
//!
//! A directive that does not apply to the backend, or that the runtime
//! layer cannot express, is skipped, with its reason, and never counted as
//! a pass or a failure. Two kinds exist in the pinned revision:
//!
//! - A refusal that a capability the backend declares lifts. A script
//!   asserts that a module is refused under the features the script needs.
//!   Where a capability the backend declares above those makes the module
//!   valid, as stack switching does a tag with results, a faithful engine
//!   accepts it. The validator decides, as it decides the suites.
//! - An argument of `ref.host`, a host object inside the internal
//!   hierarchy, which the host cannot make through the runtime layer.
//!
//! # Expected failures
//!
//! Each backend has a list of the directives its engine fails. Each entry
//! cites what explains its failure: a defect of the engine, as an issue in
//! its tracker or a line of its source at a fixed commit, or a limit that
//! the embedding of the engine requires, as a line of the specification at
//! a fixed commit. An entry without a citation fails the check.
//! A directive that fails and is not listed fails its script's test, and so
//! does a listed directive that passes, so the list stays current. See
//! [`ExpectedFailures`] for the format.

mod check;
mod citation;
mod expected_failure;
mod expected_failures;
mod patterns;
mod runner;
mod script;
mod script_run;
mod spectest;
mod suite;
mod text;

pub use crate::check::{check_expected_failures, check_script, report_suites, run_script};
pub use crate::citation::Citation;
pub use crate::expected_failure::ExpectedFailure;
pub use crate::expected_failures::ExpectedFailures;
pub use crate::script::{SCRIPTS, Script};
pub use crate::script_run::ScriptRun;
pub use crate::suite::Suite;

/// The attribute each generated test carries, reached through this crate
/// so that a backend does not name it itself.
#[doc(hidden)]
pub mod __private {
    pub use wcmp_macros::test;
}

/// Expands to one test for each script of the suite, run with the engine
/// that `$engine`, a function of no arguments that returns an
/// [`Engine`](wcmp_wasm_core::Engine), makes, and held to `$expected`, a
/// `&str` constant that holds the backend's list of expected failures.
///
/// Three more tests come with them. One checks the list itself: every
/// entry cites what explains its failure and names a script of the suite.
/// One prints the suite of every script, and whether the backend runs it.
/// One checks that the build embedded the pinned test suite.
///
/// See the crate's documentation.
#[macro_export]
macro_rules! faithfulness_tests {
    ($engine:path, $expected:path $(,)?) => {
        $crate::__script_tests!($engine, $expected);

        #[$crate::__private::test]
        fn it_cites_an_engine_defect_for_each_expected_failure() {
            $crate::check_expected_failures($expected);
        }

        #[$crate::__private::test]
        fn it_reports_the_suite_of_each_script() {
            $crate::report_suites(&$engine());
        }

        #[$crate::__private::test]
        fn it_embeds_the_pinned_testsuite() {
            assert!(
                !$crate::SCRIPTS.is_empty(),
                "the build embedded no script: the faithfulness suite runs through \
                 `tests faithfulness`, which sets WCMP_SPEC_TESTSUITE to the pinned test suite"
            );
        }
    };
}

// The crate's own unit tests run in a browser in the web lane, as every
// other test binary of the workspace does.
#[cfg(all(test, target_arch = "wasm32"))]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);
