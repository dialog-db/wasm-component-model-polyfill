// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/// How one step of the smoke test went.
#[derive(Debug)]
pub enum Outcome {
    /// The step ran and observed what it expected. Carries the evidence.
    Passed(String),
    /// The step ran and observed something else, or errored. Carries why.
    Failed(String),
    /// The step did not run on this target. Carries the reason.
    Skipped(String),
}

impl Outcome {
    pub fn label(&self) -> &'static str {
        match self {
            Outcome::Passed(_) => "ok",
            Outcome::Failed(_) => "FAIL",
            Outcome::Skipped(_) => "skip",
        }
    }

    pub fn detail(&self) -> &str {
        match self {
            Outcome::Passed(detail) | Outcome::Failed(detail) | Outcome::Skipped(detail) => detail,
        }
    }
}
