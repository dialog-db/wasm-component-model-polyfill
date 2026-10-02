// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The committed stage of every scenario for every subject, and the
//! gate that holds a run to it.

use core::fmt;
use core::str::FromStr;
use std::collections::BTreeSet;

use crate::difference::Difference;
use crate::error::{Error, Result};
use crate::report::Report;
use crate::subject::Subject;

/// The comments a record prints below its header: what the columns and
/// the stages mean, and when a run fails.
const COMMENTS: &str = "\
# The stage where each subject stopped on each scenario, one line per
# scenario and subject:
#
#   <scenario> <subject> <stage> \"<reason>\"
#
# The subject is `wasmtime`, the Wasmtime run, or the polyfill in the
# browser (`web`), or natively over the Wasmtime backend (`native`) or
# the Wasmi backend (`wasmi`). The reason is the text that says why the
# subject stopped there, for a person to read. A run is held to the
# stage alone. A pass has no reason.
#
# The stages, in the order a run meets them:
#
#   compile      The toolchain refused a program. The stage is the same
#                for every subject, and nothing else runs.
#   compose      The composition tool refused the components. The stage
#                is the same for every subject.
#   parse        `Component::new` failed.
#   link         The linker did not supply an import.
#   instantiate  Instantiation failed.
#   call         A call failed where its expectation is a result.
#   mismatch     Every call ran, but a result or an output line differs,
#                or a call succeeded where it had to fail.
#   pass         Every call and every output line met its expectation.
#
# A run fails when a subject stops at another stage than its line holds,
# earlier or later; when a scenario has no line for a subject, or a line
# names a scenario that does not exist; and when the revision in the
# header is not the one the build compiled with.
";

/// The committed stage of every scenario for every subject, with the
/// revision of the toolchain it was made from.
///
/// A record is a text file. Its first line is the header, `#
/// <toolchain> <revision>`, which names the pin. Every other line is
/// blank, a comment whose first non-blank character is `#`, or one
/// [`Report`] line: the scenario, the subject, the stage, and the
/// reason in double quotes. Each subject of a scenario has its own
/// line, and no two lines name the same scenario and subject.
///
/// ```text
/// # zena b2237f7e65847eda43ef1f4094eea77fe225ce0d
/// scalar-export wasmtime pass
/// scalar-export web parse "component scalar: not supported"
/// scalar-export native pass
/// ```
///
/// The record reads with [`str::parse`] and prints with
/// [`Display`](fmt::Display): the header, comments that explain the
/// columns and the stages, and the lines in order.
/// [`Record::differences`] is the gate that holds a run to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// The toolchain the header names.
    pub toolchain: String,
    /// The revision of the toolchain the record was made from.
    pub pin: String,
    /// One line per scenario and subject.
    pub lines: Vec<Report>,
}

impl Record {
    /// A record of `toolchain` at the revision `pin` that holds
    /// `reports`, ordered by scenario and then by subject.
    pub fn new(toolchain: impl Into<String>, pin: impl Into<String>, reports: Vec<Report>) -> Self {
        let mut lines = reports;
        lines.sort_by(|a, b| (&a.scenario, a.subject).cmp(&(&b.scenario, b.subject)));
        Record {
            toolchain: toolchain.into(),
            pin: pin.into(),
            lines,
        }
    }

    /// The line for `subject` on `scenario`, if the record has one.
    pub fn line(&self, scenario: &str, subject: Subject) -> Option<&Report> {
        self.lines
            .iter()
            .find(|line| line.scenario == scenario && line.subject == subject)
    }

    /// Every way a run differs from the record, in order: the pin, the
    /// stages, the missing lines, and the lines of scenarios that do not
    /// exist. The run passes the gate when there is none.
    ///
    /// `built` is the revision of the toolchain the build compiled the
    /// scenarios with. `reports` holds where each subject that ran
    /// stopped on each scenario that exists. A run on one target has no
    /// report of the other target's subject, so only the stages of the
    /// subjects in `reports` are compared. Every scenario in `reports`
    /// still needs a line for every subject.
    ///
    /// Only the stages are compared. The reasons are for a person to
    /// read, and they change whenever the toolchain or the polyfill does.
    pub fn differences(&self, built: &str, reports: &[Report]) -> Vec<Difference> {
        let mut differences = Vec::new();
        if self.pin != built {
            differences.push(Difference::Pin {
                recorded: self.pin.clone(),
                built: built.to_string(),
            });
        }
        for report in reports {
            if let Some(line) = self.line(&report.scenario, report.subject)
                && line.verdict.stage != report.verdict.stage
            {
                differences.push(Difference::Stage {
                    scenario: report.scenario.clone(),
                    subject: report.subject,
                    recorded: line.verdict.stage,
                    verdict: report.verdict.clone(),
                });
            }
        }
        let scenarios: BTreeSet<&str> = reports
            .iter()
            .map(|report| report.scenario.as_str())
            .collect();
        for scenario in &scenarios {
            for subject in Subject::ALL {
                if self.line(scenario, subject).is_none() {
                    differences.push(Difference::Missing {
                        scenario: scenario.to_string(),
                        subject,
                    });
                }
            }
        }
        for line in &self.lines {
            if !scenarios.contains(line.scenario.as_str()) {
                differences.push(Difference::Unknown(line.clone()));
            }
        }
        differences
    }
}

