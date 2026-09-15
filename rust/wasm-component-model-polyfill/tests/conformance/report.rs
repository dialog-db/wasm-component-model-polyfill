//! The conformance progress report: the expectation categories, the
//! per-file results, and the per-corpus summary the harness prints
//! and writes as JSON.

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Why an expected failure is expected. The vocabulary is fixed so
/// that the progress summary can count failures by cause.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    /// A Component Model feature the polyfill does not implement yet.
    DeferredFeature,
    /// A limit of the runtime layer or of its backend.
    Substrate,
    /// A binary the polyfill accepts and the specification rejects.
    Validation,
    /// A trap or bounds check whose message differs from Wasmtime's,
    /// or that is missing.
    TrapMessage,
    /// A wrong result from a feature the polyfill claims to support.
    Defect,
    /// A consequence of an earlier failure in the same file.
    Cascade,
}

impl Category {
    pub const ALL: [Category; 6] = [
        Category::DeferredFeature,
        Category::Substrate,
        Category::Validation,
        Category::TrapMessage,
        Category::Defect,
        Category::Cascade,
    ];

    pub fn parse(token: &str) -> Option<Self> {
        Category::ALL
            .into_iter()
            .find(|category| category.name() == token)
    }

    pub fn name(self) -> &'static str {
        match self {
            Category::DeferredFeature => "deferred-feature",
            Category::Substrate => "substrate",
            Category::Validation => "validation",
            Category::TrapMessage => "trap-message",
            Category::Defect => "defect",
            Category::Cascade => "cascade",
        }
    }
}

/// One line of `expected-failures.txt`.
#[derive(Debug)]
pub struct Expectation {
    pub file: String,
    pub line: usize,
    pub category: Category,
}

/// Parse the whole expected-failures list. A line whose category is
/// missing or unknown is an error that names the line.
pub fn parse_expectations(text: &str) -> Result<Vec<Expectation>, String> {
    let mut expectations = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let number = index + 1;
        let mut words = line.splitn(3, ' ');
        let location = words.next().unwrap_or_default();
        let (file, directive_line) = location.rsplit_once(':').ok_or_else(|| {
            format!("expected-failures.txt:{number}: no `<path>:<line>` location")
        })?;
        let directive_line = directive_line.parse().map_err(|_| {
            format!("expected-failures.txt:{number}: `{directive_line}` is not a line number")
        })?;
        let token = words.next().unwrap_or_default();
        let category = Category::parse(token).ok_or_else(|| {
            let known = Category::ALL
                .iter()
                .map(|category| category.name())
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "expected-failures.txt:{number}: `{token}` is not a category (one of {known}) \
                 in `{line}`"
            )
        })?;
        expectations.push(Expectation {
            file: file.to_owned(),
            line: directive_line,
            category,
        });
    }
    Ok(expectations)
}

/// One directive the harness could not satisfy.
#[derive(Debug)]
pub struct Failure {
    pub line: usize,
    pub reason: String,
}

/// One corpus file's result: how many directives ran, which failed,
/// and which failures the list expects.
#[derive(Debug)]
pub struct FileReport {
    pub path: String,
    pub directives: usize,
    pub failures: Vec<Failure>,
    pub expected: Vec<Expectation>,
}

impl FileReport {
    /// Failures the list does not expect.
    pub fn unexpected(&self) -> impl Iterator<Item = &Failure> {
        self.failures
            .iter()
            .filter(|failure| !self.expected.iter().any(|e| e.line == failure.line))
    }

    /// Expectations that no failure matched.
    pub fn stale(&self) -> impl Iterator<Item = &Expectation> {
        self.expected
            .iter()
            .filter(|expectation| !self.failures.iter().any(|f| f.line == expectation.line))
    }

    /// Expectations a failure matched.
    pub fn matched(&self) -> impl Iterator<Item = &Expectation> {
        self.expected
            .iter()
            .filter(|expectation| self.failures.iter().any(|f| f.line == expectation.line))
    }

    pub fn passed(&self) -> usize {
        self.directives.saturating_sub(self.failures.len())
    }
}

/// Project another target's results from these results and that
/// target's expectation delta: every delta line that passed here is
/// counted as failing there, under the delta's category. The
/// projection is exact when that target's own run reports no
/// unexpected failure and no stale expectation, which its test suite
/// enforces.
pub fn project(reports: &[FileReport], delta: &[Expectation]) -> Vec<FileReport> {
    reports
        .iter()
        .map(|report| {
            let mut failures: Vec<Failure> = report
                .failures
                .iter()
                .map(|failure| Failure {
                    line: failure.line,
                    reason: failure.reason.clone(),
                })
                .collect();
            let mut expected: Vec<Expectation> = report
                .expected
                .iter()
                .map(|expectation| Expectation {
                    file: expectation.file.clone(),
                    line: expectation.line,
                    category: expectation.category,
                })
                .collect();
            for line in delta.iter().filter(|e| e.file == report.path) {
                if !failures.iter().any(|f| f.line == line.line) {
                    failures.push(Failure {
                        line: line.line,
                        reason: "projected from the target's expectation delta".to_owned(),
                    });
                }
                expected.push(Expectation {
                    file: line.file.clone(),
                    line: line.line,
                    category: line.category,
                });
            }
            FileReport {
                path: report.path.clone(),
                directives: report.directives,
                failures,
                expected,
            }
        })
        .collect()
}

