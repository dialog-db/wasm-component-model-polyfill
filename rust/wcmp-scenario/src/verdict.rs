//! Where a subject stopped, and why.

use crate::stage::Stage;

/// The stage where a subject stopped, with the text a person reads to
/// learn why.
///
/// The reason is for a person only. Two verdicts are compared by their
/// stage, because the text holds names and numbers that change whenever
/// the toolchain or the polyfill does. A pass has an empty reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    /// The first stage where the subject stopped.
    pub stage: Stage,
    /// Why it stopped there, such as the error the step returned.
    pub reason: String,
}

impl Verdict {
    /// A subject that stopped at `stage` for `reason`.
    pub fn new(stage: Stage, reason: impl Into<String>) -> Self {
        Self {
            stage,
            reason: reason.into(),
        }
    }

    /// A subject that met every expectation.
    pub fn pass() -> Self {
        Self::new(Stage::Pass, "")
    }

    /// Whether the subject met every expectation.
    pub fn passed(&self) -> bool {
        self.stage == Stage::Pass
    }
}
