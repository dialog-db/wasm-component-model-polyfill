// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The step where a subject stopped.

use core::fmt;
use core::str::FromStr;

use crate::error::Error;

/// The first step where a subject stopped, in the order a run meets
/// them.
///
/// The order is the order of the variants, so a stage compares as
/// earlier or later than another, and every stage but [`Stage::Pass`]
/// is before `pass`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stage {
    /// The toolchain refused a program. The stage is the same for every
    /// subject, and nothing else runs.
    Compile,
    /// The composition tool refused the components. The stage is the
    /// same for every subject.
    Compose,
    /// Parsing the component failed.
    Parse,
    /// The linker did not supply an import.
    Link,
    /// Instantiation failed.
    Instantiate,
    /// A call failed where its expectation is a result.
    Call,
    /// Every call ran, but a result or an output line differs, or a
    /// call succeeded where it had to fail.
    Mismatch,
    /// Every call and every output line met its expectation.
    Pass,
}

impl Stage {
    /// Every stage, in order.
    pub const ALL: [Stage; 8] = [
        Stage::Compile,
        Stage::Compose,
        Stage::Parse,
        Stage::Link,
        Stage::Instantiate,
        Stage::Call,
        Stage::Mismatch,
        Stage::Pass,
    ];

    /// The stage's name, as the files spell it.
    pub fn name(self) -> &'static str {
        match self {
            Stage::Compile => "compile",
            Stage::Compose => "compose",
            Stage::Parse => "parse",
            Stage::Link => "link",
            Stage::Instantiate => "instantiate",
            Stage::Call => "call",
            Stage::Mismatch => "mismatch",
            Stage::Pass => "pass",
        }
    }
}

impl fmt::Display for Stage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

impl FromStr for Stage {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Stage::ALL
            .into_iter()
            .find(|stage| stage.name() == text)
            .ok_or_else(|| Error::UnknownStage(text.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    fn it_orders_the_stages_as_a_run_meets_them() {
        let names: Vec<_> = Stage::ALL.iter().map(|stage| stage.name()).collect();
        assert_eq!(
            names,
            [
                "compile",
                "compose",
                "parse",
                "link",
                "instantiate",
                "call",
                "mismatch",
                "pass"
            ]
        );
        assert!(Stage::ALL.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[wcmp_macros::test]
    fn it_reads_back_every_stage_it_names() {
        for stage in Stage::ALL {
            assert_eq!(stage.to_string().parse::<Stage>(), Ok(stage));
        }
        assert_eq!(
            "passed".parse::<Stage>(),
            Err(Error::UnknownStage("passed".to_string()))
        );
    }
}