/// The counts for one corpus directory, or for every corpus together.
#[derive(Debug, Default)]
pub struct Tally {
    pub directives: usize,
    pub passed: usize,
    pub expected: BTreeMap<Category, usize>,
    pub unexpected: usize,
    pub stale: usize,
}

impl Tally {
    pub fn add(&mut self, report: &FileReport) {
        self.directives += report.directives;
        self.passed += report.passed();
        for expectation in report.matched() {
            *self.expected.entry(expectation.category).or_default() += 1;
        }
        self.unexpected += report.unexpected().count();
        self.stale += report.stale().count();
    }

    pub fn expected_in(&self, category: Category) -> usize {
        self.expected.get(&category).copied().unwrap_or_default()
    }

    pub fn pass_percent(&self) -> f64 {
        if self.directives == 0 {
            100.0
        } else {
            self.passed as f64 * 100.0 / self.directives as f64
        }
    }
}

/// The progress summary: one tally per corpus directory and a total.
#[derive(Debug)]
pub struct Summary {
    pub corpora: BTreeMap<String, Tally>,
    pub total: Tally,
}

impl Summary {
    pub fn new(reports: &[FileReport]) -> Self {
        let mut corpora: BTreeMap<String, Tally> = BTreeMap::new();
        let mut total = Tally::default();
        for report in reports {
            let corpus = report.path.split('/').next().unwrap_or_default().to_owned();
            corpora.entry(corpus).or_default().add(report);
            total.add(report);
        }
        Summary { corpora, total }
    }

    /// The summary as an aligned text table.
    pub fn table(&self) -> String {
        let mut header = vec![
            "corpus".to_owned(),
            "directives".to_owned(),
            "passed".to_owned(),
            "pass %".to_owned(),
        ];
        header.extend(
            Category::ALL
                .iter()
                .map(|category| category.name().to_owned()),
        );
        header.push("unexpected".to_owned());
        header.push("stale".to_owned());

        let row = |name: &str, tally: &Tally| {
            let mut cells = vec![
                name.to_owned(),
                tally.directives.to_string(),
                tally.passed.to_string(),
                format!("{:.1}", tally.pass_percent()),
            ];
            cells.extend(
                Category::ALL
                    .iter()
                    .map(|category| tally.expected_in(*category).to_string()),
            );
            cells.push(tally.unexpected.to_string());
            cells.push(tally.stale.to_string());
            cells
        };
        let mut rows: Vec<Vec<String>> = self
            .corpora
            .iter()
            .map(|(name, tally)| row(name, tally))
            .collect();
        rows.push(row("total", &self.total));

        let widths: Vec<usize> = (0..header.len())
            .map(|column| {
                rows.iter()
                    .map(|cells| cells[column].len())
                    .chain(std::iter::once(header[column].len()))
                    .max()
                    .unwrap_or_default()
            })
            .collect();
        let render = |cells: &[String]| {
            cells
                .iter()
                .enumerate()
                .map(|(column, cell)| {
                    if column == 0 {
                        format!("{cell:<width$}", width = widths[column])
                    } else {
                        format!("{cell:>width$}", width = widths[column])
                    }
                })
                .collect::<Vec<_>>()
                .join("  ")
        };
        let mut out = String::new();
        let _ = writeln!(out, "{}", render(&header));
        for cells in &rows {
            let _ = writeln!(out, "{}", render(cells));
        }
        out
    }

