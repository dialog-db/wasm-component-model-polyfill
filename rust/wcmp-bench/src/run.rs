// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One benchmark's run: the timer, the warm-up, and the batching.

use crate::case::Case;
use crate::clock::Clock;
use crate::error::Result;
use crate::plan::Plan;

/// The state one benchmark body is driven through.
///
/// A body sets its guest up, then loops:
///
/// ```ignore
/// while run.iterate() {
///     function.call(&mut store, &arguments).await?;
/// }
/// ```
///
/// [`Run::iterate`] is where every per-target concern lives. It reads
/// the clock, runs the warm-up, sizes a batch from what the warm-up
/// cost, and stops when the plan's samples are in, so a body is the
/// same text on a native host and on a page.
///
/// Setup is outside the loop and therefore outside every sample: a
/// benchmark measures the work it repeats, not the store it built to
/// repeat it in.
pub struct Run {
    plan: Plan,
    clock: Clock,
    case: Case,
    bytes: Option<u64>,
    elements: Option<u64>,
    phase: Phase,
    warmed: u64,
    batch: u64,
    in_batch: u64,
    iterations: u64,
    samples_ns: Vec<f64>,
    started: f64,
}

/// Where a run has got to.
enum Phase {
    /// Running the warm-up iterations, which are timed only as a
    /// whole, to size the batch.
    Warmup,
    /// Sizing the batch: timing one and growing it until it lasts
    /// long enough to be worth timing.
    Calibrate,
    /// Running timed batches.
    Measure,
    /// Every sample is in.
    Done,
}

impl Run {
    /// Start a run of `case` under `plan`.
    pub fn new(plan: Plan, case: Case) -> Result<Self> {
        Ok(Self {
            plan,
            clock: Clock::new()?,
            case,
            bytes: None,
            elements: None,
            phase: Phase::Warmup,
            warmed: 0,
            batch: 1,
            in_batch: 0,
            iterations: 0,
            samples_ns: Vec::with_capacity(plan.samples),
            started: 0.0,
        })
    }

    /// The case this run measures.
    pub fn case(&self) -> Case {
        self.case
    }

    /// State how many bytes one iteration moves across the boundary.
    ///
    /// The report carries it as `bytes_per_iteration` and derives a
    /// throughput from it, so a benchmark that moves a payload should
    /// say how big it is.
    pub fn moves_bytes(&mut self, bytes: u64) {
        self.bytes = Some(bytes);
    }

    /// State how many elements one iteration moves across the
    /// boundary: list elements, record fields, or handles, whichever
    /// the benchmark's payload is counted in.
    pub fn moves_elements(&mut self, elements: u64) {
        self.elements = Some(elements);
    }

    /// Whether to run one more iteration.
    ///
    /// Call it as the condition of the body's loop and nowhere else:
    /// each call closes the iteration before it and opens the one
    /// after, which is how the timer stays out of the body.
    pub fn iterate(&mut self) -> bool {
        match self.phase {
            Phase::Warmup => {
                if self.warmed == 0 {
                    self.started = self.clock.now();
                }
                if self.warmed < self.plan.warmup_iterations {
                    self.warmed += 1;
                    self.iterations += 1;
                    return true;
                }
                self.batch = self.batch_from_warmup(self.clock.now() - self.started);
                self.open(Phase::Calibrate);
                true
            }
            Phase::Calibrate => {
                self.in_batch += 1;
                if self.in_batch < self.batch {
                    self.iterations += 1;
                    return true;
                }
                // The calibration batch is timed and thrown away. Its
                // only job is to find a batch that lasts long enough
                // to be worth timing, which the warm-up's estimate
                // cannot settle on a clock that reads in steps of a
                // tenth of a millisecond.
                let elapsed = self.clock.now() - self.started;
                if elapsed < self.plan.target_sample_ms * 0.5 && self.batch < self.plan.max_batch {
                    self.batch = self.grown_batch(elapsed);
                    self.open(Phase::Calibrate);
                } else {
                    self.open(Phase::Measure);
                }
                true
            }
            Phase::Measure => {
                self.in_batch += 1;
                if self.in_batch < self.batch {
                    self.iterations += 1;
                    return true;
                }
                let elapsed = self.clock.now() - self.started;
                self.samples_ns
                    .push(elapsed * 1_000_000.0 / self.batch as f64);
                if self.samples_ns.len() >= self.plan.samples {
                    self.phase = Phase::Done;
                    return false;
                }
                self.open(Phase::Measure);
                true
            }
            Phase::Done => false,
        }
    }

    /// Enter `phase` with a fresh batch, and count the iteration the
    /// caller is about to run.
    fn open(&mut self, phase: Phase) {
        self.phase = phase;
        self.in_batch = 0;
        self.iterations += 1;
        self.started = self.clock.now();
    }

