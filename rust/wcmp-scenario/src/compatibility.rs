// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The compatibility report: where each subject of a run stopped on
//! each scenario, and why.

use core::fmt;
use std::collections::BTreeMap;

use crate::report::Report;
use crate::stage::Stage;
use crate::subject::Subject;
use crate::verdict::Verdict;

/// The subjects of an entry, in order, each with the name the report
/// gives it. The browser leads, because it is the subject that matters
/// most.
const SUBJECTS: [(&str, Subject); 4] = [
    ("Browser", Subject::Web),
    ("Native", Subject::Native),
    ("Wasmi", Subject::Wasmi),
    ("Wasmtime", Subject::Wasmtime),
];

/// A run's reports as a person reads them: one entry per scenario, and
/// under it one line per subject with the stage where it stopped and,
/// before `pass`, the reason.
///
/// The report prints a header line that names the toolchain and the
/// pin, then a list with one entry per scenario in the order of their
/// names. Under each entry come the subjects `Browser`, `Native`, `Wasmi`,
/// and `Wasmtime`, in that order, one per line. `Native` is the polyfill
/// over the Wasmtime backend, and `Wasmi` the polyfill over the Wasmi
/// backend. A line whose stage is
/// `pass` holds only the stage. A line whose stage comes before `pass`
/// also holds the reason in parentheses, whole, as the run observed it.
/// A reason of several lines keeps them all; each line after the first
/// is indented under its subject, so the list stays a list. A subject
/// with no report reads `no report`. A footer counts the passes of each
/// subject out of the scenarios; a subject with no report on a scenario
/// does not pass it.
///
/// ```text
/// zena at b2237f7e65847eda43ef1f4094eea77fe225ce0d
///
/// - async-sleep
///   - Browser: parse (reference type (ref null (module 8)) is not supported)
///   - Native: parse (tags are not supported)
///   - Wasmi: parse (unsupported feature: gc)
///   - Wasmtime: pass
/// - scalar-export
///   - Browser: pass
///   - Native: pass
///   - Wasmi: pass
///   - Wasmtime: pass
///
/// Passes: Browser 1/2, Native 1/2, Wasmi 1/2, Wasmtime 2/2
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compatibility {
    toolchain: String,
    pin: String,
    entries: Vec<(String, [Option<Verdict>; SUBJECTS.len()])>,
}

impl Compatibility {
    /// The report of a run of `toolchain` at the revision `pin` that
    /// observed `reports`. When two reports name the same scenario and
    /// subject, the first one counts.
    pub fn new(toolchain: impl Into<String>, pin: impl Into<String>, reports: &[Report]) -> Self {
        let mut entries: BTreeMap<&str, [Option<Verdict>; SUBJECTS.len()]> = BTreeMap::new();
        for report in reports {
            let verdicts = entries.entry(&report.scenario).or_default();
            let column = SUBJECTS
                .iter()
                .position(|&(_, subject)| subject == report.subject)
                .expect("every subject has a place in the report");
            verdicts[column].get_or_insert_with(|| report.verdict.clone());
        }
        Compatibility {
            toolchain: toolchain.into(),
            pin: pin.into(),
            entries: entries
                .into_iter()
                .map(|(scenario, verdicts)| (scenario.to_string(), verdicts))
                .collect(),
        }
    }
}

