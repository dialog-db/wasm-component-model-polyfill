//! One entry of a backend's list of expected failures.

use crate::citation::Citation;

/// A directive of the suite that a backend's engine fails, and what
/// explains it: a defect of the engine, or a limit that the embedding of
/// the engine requires.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpectedFailure {
    path: String,
    line: usize,
    citations: Vec<Citation>,
    reason: String,
}

impl ExpectedFailure {
    /// The failure of the directive on `line` of the script at `path`,
    /// explained by what `citations` name, in the words of `reason`.
    pub fn new(path: String, line: usize, citations: Vec<Citation>, reason: String) -> Self {
        Self {
            path,
            line,
            citations,
            reason,
        }
    }

    /// The path of the script under the root of the test suite.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The line of the directive in the script, counted from one.
    pub fn line(&self) -> usize {
        self.line
    }

    /// What explains the failure, one citation or more.
    pub fn citations(&self) -> &[Citation] {
        &self.citations
    }

    /// What fails, in the list's own words.
    pub fn reason(&self) -> &str {
        &self.reason
    }
}