impl fmt::Display for Record {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(formatter, "# {} {}", self.toolchain, self.pin)?;
        writeln!(formatter, "{COMMENTS}")?;
        for line in &self.lines {
            writeln!(formatter, "{line}")?;
        }
        Ok(())
    }
}

impl FromStr for Record {
    type Err = Error;

    /// Read a record. Lines count from 1.
    ///
    /// # Errors
    ///
    /// [`Error::Syntax`] when the first line is not the header, a line
    /// is not a report, or two lines name the same scenario and subject.
    fn from_str(text: &str) -> Result<Self> {
        let mut lines = text.lines().enumerate();
        let header = lines.next().map(|(_, line)| line).unwrap_or_default();
        let (toolchain, pin) = header
            .strip_prefix('#')
            .and_then(|header| {
                let mut words = header.split_whitespace();
                match (words.next(), words.next(), words.next()) {
                    (Some(toolchain), Some(pin), None) => Some((toolchain, pin)),
                    _ => None,
                }
            })
            .ok_or_else(|| Error::Syntax {
                line: 1,
                reason: format!("expected the header `# <toolchain> <revision>`, found `{header}`"),
            })?;
        let mut record = Record {
            toolchain: toolchain.to_string(),
            pin: pin.to_string(),
            lines: Vec::new(),
        };
        for (index, line) in lines {
            let number = index + 1;
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let report: Report = line.parse().map_err(|error| match error {
                Error::Syntax { reason, .. } => Error::Syntax {
                    line: number,
                    reason,
                },
                other => other,
            })?;
            if record.line(&report.scenario, report.subject).is_some() {
                return Err(Error::Syntax {
                    line: number,
                    reason: format!("a second line for `{} {}`", report.scenario, report.subject),
                });
            }
            record.lines.push(report);
        }
        Ok(record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stage::Stage;
    use crate::verdict::Verdict;

    const PIN: &str = "b2237f7e65847eda43ef1f4094eea77fe225ce0d";

    fn report(scenario: &str, subject: Subject, stage: Stage, reason: &str) -> Report {
        Report {
            scenario: scenario.to_string(),
            subject,
            verdict: Verdict::new(stage, reason),
        }
    }

    /// Where each subject stopped on two scenarios.
    fn run() -> Vec<Report> {
        vec![
            report("scalar", Subject::Wasmtime, Stage::Pass, ""),
            report("scalar", Subject::Web, Stage::Parse, "no tags"),
            report("scalar", Subject::Native, Stage::Instantiate, "no tags"),
            report("scalar", Subject::Wasmi, Stage::Parse, "no gc"),
            report("refused", Subject::Wasmtime, Stage::Compile, "exit 1"),
            report("refused", Subject::Web, Stage::Compile, "exit 1"),
            report("refused", Subject::Native, Stage::Compile, "exit 1"),
            report("refused", Subject::Wasmi, Stage::Compile, "exit 1"),
        ]
    }

    /// The record of [`run`], made at [`PIN`].
    fn record() -> Record {
        Record::new("zena", PIN, run())
    }

    /// [`record`] with the line of `subject` on `scenario` at `stage`.
    fn record_with(scenario: &str, subject: Subject, stage: Stage) -> Record {
        let mut record = record();
        let line = record
            .lines
            .iter_mut()
            .find(|line| line.scenario == scenario && line.subject == subject)
            .unwrap();
        line.verdict.stage = stage;
        record
    }

    #[wcmp_macros::test]
    fn it_prints_a_header_comments_and_ordered_lines_that_read_back() {
        let record = record();
        let text = record.to_string();
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some(format!("# zena {PIN}").as_str()));
        let reports: Vec<_> = lines
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .collect();
        assert_eq!(
            reports,
            [
                "refused wasmtime compile \"exit 1\"",
                "refused web compile \"exit 1\"",
                "refused native compile \"exit 1\"",
                "refused wasmi compile \"exit 1\"",
                "scalar wasmtime pass",
                "scalar web parse \"no tags\"",
                "scalar native instantiate \"no tags\"",
                "scalar wasmi parse \"no gc\"",
            ]
        );
        for stage in Stage::ALL {
            assert!(text.contains(&format!("#   {stage} ")), "{stage}");
        }
        assert_eq!(text.parse::<Record>(), Ok(record));
    }

    #[wcmp_macros::test]
    fn it_refuses_a_record_with_no_header_a_bad_line_or_a_line_twice() {
        let cases = [
            (String::new(), 1),
            ("scalar wasmtime pass\n".to_string(), 1),
            ("# zena\n".to_string(), 1),
            (format!("# zena {PIN} extra\n"), 1),
            (
                format!("# zena {PIN}\n\n# a comment\nscalar browser pass\n"),
                4,
            ),
            (
                format!("# zena {PIN}\nscalar web pass\nscalar web parse \"no\"\n"),
                3,
            ),
        ];
        for (text, expected) in cases {
            match text.parse::<Record>() {
                Err(Error::Syntax { line, .. }) => assert_eq!(line, expected, "{text:?}"),
                other => panic!("{text:?} read as {other:?}"),
            }
        }
    }

    #[wcmp_macros::test]
    fn it_passes_a_run_that_stops_where_the_record_says_whatever_the_reason() {
        assert!(record().differences(PIN, &run()).is_empty());
        let mut reworded = run();
        for report in &mut reworded {
            report.verdict.reason.push_str(", said differently");
        }
        assert!(record().differences(PIN, &reworded).is_empty());
        // One target reports the Wasmtime run and its own subject only.
        let native: Vec<_> = run()
            .into_iter()
            .filter(|report| report.subject != Subject::Web)
            .collect();
        assert!(record().differences(PIN, &native).is_empty());
    }

    #[wcmp_macros::test]
    fn it_fails_a_run_whose_stage_is_one_step_before_or_after_the_record() {
        for (recorded, observed) in [
            // The record is one step later than the run, then one step
            // earlier, including a pass where the record has a failure.
            (Stage::Link, Stage::Parse),
            (Stage::Mismatch, Stage::Pass),
        ] {
            let record = record_with("scalar", Subject::Web, recorded);
            let mut run = run();
            run[1].verdict = Verdict::new(observed, "");
            let differences = record.differences(PIN, &run);
            assert_eq!(
                differences,
                [Difference::Stage {
                    scenario: "scalar".to_string(),
                    subject: Subject::Web,
                    recorded,
                    verdict: Verdict::new(observed, ""),
                }]
            );
            assert_eq!(
                differences[0].to_string(),
                format!("`scalar web` stopped at `{observed}` where the record holds `{recorded}`")
            );
        }
        let later = record_with("scalar", Subject::Native, Stage::Call);
        let earlier = record_with("scalar", Subject::Native, Stage::Link);
        for record in [later, earlier] {
            let differences = record.differences(PIN, &run());
            assert_eq!(differences.len(), 1, "{differences:?}");
            assert_eq!(
                differences[0].to_string(),
                format!(
                    "`scalar native` stopped at `instantiate` where the record holds `{}`: \"no tags\"",
                    record
                        .line("scalar", Subject::Native)
                        .unwrap()
                        .verdict
                        .stage
                )
            );
        }
    }

    #[wcmp_macros::test]
    fn it_fails_a_run_once_for_each_missing_line_and_each_line_of_no_scenario() {
        let mut record = record();
        record
            .lines
            .retain(|line| !(line.scenario == "scalar" && line.subject == Subject::Web));
        let stray = report("no-such-scenario", Subject::Native, Stage::Pass, "");
        record.lines.push(stray.clone());
        let differences = record.differences(PIN, &run());
        assert_eq!(
            differences,
            [
                Difference::Missing {
                    scenario: "scalar".to_string(),
                    subject: Subject::Web,
                },
                Difference::Unknown(stray),
            ]
        );
        assert_eq!(
            differences
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            [
                "the record has no line for `scalar web`",
                "the record's line `no-such-scenario native pass` names no scenario of the run",
            ]
        );
    }

    #[wcmp_macros::test]
    fn it_fails_a_run_built_from_another_revision_and_names_both() {
        let other = "0123456789abcdef0123456789abcdef01234567";
        let differences = record().differences(other, &run());
        assert_eq!(
            differences,
            [Difference::Pin {
                recorded: PIN.to_string(),
                built: other.to_string(),
            }]
        );
        let message = differences[0].to_string();
        assert!(
            message.contains(PIN) && message.contains(other),
            "{message}"
        );
    }
}