impl fmt::Display for Compatibility {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(formatter, "{} at {}", self.toolchain, self.pin)?;
        writeln!(formatter)?;
        for (scenario, verdicts) in &self.entries {
            writeln!(formatter, "- {scenario}")?;
            for ((name, _), verdict) in SUBJECTS.iter().zip(verdicts) {
                write!(formatter, "  - {name}: ")?;
                match verdict {
                    None => writeln!(formatter, "no report")?,
                    Some(verdict) if verdict.stage == Stage::Pass || verdict.reason.is_empty() => {
                        writeln!(formatter, "{}", verdict.stage)?
                    }
                    Some(verdict) => writeln!(
                        formatter,
                        "{} ({})",
                        verdict.stage,
                        verdict.reason.replace('\n', "\n    ")
                    )?,
                }
            }
        }
        if !self.entries.is_empty() {
            writeln!(formatter)?;
        }
        let passes = (0..SUBJECTS.len())
            .map(|column| {
                let count = self
                    .entries
                    .iter()
                    .filter(|(_, verdicts)| {
                        verdicts[column]
                            .as_ref()
                            .is_some_and(|verdict| verdict.stage == Stage::Pass)
                    })
                    .count();
                format!("{} {count}/{}", SUBJECTS[column].0, self.entries.len())
            })
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(formatter, "Passes: {passes}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIN: &str = "b2237f7e65847eda43ef1f4094eea77fe225ce0d";

    fn report(scenario: &str, subject: Subject, verdict: Verdict) -> Report {
        Report {
            scenario: scenario.to_string(),
            subject,
            verdict,
        }
    }

    #[wcmp_macros::test]
    fn it_prints_the_pin_each_subject_in_order_with_its_whole_reason_and_a_footer_of_passes() {
        // A reason longer than any line of the report, which must come
        // through whole.
        let long = format!(
            "component sleeper: instantiation error: {}reference type (ref null (module 8)) is not supported by the runtime layer",
            "the runtime substrate failed to instantiate the component: ".repeat(3)
        );
        // The reports come in no particular order: the report puts the
        // scenarios in the order of their names and the subjects in its
        // own order.
        let reports = [
            report("strings", Subject::Wasmtime, Verdict::pass()),
            report(
                "strings",
                Subject::Native,
                Verdict::new(Stage::Parse, "tags are not supported"),
            ),
            report(
                "strings",
                Subject::Wasmi,
                Verdict::new(Stage::Parse, "unsupported feature: gc"),
            ),
            report(
                "strings",
                Subject::Web,
                Verdict::new(Stage::Link, "no import `wcmp:scenario/host`"),
            ),
            report(
                "async-sleep",
                Subject::Web,
                Verdict::new(Stage::Parse, &long),
            ),
            report("async-sleep", Subject::Native, Verdict::pass()),
            report("async-sleep", Subject::Wasmi, Verdict::pass()),
            report(
                "async-sleep",
                Subject::Wasmtime,
                Verdict::new(
                    Stage::Mismatch,
                    "call 1 returned 4u32 where 3u32 was expected",
                ),
            ),
            report("scalar-export", Subject::Native, Verdict::pass()),
            report("scalar-export", Subject::Web, Verdict::pass()),
            report("scalar-export", Subject::Wasmi, Verdict::pass()),
            report("scalar-export", Subject::Wasmtime, Verdict::pass()),
        ];
        assert_eq!(
            Compatibility::new("zena", PIN, &reports).to_string(),
            format!(
                "\
zena at {PIN}

- async-sleep
  - Browser: parse ({long})
  - Native: pass
  - Wasmi: pass
  - Wasmtime: mismatch (call 1 returned 4u32 where 3u32 was expected)
- scalar-export
  - Browser: pass
  - Native: pass
  - Wasmi: pass
  - Wasmtime: pass
- strings
  - Browser: link (no import `wcmp:scenario/host`)
  - Native: parse (tags are not supported)
  - Wasmi: parse (unsupported feature: gc)
  - Wasmtime: pass

Passes: Browser 1/3, Native 2/3, Wasmi 2/3, Wasmtime 2/3
"
            )
        );
    }

    #[wcmp_macros::test]
    fn it_keeps_every_line_of_a_reason_indented_under_its_subject() {
        let reports = [
            report(
                "refused",
                Subject::Web,
                Verdict::not_compiled("main", 1, "refused.zena:1:1 - Error\n  expected `;`"),
            ),
            report("refused", Subject::Native, Verdict::new(Stage::Compile, "")),
            report("refused", Subject::Wasmi, Verdict::new(Stage::Compile, "")),
            report(
                "refused",
                Subject::Wasmtime,
                Verdict::not_compiled("main", 1, "refused.zena:1:1 - Error\n  expected `;`"),
            ),
        ];
        assert_eq!(
            Compatibility::new("zena", PIN, &reports).to_string(),
            format!(
                "\
zena at {PIN}

- refused
  - Browser: compile (program main did not compile (exit 1): refused.zena:1:1 - Error
      expected `;`)
  - Native: compile
  - Wasmi: compile
  - Wasmtime: compile (program main did not compile (exit 1): refused.zena:1:1 - Error
      expected `;`)

Passes: Browser 0/1, Native 0/1, Wasmi 0/1, Wasmtime 0/1
"
            )
        );
    }

    #[wcmp_macros::test]
    fn it_says_no_report_for_a_subject_with_none_and_counts_no_pass_for_it() {
        let reports = [
            report("scalar-export", Subject::Wasmtime, Verdict::pass()),
            report("scalar-export", Subject::Native, Verdict::pass()),
        ];
        assert_eq!(
            Compatibility::new("zena", PIN, &reports).to_string(),
            format!(
                "\
zena at {PIN}

- scalar-export
  - Browser: no report
  - Native: pass
  - Wasmi: no report
  - Wasmtime: pass

Passes: Browser 0/1, Native 1/1, Wasmi 0/1, Wasmtime 1/1
"
            )
        );
    }

    #[wcmp_macros::test]
    fn it_prints_the_header_and_a_footer_of_no_scenarios_for_a_run_of_none() {
        assert_eq!(
            Compatibility::new("zena", PIN, &[]).to_string(),
            format!(
                "\
zena at {PIN}

Passes: Browser 0/0, Native 0/0, Wasmi 0/0, Wasmtime 0/0
"
            )
        );
    }
}
