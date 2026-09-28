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
    /// A name that is not the name of a kind of link.
    #[error("`{0}` is not a way to link")]
    UnknownLinking(String),
    /// A run-time link of a scenario's wiring names a component the
    /// scenario does not have.
    #[error("the link `{link}` names component {component}, which the scenario does not have")]
    UnknownComponent {
        /// The link, as its line spells it.
        link: String,
        /// The component it names.
        component: String,
    },
    /// A run-time link of a scenario's wiring names an import that its
    /// importer does not have, such as a misspelled interface name.
    #[error("the link `{link}` names an import that component {component} does not have")]
    UnknownImport {
        /// The link, as its line spells it.
        link: String,
        /// The importer.
        component: String,
    },
    /// A run-time link of a scenario's wiring names an export that its
    /// exporter does not have.
    #[error("the link `{link}` names an export that component {component} does not have")]
    UnknownExport {
        /// The link, as its line spells it.
        link: String,
        /// The exporter.
        component: String,
    },
    /// The run-time links of a scenario's wiring leave no order to
    /// instantiate its components in: each of these waits for another.
    #[error("the run-time links form a cycle among {}", .0.join(", "))]
    LinkCycle(Vec<String>),
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
    /// A call asked to be typed, but its signature is not one a
    /// runner makes typed calls for.
    #[error(
        "a typed call takes at most two parameters of one scalar or string type and returns nothing or one value of that type, and `{signature}` does not"
    )]
    Untyped {
        /// The signature, such as `(s32, u32) -> s32`.
        signature: String,
    },
}

/// The result of reading a scenario's files or judging a run.
pub type Result<T> = core::result::Result<T, Error>;
