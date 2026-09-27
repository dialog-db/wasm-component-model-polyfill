//! One call the Wasmtime run made.

use core::fmt;

use crate::call::Call;
use crate::outcome::Outcome;

/// One call the Wasmtime run made, and how it ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    /// The call, as the expectations list it.
    pub call: Call,
    /// How the call ended.
    pub outcome: Outcome,
}

impl fmt::Display for Observation {
    /// The observation as a `call` line.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "call {} -> {}", self.call, self.outcome)
    }
}
