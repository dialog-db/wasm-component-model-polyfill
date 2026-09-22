//! The run controls, which are the same on both targets.

use crate::error::{Error, Result};

/// How every benchmark in a run is warmed up and iterated.
///
/// One plan drives the whole suite, and the same plan means the same
/// warm-up and the same sampling on either target: what differs
/// between a native run and a browser run is the time each iteration
/// takes, never how many were taken or how they were counted.
///
/// A number of iterations is not fixed in advance. A benchmark's
/// iterations are grouped into batches sized from the warm-up's own
/// timing and then grown by an untimed calibration batch, so that one
/// timed batch lasts about [`Plan::target_sample_ms`] however fast the
/// target is. That is what keeps a browser's coarse clock from being
/// the thing measured.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    /// Iterations run before any measurement, both to reach a steady
    /// state and to estimate what one iteration costs.
    pub warmup_iterations: u64,
    /// Timed batches per benchmark. Each is one sample, and the report
    /// quotes their median and spread.
    pub samples: usize,
    /// How long one timed batch should last, in milliseconds.
    pub target_sample_ms: f64,
    /// The ceiling on a batch, so that a benchmark whose iteration is
    /// far below the clock's resolution still ends.
    pub max_batch: u64,
}

impl Default for Plan {
    fn default() -> Self {
        Self {
            warmup_iterations: 16,
            samples: 25,
            target_sample_ms: 5.0,
            max_batch: 100_000,
        }
    }
}

impl Plan {
    /// The control names an override may set, for an error message.
    const KEYS: &'static str = "warmup, samples, target-sample-ms, max-batch";

    /// Apply `key=value` overrides to this plan.
    ///
    /// Both runners take their overrides this way — the native one
    /// from its command line, the browser one from the page's query
    /// string — so that a run is asked for in one vocabulary
    /// whichever target answers it.
    pub fn with_overrides<'a>(
        mut self,
        overrides: impl IntoIterator<Item = &'a str>,
    ) -> Result<Self> {
        for entry in overrides {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            let (key, value) = entry.split_once('=').ok_or_else(|| {
                Error::Setup(format!(
                    "`{entry}` is not a `key=value` run control (one of {})",
                    Self::KEYS
                ))
            })?;
            match key {
                "warmup" => self.warmup_iterations = parse(key, value)?,
                "samples" => self.samples = parse(key, value)?,
                "target-sample-ms" => self.target_sample_ms = parse(key, value)?,
                "max-batch" => self.max_batch = parse(key, value)?,
                other => {
                    return Err(Error::Setup(format!(
                        "`{other}` is not a run control (one of {})",
                        Self::KEYS
                    )));
                }
            }
        }
        self.validate()
    }

    /// The plan with every control checked for a value a run can be
    /// made of.
    fn validate(self) -> Result<Self> {
        if self.warmup_iterations == 0 {
            return Err(Error::Setup("`warmup` must be at least 1".to_owned()));
        }
        if self.samples == 0 {
            return Err(Error::Setup("`samples` must be at least 1".to_owned()));
        }
        if !(self.target_sample_ms.is_finite() && self.target_sample_ms > 0.0) {
            return Err(Error::Setup(
                "`target-sample-ms` must be a positive number".to_owned(),
            ));
        }
        if self.max_batch == 0 {
            return Err(Error::Setup("`max-batch` must be at least 1".to_owned()));
        }
        Ok(self)
    }

    /// The plan as JSON, for the report.
    pub fn json(&self) -> String {
        format!(
            "{{\"warmup_iterations\":{},\"samples\":{},\"target_sample_ms\":{},\"max_batch\":{}}}",
            self.warmup_iterations, self.samples, self.target_sample_ms, self.max_batch
        )
    }
}

/// One control's value, with the control named when it does not parse.
fn parse<T: core::str::FromStr>(key: &str, value: &str) -> Result<T> {
    value
        .parse()
        .map_err(|_| Error::Setup(format!("`{value}` is not a value for `{key}`")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    fn it_takes_run_controls_as_key_value_words() {
        let plan = Plan::default()
            .with_overrides(["samples=3", "warmup=2", "target-sample-ms=0.5"])
            .expect("the controls parse");
        assert_eq!(plan.samples, 3);
        assert_eq!(plan.warmup_iterations, 2);
        assert!((plan.target_sample_ms - 0.5).abs() < f64::EPSILON);
        assert_eq!(plan.max_batch, Plan::default().max_batch);
    }

    #[wcmp_macros::test]
    fn it_ignores_an_empty_control() {
        let plan = Plan::default()
            .with_overrides(["", "  "])
            .expect("nothing to apply");
        assert_eq!(plan, Plan::default());
    }

    #[wcmp_macros::test]
    fn it_rejects_an_unknown_run_control() {
        let error = Plan::default()
            .with_overrides(["iterations=10"])
            .expect_err("`iterations` is not a control");
        let message = error.to_string();
        assert!(
            message.contains("`iterations` is not a run control"),
            "{message}"
        );
        assert!(message.contains("target-sample-ms"), "{message}");
    }

    #[wcmp_macros::test]
    fn it_rejects_a_control_without_a_value() {
        let error = Plan::default()
            .with_overrides(["samples"])
            .expect_err("a control needs a value");
        assert!(
            error
                .to_string()
                .contains("is not a `key=value` run control"),
            "{error}"
        );
    }

    #[wcmp_macros::test]
    fn it_rejects_a_control_whose_value_cannot_make_a_run() {
        for control in ["samples=0", "warmup=0", "target-sample-ms=0", "max-batch=0"] {
            let error = Plan::default()
                .with_overrides([control])
                .expect_err("the value is out of range");
            assert!(error.to_string().contains("must be"), "{control}: {error}");
        }
    }
}