    /// The summary as JSON, for tooling. Every value is a number or a
    /// corpus directory name, so no escaping is needed.
    pub fn json(&self) -> String {
        fn tally(out: &mut String, tally: &Tally) {
            let _ = write!(
                out,
                "{{\"directives\":{},\"passed\":{},\"pass_percent\":{:.1},\"expected\":{{",
                tally.directives,
                tally.passed,
                tally.pass_percent()
            );
            for (index, category) in Category::ALL.iter().enumerate() {
                let _ = write!(
                    out,
                    "{}\"{}\":{}",
                    if index == 0 { "" } else { "," },
                    category.name(),
                    tally.expected_in(*category)
                );
            }
            let _ = write!(
                out,
                "}},\"unexpected\":{},\"stale\":{}}}",
                tally.unexpected, tally.stale
            );
        }
        let mut out = String::from("{\"corpora\":{");
        for (index, (name, corpus)) in self.corpora.iter().enumerate() {
            let _ = write!(out, "{}\"{name}\":", if index == 0 { "" } else { "," });
            tally(&mut out, corpus);
        }
        out.push_str("},\"total\":");
        tally(&mut out, &self.total);
        out.push_str("}\n");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    async fn it_parses_a_categorized_expectation() {
        let list = "# comment\n\ncm/a.wast:12 cascade no instance to invoke\n";
        let expectations = parse_expectations(list).expect("parses");
        assert_eq!(expectations.len(), 1);
        assert_eq!(expectations[0].file, "cm/a.wast");
        assert_eq!(expectations[0].line, 12);
        assert_eq!(expectations[0].category, Category::Cascade);
    }

    #[wcmp_macros::test]
    async fn it_rejects_an_expectation_without_a_category() {
        let err = parse_expectations("cm/a.wast:12 no instance to invoke\n").unwrap_err();
        assert!(err.contains("`no` is not a category"), "{err}");
        assert!(err.contains("expected-failures.txt:1"), "{err}");
    }

    #[wcmp_macros::test]
    async fn it_rejects_an_expectation_with_an_unknown_category() {
        let err = parse_expectations("cm/a.wast:12 flaky it fails sometimes\n").unwrap_err();
        assert!(err.contains("`flaky` is not a category"), "{err}");
    }

    #[wcmp_macros::test]
    async fn it_rejects_an_expectation_without_a_location() {
        let err = parse_expectations("cm/a.wast cascade reason\n").unwrap_err();
        assert!(err.contains("no `<path>:<line>` location"), "{err}");
        let err = parse_expectations("cm/a.wast:twelve cascade reason\n").unwrap_err();
        assert!(err.contains("`twelve` is not a line number"), "{err}");
    }

    fn report(
        path: &str,
        directives: usize,
        failed: &[usize],
        expected: &[(usize, Category)],
    ) -> FileReport {
        FileReport {
            path: path.to_owned(),
            directives,
            failures: failed
                .iter()
                .map(|line| Failure {
                    line: *line,
                    reason: String::new(),
                })
                .collect(),
            expected: expected
                .iter()
                .map(|(line, category)| Expectation {
                    file: path.to_owned(),
                    line: *line,
                    category: *category,
                })
                .collect(),
        }
    }

    #[wcmp_macros::test]
    async fn it_tallies_passes_and_expected_failures_per_corpus() {
        let reports = [
            report(
                "cm/a.wast",
                10,
                &[3, 4],
                &[(3, Category::Cascade), (4, Category::Defect)],
            ),
            report("cm/b.wast", 5, &[], &[]),
            report(
                "wasmtime/c.wast",
                4,
                &[1, 2],
                &[(1, Category::Substrate), (9, Category::Cascade)],
            ),
        ];
        let summary = Summary::new(&reports);
        let cm = &summary.corpora["cm"];
        assert_eq!((cm.directives, cm.passed), (15, 13));
        assert_eq!(cm.expected_in(Category::Cascade), 1);
        assert_eq!(cm.expected_in(Category::Defect), 1);
        assert_eq!((cm.unexpected, cm.stale), (0, 0));
        let wasmtime = &summary.corpora["wasmtime"];
        assert_eq!((wasmtime.directives, wasmtime.passed), (4, 2));
        assert_eq!(wasmtime.expected_in(Category::Substrate), 1);
        assert_eq!((wasmtime.unexpected, wasmtime.stale), (1, 1));
        assert_eq!((summary.total.directives, summary.total.passed), (19, 15));
    }

    #[wcmp_macros::test]
    async fn it_renders_the_summary_as_a_table_and_as_json() {
        let reports = [report("cm/a.wast", 4, &[2], &[(2, Category::TrapMessage)])];
        let summary = Summary::new(&reports);
        let table = summary.table();
        let rows: Vec<Vec<&str>> = table
            .lines()
            .map(|line| line.split_whitespace().collect())
            .collect();
        assert_eq!(rows[0][..4], ["corpus", "directives", "passed", "pass"]);
        assert_eq!(
            rows[1],
            [
                "cm", "4", "3", "75.0", "0", "0", "0", "1", "0", "0", "0", "0"
            ]
        );
        assert_eq!(rows[2][..4], ["total", "4", "3", "75.0"]);
        let json = summary.json();
        assert!(
            json.starts_with(
                "{\"corpora\":{\"cm\":{\"directives\":4,\"passed\":3,\"pass_percent\":75.0,\
                 \"expected\":{\"deferred-feature\":0,\"substrate\":0,\"validation\":0,\
                 \"trap-message\":1,\"defect\":0,\"cascade\":0},\"unexpected\":0,\"stale\":0}}"
            ),
            "{json}"
        );
        assert!(
            json.contains("\"total\":{\"directives\":4,\"passed\":3"),
            "{json}"
        );
    }
}
