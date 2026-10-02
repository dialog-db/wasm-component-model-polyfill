// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One way a run differs from the record.

use core::fmt;

use crate::report::Report;
use crate::stage::Stage;
use crate::subject::Subject;
use crate::syntax::quote;
use crate::verdict::Verdict;

/// One way a run differs from the committed record. Each one fails the
/// run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Difference {
    /// The record was made from another revision of the toolchain than
    /// the one the build compiled the scenarios with.
    Pin {
        /// The revision the record's header names.
        recorded: String,
        /// The revision the build compiled with.
        built: String,
    },
    /// A subject stopped at another stage than the record holds, earlier
    /// or later.
    Stage {
        /// The scenario.
        scenario: String,
        /// The subject that ran it.
        subject: Subject,
        /// The stage the record holds.
        recorded: Stage,
        /// Where the subject stopped this time, and why.
        verdict: Verdict,
    },
    /// The record has no line for a subject of a scenario that ran.
    Missing {
        /// The scenario.
        scenario: String,
        /// The subject with no line.
        subject: Subject,
    },
    /// The record has a line for a scenario that did not run, because
    /// it does not exist.
    Unknown(Report),
}

impl fmt::Display for Difference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Difference::Pin { recorded, built } => write!(
                formatter,
                "the record was made from revision {recorded}, but the build compiled with revision {built}"
            ),
            Difference::Stage {
                scenario,
                subject,
                recorded,
                verdict,
            } => {
                write!(
                    formatter,
                    "`{scenario} {subject}` stopped at `{}` where the record holds `{recorded}`",
                    verdict.stage
                )?;
                if !verdict.reason.is_empty() {
                    write!(formatter, ": {}", quote(&verdict.reason, '"'))?;
                }
                Ok(())
            }
            Difference::Missing { scenario, subject } => write!(
                formatter,
                "the record has no line for `{scenario} {subject}`"
            ),
            Difference::Unknown(line) => write!(
                formatter,
                "the record's line `{line}` names no scenario of the run"
            ),
        }
    }
}
