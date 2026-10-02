// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Where one subject stopped on one scenario, as one line.

use core::fmt;
use core::str::FromStr;

use crate::error::{Error, Result};
use crate::subject::Subject;
use crate::syntax::{quote, report};
use crate::verdict::Verdict;

/// Where one subject stopped on one scenario, and why.
///
/// A report prints as one line: the scenario, the subject, the stage,
/// and the reason in double quotes when there is one, separated by
/// blanks. The reason takes the escapes of the crate's text format, so
/// a reason of several lines stays on one. The line reads back with
/// [`str::parse`].
///
/// ```text
/// scalar-export native pass
/// string-roundtrip web instantiate "tags are not supported"
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The scenario's name.
    pub scenario: String,
    /// The subject that ran it.
    pub subject: Subject,
    /// Where the subject stopped, and why.
    pub verdict: Verdict,
}

impl fmt::Display for Report {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} {} {}",
            self.scenario, self.subject, self.verdict.stage
        )?;
        if !self.verdict.reason.is_empty() {
            write!(formatter, " {}", quote(&self.verdict.reason, '"'))?;
        }
        Ok(())
    }
}

impl FromStr for Report {
    type Err = Error;

    /// Read one report line. A line that is not one, including a blank
    /// line or a comment, is an [`Error::Syntax`] on line 1.
    fn from_str(line: &str) -> Result<Self> {
        let (scenario, subject, verdict) =
            report(line).map_err(|reason| Error::Syntax { line: 1, reason })?;
        Ok(Report {
            scenario,
            subject,
            verdict,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stage::Stage;

    #[wcmp_macros::test]
    fn it_prints_one_line_that_reads_back() {
        let reports = [
            Report {
                scenario: "scalar-export".to_string(),
                subject: Subject::Native,
                verdict: Verdict::pass(),
            },
            Report {
                scenario: "string-roundtrip".to_string(),
                subject: Subject::Web,
                verdict: Verdict::new(Stage::Instantiate, "tags are \"not\"\nsupported"),
            },
        ];
        let lines: Vec<String> = reports.iter().map(Report::to_string).collect();
        assert_eq!(
            lines,
            [
                "scalar-export native pass",
                r#"string-roundtrip web instantiate "tags are \"not\"\nsupported""#,
            ]
        );
        for (line, report) in lines.iter().zip(&reports) {
            assert_eq!(&line.parse::<Report>().unwrap(), report);
        }
    }

    #[wcmp_macros::test]
    fn it_refuses_a_line_that_is_not_a_report() {
        for line in [
            "",
            "scalar-export",
            "scalar-export browser pass",
            "scalar-export web passed",
            "scalar-export web pass extra",
        ] {
            assert!(
                matches!(line.parse::<Report>(), Err(Error::Syntax { line: 1, .. })),
                "{line:?} read as a report"
            );
        }
    }
}
