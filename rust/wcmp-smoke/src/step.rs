use crate::Outcome;

/// One step of the smoke test: what it exercises and how it went.
#[derive(Debug)]
pub struct Step {
    pub name: &'static str,
    pub outcome: Outcome,
}

impl Step {
    /// Run a step body and record its outcome. The body returns the
    /// evidence on success and the reason on failure.
    pub fn run(name: &'static str, body: impl FnOnce() -> Result<String, String>) -> Self {
        let outcome = match body() {
            Ok(evidence) => Outcome::Passed(evidence),
            Err(reason) => Outcome::Failed(reason),
        };
        Step { name, outcome }
    }

    pub fn skipped(name: &'static str, reason: impl Into<String>) -> Self {
        Step {
            name,
            outcome: Outcome::Skipped(reason.into()),
        }
    }
}
