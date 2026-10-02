// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One benchmark: what it drives, what it moves, and how it is run.

use core::future::Future;
use core::pin::Pin;

use crate::case::Case;
use crate::error::Result;
use crate::measurement::Measurement;
use crate::plan::Plan;
use crate::run::Run;

/// A benchmark body: the `async fn` the `bench` attribute wrote, as
/// the runner holds it.
///
/// The future is boxed because the runner keeps benchmarks of
/// different bodies in one list; the box is taken once per benchmark,
/// outside every sample, so it is not part of what is measured.
pub type BenchmarkBody = fn(&mut Run) -> Pin<Box<dyn Future<Output = Result<()>> + '_>>;

/// One benchmark of the suite, at one case.
///
/// A benchmark names the guest it drives and the value it moves, so a
/// number in the report has a meaning without the source beside it.
/// The `bench` attribute builds these; a suite lists them.
pub struct Benchmark {
    name: String,
    guest: &'static str,
    payload: &'static str,
    case: Case,
    body: BenchmarkBody,
}

impl Benchmark {
    /// One benchmark per case, all sharing `body`.
    ///
    /// A case that is not [`Case::None`] extends the definition's name
    /// with `/<case>`, so `string-roundtrip` at 4096 bytes reports as
    /// `string-roundtrip/4096`.
    pub fn cases(
        name: &str,
        guest: &'static str,
        payload: &'static str,
        cases: &[Case],
        body: BenchmarkBody,
    ) -> Vec<Self> {
        cases
            .iter()
            .map(|case| Self {
                name: match case.suffix() {
                    Some(suffix) => format!("{name}/{suffix}"),
                    None => name.to_owned(),
                },
                guest,
                payload,
                case: *case,
                body,
            })
            .collect()
    }

    /// The benchmark's name, with its case.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The guest the benchmark drives.
    pub fn guest(&self) -> &'static str {
        self.guest
    }

    /// The value the benchmark moves.
    pub fn payload(&self) -> &'static str {
        self.payload
    }

    /// The case the benchmark is measured at.
    pub fn case(&self) -> Case {
        self.case
    }

    /// Measure this benchmark under `plan`.
    ///
    /// A benchmark that errors — a missing export, a trap, a target
    /// with no clock — is measured no further and reports the error in
    /// place of its numbers.
    pub async fn measure(&self, plan: Plan) -> Measurement {
        let mut run = match Run::new(plan, self.case) {
            Ok(run) => run,
            Err(error) => return Measurement::failed(self, &error),
        };
        // The body is awaited into a binding of its own: the future
        // borrows the run, and the measurement reads it back.
        let outcome = (self.body)(&mut run).await;
        match outcome {
            Ok(()) => Measurement::new(self, &run, None),
            Err(error) => Measurement::new(self, &run, Some(&error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::case::Case;

    /// A plan that finishes at once: the body is empty, so any batch
    /// is already longer than the target.
    fn quick() -> Plan {
        Plan {
            warmup_iterations: 2,
            samples: 3,
            target_sample_ms: 0.0000001,
            max_batch: 8,
        }
    }

    #[wcmp_macros::bench(
        guest = "no guest: the harness measuring its own loop",
        payload = "nothing crosses a boundary; the case is carried through",
        cases = [1, 2]
    )]
    async fn a_loop(run: &mut Run) -> Result<()> {
        run.moves_elements(run.case().number());
        while run.iterate() {}
        Ok(())
    }

    #[wcmp_macros::bench(guest = "no guest", payload = "nothing: the body never iterates")]
    async fn a_body_that_never_iterates(_run: &mut Run) -> Result<()> {
        Ok(())
    }

    #[wcmp_macros::test]
    fn it_names_one_benchmark_per_case() {
        let benchmarks = a_loop();
        assert_eq!(benchmarks.len(), 2);
        assert_eq!(benchmarks[0].name(), "a-loop/1");
        assert_eq!(benchmarks[1].name(), "a-loop/2");
        assert_eq!(benchmarks[0].case(), Case::Number(1));
        assert_eq!(
            a_body_that_never_iterates()[0].name(),
            "a-body-that-never-iterates"
        );
    }

    #[wcmp_macros::test]
    async fn it_measures_a_body_on_this_target() {
        let benchmarks = a_loop();
        let measurement = benchmarks[1].measure(quick()).await;
        assert_eq!(measurement.error(), None);
        let median = measurement.median_ns().expect("a median");
        assert!(median >= 0.0, "{median}");
        let json = measurement.json();
        assert!(json.contains("\"name\":\"a-loop/2\""), "{json}");
        assert!(json.contains("\"case\":2"), "{json}");
        assert!(json.contains("\"elements_per_iteration\":2"), "{json}");
        assert!(json.contains("\"error\":null"), "{json}");
    }

    #[wcmp_macros::test]
    async fn it_reports_a_body_that_never_iterated_as_a_failure() {
        let benchmarks = a_body_that_never_iterates();
        let measurement = benchmarks[0].measure(quick()).await;
        let error = measurement.error().expect("no sample is a failure");
        assert!(error.contains("recorded no sample"), "{error}");
    }
}
