//! The compatibility table: a record read as one row per scenario.

use core::fmt;

use crate::record::Record;
use crate::stage::Stage;
use crate::subject::Subject;

/// The columns of the table, in order, each with the subject it shows.
/// The browser leads, because it is the subject that matters most.
const COLUMNS: [(&str, Subject); 3] = [
    ("Browser", Subject::Web),
    ("Native", Subject::Native),
    ("Wasmtime", Subject::Wasmtime),
];

/// A [`Record`] as a person reads it: one row per scenario, and one
/// column per subject.
///
/// The table prints a header line that names the toolchain and the pin,
/// a row of column names, one row per scenario with the stage in each
/// cell, and a footer that counts the passes in each column. The
/// columns are `Browser`, `Native`, and `Wasmtime`, in that order. A
/// cell whose subject has no line in the record holds `-`, and does not
/// count as a pass.
///
/// ```text
/// zena at b2237f7e65847eda43ef1f4094eea77fe225ce0d
///
/// Scenario       Browser  Native  Wasmtime
/// scalar-export  pass     pass    pass
/// strings        link     pass    pass
/// Passes         1/2      2/2     2/2
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    toolchain: String,
    pin: String,
    rows: Vec<(String, [Option<Stage>; 3])>,
}

impl Table {
    /// The table of `record`, with the scenarios in the order the record
    /// first names them.
    pub fn new(record: &Record) -> Self {
        let mut rows: Vec<(String, [Option<Stage>; 3])> = Vec::new();
        for line in &record.lines {
            if rows.iter().all(|(scenario, _)| *scenario != line.scenario) {
                let stages = COLUMNS
                    .map(|(_, subject)| Some(record.line(&line.scenario, subject)?.verdict.stage));
                rows.push((line.scenario.clone(), stages));
            }
        }
        Table {
            toolchain: record.toolchain.clone(),
            pin: record.pin.clone(),
            rows,
        }
    }
}

impl fmt::Display for Table {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut grid = vec![
            core::iter::once("Scenario".to_string())
                .chain(COLUMNS.map(|(name, _)| name.to_string()))
                .collect::<Vec<_>>(),
        ];
        for (scenario, stages) in &self.rows {
            grid.push(
                core::iter::once(scenario.clone())
                    .chain(
                        stages
                            .map(|stage| stage.map_or("-".to_string(), |stage| stage.to_string())),
                    )
                    .collect(),
            );
        }
        let passes = (0..COLUMNS.len()).map(|column| {
            let count = self
                .rows
                .iter()
                .filter(|(_, stages)| stages[column] == Some(Stage::Pass))
                .count();
            format!("{count}/{}", self.rows.len())
        });
        grid.push(
            core::iter::once("Passes".to_string())
                .chain(passes)
                .collect(),
        );

        let widths: Vec<usize> = (0..grid[0].len())
            .map(|column| grid.iter().map(|row| row[column].len()).max().unwrap_or(0))
            .collect();
        writeln!(formatter, "{} at {}", self.toolchain, self.pin)?;
        writeln!(formatter)?;
        for row in &grid {
            let line = row
                .iter()
                .zip(&widths)
                .map(|(cell, width)| format!("{cell:<width$}"))
                .collect::<Vec<_>>()
                .join("  ");
            writeln!(formatter, "{}", line.trim_end())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::Report;
    use crate::verdict::Verdict;

    const PIN: &str = "b2237f7e65847eda43ef1f4094eea77fe225ce0d";

    fn report(scenario: &str, subject: Subject, stage: Stage) -> Report {
        Report {
            scenario: scenario.to_string(),
            subject,
            verdict: Verdict::new(stage, if stage == Stage::Pass { "" } else { "why" }),
        }
    }

    #[wcmp_macros::test]
    fn it_prints_the_pin_a_row_per_scenario_led_by_the_browser_and_a_footer_of_passes() {
        let record = Record::new(
            "zena",
            PIN,
            vec![
                report("strings", Subject::Wasmtime, Stage::Pass),
                report("strings", Subject::Web, Stage::Link),
                report("strings", Subject::Native, Stage::Pass),
                report("scalar-export", Subject::Wasmtime, Stage::Pass),
                report("scalar-export", Subject::Web, Stage::Pass),
                report("scalar-export", Subject::Native, Stage::Pass),
                report("refused", Subject::Wasmtime, Stage::Compile),
                report("refused", Subject::Web, Stage::Compile),
                report("refused", Subject::Native, Stage::Compile),
            ],
        );
        assert_eq!(
            Table::new(&record).to_string(),
            format!(
                "\
zena at {PIN}

Scenario       Browser  Native   Wasmtime
refused        compile  compile  compile
scalar-export  pass     pass     pass
strings        link     pass     pass
Passes         1/3      2/3      2/3
"
            )
        );
    }

    #[wcmp_macros::test]
    fn it_shows_a_dash_for_a_subject_with_no_line_and_counts_no_pass_for_it() {
        let record = Record::new(
            "zena",
            PIN,
            vec![
                report("scalar-export", Subject::Wasmtime, Stage::Pass),
                report("scalar-export", Subject::Native, Stage::Pass),
            ],
        );
        assert_eq!(
            Table::new(&record).to_string(),
            format!(
                "\
zena at {PIN}

Scenario       Browser  Native  Wasmtime
scalar-export  -        pass    pass
Passes         0/1      1/1     1/1
"
            )
        );
    }

    #[wcmp_macros::test]
    fn it_prints_the_header_and_a_footer_of_no_scenarios_for_an_empty_record() {
        let record = Record::new("zena", PIN, Vec::new());
        assert_eq!(
            Table::new(&record).to_string(),
            format!(
                "\
zena at {PIN}

Scenario  Browser  Native  Wasmtime
Passes    0/0      0/0     0/0
"
            )
        );
    }
}
