//! The Wasmtime run's error type.

use std::path::PathBuf;

/// Why the Wasmtime run could not read a scenario or write what it saw.
///
/// None of these is a stage. A stage is something a scenario reaches,
/// and it goes into the observations. Each of these means that the
/// build's layout, a scenario's files, or Wasmtime's own setup is
/// broken, and a person has to fix it.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A file or directory could not be read or written.
    #[error("{}: {source}", path.display())]
    Io {
        /// The file or directory.
        path: PathBuf,
        /// What went wrong.
        source: std::io::Error,
    },
    /// A directory does not hold what the build's layout says it holds.
    #[error("{}: {reason}", path.display())]
    Layout {
        /// The directory or file that is out of place.
        path: PathBuf,
        /// What is wrong with it.
        reason: String,
    },
    /// A scenario's expectations file is malformed.
    #[error("{}: {source}", path.display())]
    Expectations {
        /// The expectations file.
        path: PathBuf,
        /// What the scenario model found wrong with it.
        source: wcmp_scenario::Error,
    },
    /// The scenario model refused to judge the run.
    #[error("scenario {scenario}: {source}")]
    Judge {
        /// The scenario's name.
        scenario: String,
        /// Why the model refused.
        source: wcmp_scenario::Error,
    },
    /// Wasmtime's engine or linker could not be set up. This is not a
    /// stage of any scenario, because no scenario has been read yet.
    #[error("setting up Wasmtime: {0}")]
    Setup(String),
}

/// The result of reading a scenario or running it.
pub type Result<T> = core::result::Result<T, Error>;
