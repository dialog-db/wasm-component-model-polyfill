// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The rules that turn a run into a verdict.

use crate::call::Call;
use crate::error::{Error, Result};
use crate::outcome::Outcome;
use crate::run::Run;
use crate::stage::Stage;
use crate::syntax::quote;
use crate::verdict::Verdict;

/// What a run is judged against: each call with the outcome it must
/// have, if there is one, and the lines the scenario must print.
pub struct Reference<'a> {
    /// Each call, in order, with the outcome it must have. `None`
    /// accepts any outcome.
    pub calls: Vec<(&'a Call, Option<&'a Outcome>)>,
    /// The lines the scenario must print, in order.
    pub output: &'a [String],
}

impl Reference<'_> {
    /// Judge `run`.
    ///
    /// A call that failed where the reference has results is the `call`
    /// stage, and the first such call gives the reason, whatever else
    /// differs. Otherwise the first difference, in call order and then
    /// in the output, is the `mismatch` stage. A run with no difference
    /// passes.
    pub fn judge(&self, run: &Run) -> Result<Verdict> {
        if run.outcomes.len() != self.calls.len() {
            return Err(Error::CallCount {
                expected: self.calls.len(),
                observed: run.outcomes.len(),
            });
        }
        let pairs = || self.calls.iter().zip(&run.outcomes).enumerate();

        for (index, ((call, reference), observed)) in pairs() {
            if let (Some(Outcome::Results(_)), Outcome::Failure(message)) = (reference, observed) {
                let mut reason = format!("call {} `{call}` failed", index + 1);
                if !message.is_empty() {
                    reason.push_str(": ");
                    reason.push_str(message);
                }
                return Ok(Verdict::new(Stage::Call, reason));
            }
        }

        for (index, ((call, reference), observed)) in pairs() {
            let Some(reference) = reference else {
                continue;
            };
            if *reference == observed {
                continue;
            }
            let reason = match (reference, observed) {
                (Outcome::Failure(_), Outcome::Results(_)) => format!(
                    "call {} `{call}` returned {observed} where it had to fail",
                    index + 1
                ),
                _ => format!(
                    "call {} `{call}` returned {observed} where {reference} was expected",
                    index + 1
                ),
            };
            return Ok(Verdict::new(Stage::Mismatch, reason));
        }

        let lines = self.output.len().max(run.output.len());
        for index in 0..lines {
            let expected = self.output.get(index);
            let observed = run.output.get(index);
            if expected == observed {
                continue;
            }
            let line = |line: Option<&String>| match line {
                Some(line) => quote(line, '"'),
                None => "nothing".to_string(),
            };
            let reason = format!(
                "output line {} is {} where {} was expected",
                index + 1,
                line(observed),
                line(expected)
            );
            return Ok(Verdict::new(Stage::Mismatch, reason));
        }

        Ok(Verdict::pass())
    }
}
