//! The model's error type.

use crate::stage::Stage;

/// Why a scenario's files could not be read, or a run could not be
/// judged.
///
/// None of these is a stage. Each one means that a file is malformed or
/// that a runner handed the model something that does not fit the
/// scenario, and a person has to fix the file or the runner.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A line of an expectations or observations file is not in the
    /// format. Lines count from 1.
    #[error("line {line}: {reason}")]
    Syntax {
        /// The line that failed to parse.
        line: usize,
        /// What is wrong with it.
        reason: String,
    },
    /// A name that is not the name of a stage.
    #[error("`{0}` is not a stage")]
    UnknownStage(String),
    /// A name that is not the name of a subject.
    #[error("`{0}` is not a subject")]
    UnknownSubject(String),
    /// An observations file has no `stage` line.
    #[error("the observations have no `stage` line")]
    MissingStage,
    /// A run reported a different number of outcomes from the number of
    /// entries it was judged against.
    #[error("the run reported {observed} outcomes for {expected} calls")]
    CallCount {
        /// The number of entries.
        expected: usize,
        /// The number of outcomes the run reported.
        observed: usize,
    },
    /// The Wasmtime run passed, but its observations do not list the
    /// calls the expectations list, so they were made from another
    /// version of the scenario. Calls count from 1.
    #[error(
        "the observations are stale: call {call} is `{observed}` where the expectations have `{expected}`"
    )]
    StaleObservations {
        /// The first call that differs.
        call: usize,
        /// The call as the expectations list it, or `nothing`.
        expected: String,
        /// The call as the observations list it, or `nothing`.
        observed: String,
    },
    /// Observations of a run that stopped before its calls were asked
    /// for with a stage that a run reaches only by making them.
    #[error("a run that made no calls cannot stop at `{0}`")]
    StoppedLate(Stage),
}

/// The result of reading a scenario's files or judging a run.
pub type Result<T> = core::result::Result<T, Error>;
