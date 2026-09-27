//! One entry of an expectations file.

use core::fmt;

use crate::call::Call;
use crate::outcome::Outcome;

/// One entry of an expectations file: a call and the outcome it must
/// have, if the file gives one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The call to make.
    pub call: Call,
    /// The outcome the call must have. `None` leaves it to the Wasmtime
    /// run: a polyfill subject must then end the call as the Wasmtime
    /// run did.
    pub outcome: Option<Outcome>,
}

impl fmt::Display for Entry {
    /// The entry as a `call` line.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "call {}", self.call)?;
        match &self.outcome {
            Some(outcome) => write!(formatter, " -> {outcome}"),
            None => Ok(()),
        }
    }
}
