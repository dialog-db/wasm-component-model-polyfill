//! The suite's error type.

/// Why a benchmark could not be measured.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The polyfill returned an error while the benchmark was driving
    /// its guest.
    #[error("the polyfill returned an error: {0}")]
    Polyfill(#[from] wcmp::Error),
    /// The benchmark could not be set up or configured: a guest export
    /// is missing, a run control is out of range, or the target has no
    /// clock to measure against.
    #[error("the benchmark cannot run: {0}")]
    Setup(String),
}

/// The result every benchmark body returns.
pub type Result<T> = core::result::Result<T, Error>;
