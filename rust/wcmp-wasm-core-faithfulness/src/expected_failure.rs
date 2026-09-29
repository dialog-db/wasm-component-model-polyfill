//! One entry of a backend's list of expected failures.

use crate::citation::Citation;

/// A directive of the suite that a backend's engine fails, and the defect
/// of the engine that explains it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpectedFailure {
    path: String,
    line: usize,
    citation: Citation,
    reason: String,
}

impl ExpectedFailure {
    /// The failure of the directive on `line` of the script at `path`,
    /// explained by the defect `citation` names, in the words of `reason`.
    pub fn new(path: String, line: usize, citation: Citation, reason: String) -> Self {
        Self {
            path,
            line,
            citation,
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

    /// The defect of the engine that explains the failure.
    pub fn citation(&self) -> &Citation {
        &self.citation
    }

    /// What fails, in the list's own words.
    pub fn reason(&self) -> &str {
        &self.reason
    }
}
