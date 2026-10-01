//! What one run of a script did.

/// The outcome of each directive of one run of a script: how many ran,
/// and the line and reason of each that failed or was skipped.
#[derive(Clone, Debug, Default)]
pub struct ScriptRun {
    directives: usize,
    failures: Vec<(usize, String)>,
    skipped: Vec<(usize, String)>,
}

impl ScriptRun {
    /// A run that has seen no directive yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Counts a directive that passed.
    pub fn pass(&mut self) {
        self.directives += 1;
    }

    /// Counts the directive on `line` as failed, for `reason`.
    pub fn fail(&mut self, line: usize, reason: String) {
        self.directives += 1;
        self.failures.push((line, reason));
    }

    /// Counts the directive on `line` as skipped: the runtime layer cannot
    /// express it, for `reason`.
    pub fn skip(&mut self, line: usize, reason: String) {
        self.directives += 1;
        self.skipped.push((line, reason));
    }

    /// How many directives the run saw.
    pub fn directives(&self) -> usize {
        self.directives
    }

    /// How many directives passed.
    pub fn passed(&self) -> usize {
        self.directives - self.failures.len() - self.skipped.len()
    }

    /// The line, counted from one, and the reason of each directive that
    /// failed, in the order of the script.
    pub fn failures(&self) -> &[(usize, String)] {
        &self.failures
    }

    /// The line and the reason of each directive that was skipped.
    pub fn skipped(&self) -> &[(usize, String)] {
        &self.skipped
    }
}