    /// The batch to calibrate from, out of what the warm-up cost.
    ///
    /// A warm-up that read as no time at all — a browser's clamped
    /// clock says that often — starts at a modest batch and lets the
    /// calibration grow it, rather than jumping to the ceiling and
    /// making one enormous batch of a benchmark that is not fast.
    fn batch_from_warmup(&self, warmup_ms: f64) -> u64 {
        let per_iteration = warmup_ms / self.plan.warmup_iterations as f64;
        if !(per_iteration.is_finite() && per_iteration > 0.0) {
            return 16.min(self.plan.max_batch);
        }
        let batch = (self.plan.target_sample_ms / per_iteration).ceil();
        if !batch.is_finite() || batch < 1.0 {
            return 1;
        }
        (batch as u64).min(self.plan.max_batch)
    }

    /// The next batch to try, from a calibration batch that finished
    /// too quickly to time. It always grows, so calibration ends.
    fn grown_batch(&self, elapsed_ms: f64) -> u64 {
        let grown = if elapsed_ms > 0.0 {
            let scale = (self.plan.target_sample_ms / elapsed_ms).ceil().max(2.0);
            (self.batch as f64 * scale) as u64
        } else {
            self.batch.saturating_mul(8)
        };
        grown.clamp(self.batch.saturating_add(1), self.plan.max_batch)
    }

    /// Every iteration the body ran: the warm-up, the calibration, and
    /// the timed batches.
    pub fn iterations(&self) -> u64 {
        self.iterations
    }

    /// The iterations one timed sample covers.
    pub fn batch(&self) -> u64 {
        self.batch
    }

    /// The samples, each already divided down to nanoseconds per
    /// iteration.
    pub fn samples_ns(&self) -> &[f64] {
        &self.samples_ns
    }

    /// The bytes one iteration moves, when the benchmark stated them.
    pub fn bytes(&self) -> Option<u64> {
        self.bytes
    }

    /// The elements one iteration moves, when the benchmark stated
    /// them.
    pub fn elements(&self) -> Option<u64> {
        self.elements
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plan that measures quickly: the calibration ends on its first
    /// batch, because any batch at all lasts longer than the target.
    fn quick() -> Plan {
        Plan {
            warmup_iterations: 2,
            samples: 3,
            target_sample_ms: 0.0000001,
            max_batch: 8,
        }
    }

    #[wcmp_macros::test]
    fn it_records_the_samples_the_plan_asks_for() {
        let mut run = Run::new(quick(), Case::Number(7)).expect("a clock");
        while run.iterate() {}
        assert_eq!(run.samples_ns().len(), 3);
        assert!(run.samples_ns().iter().all(|sample| *sample >= 0.0));
    }

    #[wcmp_macros::test]
    fn it_counts_every_iteration_it_handed_out() {
        let mut run = Run::new(quick(), Case::None).expect("a clock");
        let mut handed_out = 0u64;
        while run.iterate() {
            handed_out += 1;
        }
        assert_eq!(run.iterations(), handed_out);
        assert!(
            handed_out >= quick().warmup_iterations + quick().samples as u64,
            "{handed_out} iterations is fewer than the warm-up and the samples"
        );
    }

    #[wcmp_macros::test]
    fn it_carries_the_case_and_the_payload_the_body_states() {
        let mut run = Run::new(quick(), Case::Number(64)).expect("a clock");
        assert_eq!(run.case(), Case::Number(64));
        assert_eq!(run.bytes(), None);
        assert_eq!(run.elements(), None);
        run.moves_bytes(64);
        run.moves_elements(4);
        assert_eq!(run.bytes(), Some(64));
        assert_eq!(run.elements(), Some(4));
    }

    #[wcmp_macros::test]
    fn it_never_hands_out_another_iteration_once_it_is_done() {
        let mut run = Run::new(quick(), Case::None).expect("a clock");
        while run.iterate() {}
        assert!(!run.iterate(), "a finished run stays finished");
    }

    #[wcmp_macros::test]
    fn it_grows_a_batch_that_finished_too_quickly_to_time() {
        let plan = Plan {
            warmup_iterations: 1,
            samples: 1,
            target_sample_ms: 1.0,
            max_batch: 64,
        };
        let mut run = Run::new(plan, Case::None).expect("a clock");
        run.batch = 1;
        // A calibration batch that read as no time at all grows by
        // more than one, and never past the ceiling.
        assert!(run.grown_batch(0.0) > 1);
        run.batch = 32;
        assert_eq!(run.grown_batch(0.0), 64);
        run.batch = 2;
        assert!(
            run.grown_batch(0.5) >= 4,
            "a batch at half the target doubles"
        );
    }
}
