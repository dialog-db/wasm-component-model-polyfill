#![warn(missing_docs)]

//! The polyfill's benchmark suite: one definition per benchmark,
//! measured natively and in a browser.
//!
//! A benchmark is an `async fn` carrying the [`macro@wcmp_macros::bench`]
//! attribute. It sets its guest up once, then loops on [`Run::iterate`],
//! which owns the timer, the warm-up, and the batching. Nothing in a
//! benchmark body is per-target, so the same definition is what both
//! runners drive: the native binary times it against the monotonic
//! clock, and the same code compiled to `wasm32-unknown-unknown` times
//! it against the browser's `performance.now()`.
//!
//! [`measure`] runs the whole suite and returns a [`Report`], which
//! renders as a table for a reader and as JSON for tooling. The JSON has
//! the same shape from either target, so two runs can be read side by
//! side — see this crate's `README.md` for how to read the numbers and
//! for what does not compare across targets.

// The `bench` attribute names this crate by path, as a macro must; a
// benchmark defined here therefore needs the crate to be reachable
// under its own name from inside itself.
extern crate self as wcmp_bench;

mod benchmark;
mod case;
mod clock;
mod error;
mod guests;
mod json;
mod measurement;
mod plan;
mod report;
mod run;
mod suite;

pub use crate::benchmark::{Benchmark, BenchmarkBody};
pub use crate::case::Case;
pub use crate::clock::Clock;
pub use crate::error::{Error, Result};
pub use crate::measurement::Measurement;
pub use crate::plan::Plan;
pub use crate::report::Report;
pub use crate::run::Run;
pub use crate::suite::benchmarks;

/// The target a report was measured on, as it appears in the report.
#[cfg(not(target_arch = "wasm32"))]
pub const TARGET: &str = "native";

/// The target a report was measured on, as it appears in the report.
#[cfg(target_arch = "wasm32")]
pub const TARGET: &str = "wasm32-unknown-unknown";

/// Measure every benchmark in the suite under `plan`, in the order
/// [`benchmarks`] lists them, and return the report.
///
/// A benchmark that fails is recorded with its error and the run goes
/// on: one broken guest does not cost the other numbers.
pub async fn measure(plan: Plan) -> Report {
    let mut measurements = Vec::new();
    for benchmark in benchmarks() {
        measurements.push(benchmark.measure(plan).await);
    }
    Report::new(TARGET, plan, measurements)
}

// The suite's own unit tests reach a browser in the web lane, where
// the clock they exercise is the browser's.
#[cfg(all(test, target_arch = "wasm32"))]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);
