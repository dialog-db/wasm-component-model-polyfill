// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::future::Future;

use crate::clock::Clock;
use crate::{Outcome, Story};

/// One story of the smoke test as it went: the story, its outcome,
/// and how long it took.
#[derive(Debug)]
pub struct Step {
    pub story: &'static Story,
    pub outcome: Outcome,
    /// How long the story took, in milliseconds.
    pub millis: f64,
}

impl Step {
    /// Run a story's body and record its outcome. The body returns the
    /// evidence on success and the reason on failure.
    pub async fn run(
        story: &'static Story,
        body: impl Future<Output = Result<String, String>>,
    ) -> Self {
        let clock = Clock::start();
        let outcome = match body.await {
            Ok(evidence) => Outcome::Passed(evidence),
            Err(reason) => Outcome::Failed(reason),
        };
        Step {
            story,
            outcome,
            millis: clock.elapsed_millis(),
        }
    }

    /// A story that did not run on this target, and why.
    pub fn skipped(story: &'static Story, reason: impl Into<String>) -> Self {
        Step {
            story,
            outcome: Outcome::Skipped(reason.into()),
            millis: 0.0,
        }
    }

    /// How long the story took, as the report prints it.
    pub fn elapsed(&self) -> String {
        if self.millis < 1.0 {
            "<1 ms".to_owned()
        } else {
            format!("{:.0} ms", self.millis)
        }
    }

    /// The story's line of the transcript: its outcome, title, elapsed
    /// time, and the evidence or the reason.
    pub fn line(&self) -> String {
        format!(
            "{:<4} {} ({}): {}",
            self.outcome.label(),
            self.story.title,
            self.elapsed(),
            self.outcome.detail()
        )
    }
}
