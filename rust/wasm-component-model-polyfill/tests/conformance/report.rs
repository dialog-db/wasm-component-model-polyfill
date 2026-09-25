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
#[derive(Clone, Debug)]
pub struct Expectation {
    pub file: String,
    pub line: usize,
    pub category: Category,
}

/// The category a regenerated line carries for a directive the list
/// did not name yet. It is deliberately not a category, so every gate
/// keeps failing until a person reads the failure and picks one.
pub const PLACEHOLDER_CATEGORY: &str = "triage";

/// One line of the list as it is written: the location, the category
/// token as it stands, and the reason after it. The token is not
/// checked here, so that a regeneration can carry the placeholder
/// through and `parse_expectations` can reject it.
struct Line<'a> {
    file: &'a str,
    line: usize,
    category: &'a str,
    reason: &'a str,
}

/// Parse one line of the list. `number` is the line's position in the
/// file, for the error message.
fn parse_line(text: &str, number: usize) -> Result<Line<'_>, String> {
    let mut words = text.splitn(3, ' ');
    let location = words.next().unwrap_or_default();
    let (file, directive_line) = location
        .rsplit_once(':')
        .ok_or_else(|| format!("expected-failures.txt:{number}: no `<path>:<line>` location"))?;
    let line = directive_line.parse().map_err(|_| {
        format!("expected-failures.txt:{number}: `{directive_line}` is not a line number")
    })?;
    Ok(Line {
        file,
        line,
        category: words.next().unwrap_or_default(),
        reason: words.next().unwrap_or_default(),
    })
}

/// Parse the whole expected-failures list. A line whose category is
/// missing, unknown, or still the regeneration's placeholder is an
/// error that names the line.
pub fn parse_expectations(text: &str) -> Result<Vec<Expectation>, String> {
    Ok(parse_entries(text)?
        .into_iter()
        .map(|(_, expectation, _)| expectation)
        .collect())
}

/// Parse an overlay list, one whose every line must fail for one
/// cause: as `parse_expectations`, and a line whose reason does not
/// carry `reason` is an error that names the line.
pub fn parse_overlay(text: &str, reason: &str) -> Result<Vec<Expectation>, String> {
    let entries = parse_entries(text)?;
    if let Some((number, expectation, _)) = entries
        .iter()
        .find(|(_, _, recorded)| !recorded.contains(reason))
    {
        return Err(format!(
            "expected-failures.txt:{number}: the line for `{}:{}` does not carry the reason \
             every line of this list must carry: `{reason}`",
            expectation.file, expectation.line
        ));
    }
    Ok(entries
        .into_iter()
        .map(|(_, expectation, _)| expectation)
        .collect())
}

/// Parse every entry of a list, with its line number and its reason.
fn parse_entries(text: &str) -> Result<Vec<(usize, Expectation, &str)>, String> {
    let mut expectations = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let number = index + 1;
        let parsed = parse_line(line, number)?;
        let token = parsed.category;
        let category = Category::parse(token).ok_or_else(|| {
            let known = Category::ALL
                .iter()
                .map(|category| category.name())
                .collect::<Vec<_>>()
                .join(", ");
            if token == PLACEHOLDER_CATEGORY {
                format!(
                    "expected-failures.txt:{number}: `{token}` is the placeholder a regenerated \
                     line carries: read the failure and replace it with one of {known} in `{line}`"
                )
            } else {
                format!(
                    "expected-failures.txt:{number}: `{token}` is not a category (one of {known}) \
                     in `{line}`"
                )
            }
        })?;
        expectations.push((
            number,
            Expectation {
                file: parsed.file.to_owned(),
                line: parsed.line,
                category,
            },
            parsed.reason,
        ));
    }
    Ok(expectations)
}

/// The run's failures that `base`, another list, does not already
/// name, so that an overlay regenerated from them holds only what
/// fails beyond the list it is layered on. Only the locations of
/// `base` are read, so a line that still carries the placeholder
/// category counts as named.
pub fn beyond(reports: &[FileReport], base: &str) -> Result<Vec<FileReport>, String> {
    let mut named = Vec::new();
    for (index, raw) in base.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parsed = parse_line(line, index + 1)?;
        named.push((parsed.file, parsed.line));
    }
    Ok(reports
        .iter()
        .map(|report| FileReport {
            path: report.path.clone(),
            directives: report.directives,
            failures: report
                .failures
                .iter()
                .filter(|failure| !named.contains(&(report.path.as_str(), failure.line)))
                .map(|failure| Failure {
                    line: failure.line,
                    reason: failure.reason.clone(),
                })
                .collect(),
            expected: Vec::new(),
        })
        .collect())
}

/// A regenerated expected-failures list: the whole file, and what the
/// run did to it.
#[derive(Debug)]
pub struct Regeneration {
    /// The list as it should be written.
    pub text: String,
    /// Lines whose directive still fails. Each kept its category and
    /// its hand-written parenthetical and took the run's reason.
    pub kept: usize,
    /// Lines the run dropped, because their directive passes now, as
    /// they read before.
    pub dropped: Vec<String>,
    /// Lines the run added, with the placeholder category, because the
    /// list did not name the directive.
    pub added: Vec<String>,
}

/// Rewrite the expected-failures list from a run: a directive that
/// still fails keeps its line's category and any hand-written
/// parenthetical and takes the run's reason, a directive that passes
/// now loses its line, and a directive the list does not name arrives
/// with `PLACEHOLDER_CATEGORY`. The header comment block of `current`
/// is carried over, and the entries below it are sorted by path and
/// then by directive line, so a regeneration of an unchanged run is
/// byte-identical to the list it read.
///
/// Two kinds of hand-written text survive the rewrite. A trailing
/// `(…)` group of the recorded reason is a note a person appended, and
/// it is restored after the run's reason, unless the run's reason
/// already ends with the same group, which is how a parenthetical the
/// runtime itself writes — `unsupported component feature: thread
/// built-ins (table extraction)` — is told apart from a note. A
/// recorded reason that ends in the run's own cause behind other
/// leading text — `the invoked export is lifted stackfully:
/// unsupported component feature: stackful asynchronous lifts` against
/// the run's `instantiation error: …: unsupported component feature:
/// stackful asynchronous lifts` — is a sentence a person wrote over an
/// unchanged cause, and it is kept whole. A line refreshes when the
/// cause at the end of the run's reason changes, which is what a
/// change to the failure text means.
pub fn regenerate(current: &str, reports: &[FileReport]) -> Result<Regeneration, String> {
    let mut preamble: Vec<&str> = Vec::new();
    let mut recorded: BTreeMap<(&str, usize), (&str, &str)> = BTreeMap::new();
    for (index, raw) in current.lines().enumerate() {
        let line = raw.trim();
        let number = index + 1;
        if line.is_empty() {
            if recorded.is_empty() {
                preamble.push(raw);
            }
            continue;
        }
        if line.starts_with('#') {
            if !recorded.is_empty() {
                return Err(format!(
                    "expected-failures.txt:{number}: a comment below the first entry has no \
                     place to go in a regenerated list: move it into the header block"
                ));
            }
            preamble.push(raw);
            continue;
        }
        let parsed = parse_line(line, number)?;
        recorded.insert((parsed.file, parsed.line), (parsed.category, parsed.reason));
    }
    while preamble.last().is_some_and(|line| line.trim().is_empty()) {
        preamble.pop();
    }

    let mut failing: BTreeMap<(&str, usize), String> = BTreeMap::new();
    for report in reports {
        for failure in &report.failures {
            failing
                .entry((report.path.as_str(), failure.line))
                .or_insert_with(|| one_line(&failure.reason));
        }
    }

    let mut text = String::new();
    for line in &preamble {
        let _ = writeln!(text, "{line}");
    }
    // The blank line separates the header from the entries, so a list
    // with none, as an overlay can be, ends at its header.
    if !preamble.is_empty() && !failing.is_empty() {
        text.push('\n');
    }
    let mut kept = 0;
    let mut added = Vec::new();
    for ((file, line), reason) in &failing {
        let rendered = match recorded.get(&(*file, *line)) {
            Some((category, recorded_reason)) => {
                kept += 1;
                let reason = merged_reason(recorded_reason, reason);
                format!("{file}:{line} {category} {reason}")
            }
            None => {
                let rendered = format!("{file}:{line} {PLACEHOLDER_CATEGORY} {reason}");
                added.push(rendered.clone());
                rendered
            }
        };
        let _ = writeln!(text, "{rendered}");
    }
    let dropped = recorded
        .iter()
        .filter(|(location, _)| !failing.contains_key(*location))
        .map(|((file, line), (category, reason))| format!("{file}:{line} {category} {reason}"))
        .collect();

    Ok(Regeneration {
        text,
        kept,
        dropped,
        added,
    })
}

/// A failure's reason as one line of the list: a reason that spans
/// lines, as a backtrace in a substrate error does, becomes one line.
fn one_line(reason: &str) -> String {
    reason.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The reason a kept line takes: the run's reason, unless the recorded
/// reason is a sentence a person wrote over the same cause, with the
/// recorded line's hand-written parenthetical restored after it.
fn merged_reason(recorded: &str, live: &str) -> String {
    let recorded = recorded.trim_end();
    let note = annotation(recorded).filter(|note| !live.ends_with(note));
    let body = match note {
        Some(note) => recorded[..recorded.len() - note.len()].trim_end(),
        None => recorded,
    };
    let reason = if keeps_a_written_lead(body, live) {
        body
    } else {
        live
    };
    match note {
        Some(note) if reason.is_empty() => note.to_owned(),
        Some(note) => format!("{reason} {note}"),
        None => reason.to_owned(),
    }
}

/// Whether the recorded reason leads with text a person wrote rather
/// than text the run writes. It does when the recorded reason ends in
/// one of the run's `: `-separated tails — the cause is the one the
/// run reports — and the text before that tail is neither empty nor
/// something the run's own reason begins with.
fn keeps_a_written_lead(recorded: &str, live: &str) -> bool {
    let recorded = unquoted(recorded);
    let live = unquoted(live);
    if recorded == live {
        return false;
    }
    let tails = std::iter::once(live.as_str()).chain(
        live.match_indices(": ")
            .map(|(index, separator)| &live[index + separator.len()..]),
    );
    for tail in tails {
        if tail.is_empty() || !recorded.ends_with(tail) {
            continue;
        }
        let lead = recorded[..recorded.len() - tail.len()].trim_end();
        return !lead.is_empty() && !live.starts_with(lead);
    }
    false
}

/// A reason without the backticks a person puts around a message they
/// quote, so that `the call fails with `no suspend provider`` and the
/// run's `…: no suspend provider` are seen to end in the same cause.
fn unquoted(reason: &str) -> String {
    reason.replace('`', "")
}

/// The trailing parenthetical group of a reason, parentheses included.
/// A group that is the whole reason is the reason itself rather than a
/// note about it, so it does not count.
fn annotation(reason: &str) -> Option<&str> {
    let reason = reason.trim_end();
    if !reason.ends_with(')') {
        return None;
    }
    let bytes = reason.as_bytes();
    let mut depth = 0usize;
    let mut start = None;
    for index in (0..bytes.len()).rev() {
        match bytes[index] {
            b')' => depth += 1,
            b'(' => {
                depth -= 1;
                if depth == 0 {
                    start = Some(index);
                    break;
                }
            }
            _ => {}
        }
    }
    let start = start?;
    if reason[..start].trim_end().is_empty() {
        return None;
    }
    Some(&reason[start..])
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
            let mut components = report.path.splitn(3, '/');
            let first = components.next().unwrap_or_default();
            let second = components.next().unwrap_or_default();
            let corpus = if second == "async" {
                format!("{first}/{second}")
            } else {
                first.to_owned()
            };
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
    async fn it_rejects_the_placeholder_category_a_regeneration_writes() {
        let list = format!("cm/a.wast:12 {PLACEHOLDER_CATEGORY} no instance to invoke\n");
        let err = parse_expectations(&list).unwrap_err();
        assert!(
            err.contains("is the placeholder a regenerated line carries"),
            "{err}"
        );
        assert!(err.contains("cascade"), "{err}");
    }

    #[wcmp_macros::test]
    async fn it_rejects_an_expectation_without_a_location() {
        let err = parse_expectations("cm/a.wast cascade reason\n").unwrap_err();
        assert!(err.contains("no `<path>:<line>` location"), "{err}");
        let err = parse_expectations("cm/a.wast:twelve cascade reason\n").unwrap_err();
        assert!(err.contains("`twelve` is not a line number"), "{err}");
    }

    const CAUSE: &str = "blocking here requires a stack switch";

    #[wcmp_macros::test]
    async fn it_parses_an_overlay_whose_every_line_carries_the_reason() {
        let list = "# comment\n\n\
                    cm/a.wast:12 deferred-feature the call fails with `blocking here requires a \
                    stack switch`\n\
                    cm/a.wast:14 cascade scheduler error: blocking here requires a stack switch\n";
        let expectations = parse_overlay(list, CAUSE).expect("parses");
        assert_eq!(expectations.len(), 2);
        assert_eq!(expectations[1].line, 14);
        assert_eq!(expectations[1].category, Category::Cascade);
    }

    #[wcmp_macros::test]
    async fn it_rejects_an_overlay_line_without_the_reason() {
        let list = "cm/a.wast:12 cascade scheduler error: blocking here requires a stack switch\n\
                    cm/a.wast:14 cascade no instance to invoke\n";
        let err = parse_overlay(list, CAUSE).unwrap_err();
        assert!(err.contains("expected-failures.txt:2"), "{err}");
        assert!(err.contains("`cm/a.wast:14`"), "{err}");
        assert!(err.contains(CAUSE), "{err}");
    }

    #[wcmp_macros::test]
    async fn it_rejects_an_overlay_line_without_a_category_before_its_reason() {
        let err = parse_overlay(
            "cm/a.wast:12 blocking here requires a stack switch\n",
            CAUSE,
        )
        .unwrap_err();
        assert!(err.contains("is not a category"), "{err}");
    }

    #[wcmp_macros::test]
    async fn it_keeps_only_the_failures_beyond_the_base_list() {
        let base = format!(
            "{HEADER}cm/a.wast:12 cascade no instance to invoke\n\
             cm/a.wast:20 {PLACEHOLDER_CATEGORY} expected `1`, got `2`\n"
        );
        let reports = [
            failing(
                "cm/a.wast",
                30,
                &[(12, "no instance to invoke"), (20, "x"), (25, CAUSE)],
            ),
            failing("cm/b.wast", 4, &[(12, CAUSE)]),
        ];
        let beyond = beyond(&reports, &base).expect("reads the base list");
        let lines: Vec<(&str, usize)> = beyond
            .iter()
            .flat_map(|report| {
                report
                    .failures
                    .iter()
                    .map(|failure| (report.path.as_str(), failure.line))
            })
            .collect();
        assert_eq!(lines, [("cm/a.wast", 25), ("cm/b.wast", 12)]);
        assert_eq!(beyond[0].directives, 30);
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

    /// A report whose failures carry the reasons a run would print.
    fn failing(path: &str, directives: usize, failed: &[(usize, &str)]) -> FileReport {
        FileReport {
            path: path.to_owned(),
            directives,
            failures: failed
                .iter()
                .map(|(line, reason)| Failure {
                    line: *line,
                    reason: (*reason).to_owned(),
                })
                .collect(),
            expected: Vec::new(),
        }
    }

    const HEADER: &str =
        "# Directives the polyfill does not pass yet:\n#   <path>:<line> <category> <reason>\n\n";

    #[wcmp_macros::test]
    async fn it_regenerates_an_unchanged_list_byte_for_byte() {
        let list = format!(
            "{HEADER}cm/a.wast:12 cascade no instance to invoke\n\
             cm/a.wast:20 validation the component parsed (the reference does not pass it either)\n\
             wasmtime/b.wast:3 substrate no i31 reference type\n"
        );
        let reports = [
            failing(
                "cm/a.wast",
                30,
                &[(12, "no instance to invoke"), (20, "the component parsed")],
            ),
            failing("wasmtime/b.wast", 4, &[(3, "no i31 reference type")]),
        ];
        let regeneration = regenerate(&list, &reports).expect("regenerates");
        assert_eq!(regeneration.text, list);
        assert_eq!(regeneration.kept, 3);
        assert!(regeneration.dropped.is_empty());
        assert!(regeneration.added.is_empty());
    }

    #[wcmp_macros::test]
    async fn it_regenerates_a_list_without_entries_byte_for_byte() {
        let list = HEADER.trim_end().to_owned() + "\n";
        let reports = [failing("cm/a.wast", 30, &[])];
        let regeneration = regenerate(&list, &reports).expect("regenerates");
        assert_eq!(regeneration.text, list);

        // The first entry a run adds arrives below the blank line.
        let reports = [failing("cm/a.wast", 30, &[(12, "no instance to invoke")])];
        let regeneration = regenerate(&list, &reports).expect("regenerates");
        assert_eq!(
            regeneration.text,
            format!("{HEADER}cm/a.wast:12 {PLACEHOLDER_CATEGORY} no instance to invoke\n")
        );
    }

    #[wcmp_macros::test]
    async fn it_keeps_the_category_and_the_parenthetical_of_a_changed_reason() {
        let list = format!(
            "{HEADER}cm/a.wast:12 validation the component parsed (wasmparser predates the limit)\n"
        );
        let reports = [failing(
            "cm/a.wast",
            30,
            &[(12, "expected rejection `too big`, but the component parsed")],
        )];
        let regeneration = regenerate(&list, &reports).expect("regenerates");
        assert_eq!(
            regeneration.text,
            format!(
                "{HEADER}cm/a.wast:12 validation expected rejection `too big`, but the component \
                 parsed (wasmparser predates the limit)\n"
            )
        );
        assert_eq!(regeneration.kept, 1);
    }

    #[wcmp_macros::test]
    async fn it_does_not_double_a_parenthetical_the_run_itself_writes() {
        let list =
            format!("{HEADER}cm/a.wast:12 deferred-feature thread built-ins (table extraction)\n");
        let reports = [failing(
            "cm/a.wast",
            30,
            &[(
                12,
                "component rejected: thread built-ins (table extraction)",
            )],
        )];
        let regeneration = regenerate(&list, &reports).expect("regenerates");
        assert_eq!(
            regeneration.text,
            format!(
                "{HEADER}cm/a.wast:12 deferred-feature component rejected: thread built-ins \
                 (table extraction)\n"
            )
        );
    }

    #[wcmp_macros::test]
    async fn it_keeps_a_reason_a_person_wrote_over_the_same_cause() {
        let list = format!(
            "{HEADER}cm/a.wast:12 deferred-feature the invoked export is lifted stackfully: \
             unsupported component feature: stackful asynchronous lifts\n\
             cm/a.wast:20 deferred-feature the callee spin-waits, so only its caller can release \
             it: the call fails with `no suspend provider`\n"
        );
        let reports = [failing(
            "cm/a.wast",
            30,
            &[
                (
                    12,
                    "instantiation error: the substrate failed: unsupported component feature: \
                     stackful asynchronous lifts",
                ),
                (20, "scheduler error: no suspend provider"),
            ],
        )];
        let regeneration = regenerate(&list, &reports).expect("regenerates");
        assert_eq!(regeneration.text, list);
        assert_eq!(regeneration.kept, 2);
    }

    #[wcmp_macros::test]
    async fn it_takes_the_runs_reason_when_the_cause_itself_changed() {
        let list = format!(
            "{HEADER}cm/a.wast:12 deferred-feature the invoked export is lifted stackfully: \
             unsupported component feature: stackful asynchronous lifts\n"
        );
        let reports = [failing(
            "cm/a.wast",
            30,
            &[(
                12,
                "unsupported component feature: the `task-cancel` trampoline",
            )],
        )];
        let regeneration = regenerate(&list, &reports).expect("regenerates");
        assert_eq!(
            regeneration.text,
            format!(
                "{HEADER}cm/a.wast:12 deferred-feature unsupported component feature: the \
                 `task-cancel` trampoline\n"
            )
        );
    }

    #[wcmp_macros::test]
    async fn it_takes_the_runs_reason_when_the_run_only_wrapped_the_recorded_one() {
        let list =
            format!("{HEADER}cm/a.wast:12 deferred-feature unsupported feature: the trampoline\n");
        let reports = [failing(
            "cm/a.wast",
            30,
            &[(
                12,
                "component rejected: unsupported feature: the trampoline",
            )],
        )];
        let regeneration = regenerate(&list, &reports).expect("regenerates");
        assert_eq!(
            regeneration.text,
            format!(
                "{HEADER}cm/a.wast:12 deferred-feature component rejected: unsupported feature: \
                 the trampoline\n"
            )
        );
    }

    #[wcmp_macros::test]
    async fn it_keeps_both_a_written_lead_and_a_written_parenthetical() {
        let list = format!(
            "{HEADER}cm/a.wast:12 substrate the core module type is refused before instantiation: \
             non-nullable reference types (the runtime layer has no i31 reference type)\n"
        );
        let reports = [failing(
            "cm/a.wast",
            30,
            &[(12, "component rejected: non-nullable reference types")],
        )];
        let regeneration = regenerate(&list, &reports).expect("regenerates");
        assert_eq!(regeneration.text, list);
    }

    #[wcmp_macros::test]
    async fn it_drops_a_line_whose_directive_passes_now() {
        let list = format!(
            "{HEADER}cm/a.wast:12 cascade no instance to invoke\n\
             cm/a.wast:20 cascade no instance to invoke\n"
        );
        let reports = [failing("cm/a.wast", 30, &[(20, "no instance to invoke")])];
        let regeneration = regenerate(&list, &reports).expect("regenerates");
        assert_eq!(
            regeneration.text,
            format!("{HEADER}cm/a.wast:20 cascade no instance to invoke\n")
        );
        assert_eq!(regeneration.kept, 1);
        assert_eq!(
            regeneration.dropped,
            ["cm/a.wast:12 cascade no instance to invoke"]
        );
    }

    #[wcmp_macros::test]
    async fn it_adds_a_new_failure_with_the_placeholder_category() {
        let list = format!("{HEADER}cm/a.wast:20 cascade no instance to invoke\n");
        let reports = [failing(
            "cm/a.wast",
            30,
            &[(12, "expected `1`, got `2`"), (20, "no instance to invoke")],
        )];
        let regeneration = regenerate(&list, &reports).expect("regenerates");
        assert_eq!(
            regeneration.text,
            format!(
                "{HEADER}cm/a.wast:12 {PLACEHOLDER_CATEGORY} expected `1`, got `2`\n\
                 cm/a.wast:20 cascade no instance to invoke\n"
            )
        );
        assert_eq!(
            regeneration.added,
            [format!(
                "cm/a.wast:12 {PLACEHOLDER_CATEGORY} expected `1`, got `2`"
            )]
        );
        // The list the regeneration wrote is one every gate rejects
        // until a person replaces the placeholder with a category.
        let err = parse_expectations(&regeneration.text).unwrap_err();
        assert!(err.contains(PLACEHOLDER_CATEGORY), "{err}");
    }

    #[wcmp_macros::test]
    async fn it_sorts_the_regenerated_entries_by_path_and_directive_line() {
        let list = HEADER.to_owned();
        let reports = [
            failing("cm/b.wast", 3, &[(9, "nine"), (2, "two")]),
            failing("cm/a.wast", 3, &[(11, "eleven")]),
        ];
        let regeneration = regenerate(&list, &reports).expect("regenerates");
        let entries: Vec<&str> = regeneration
            .text
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(|line| line.split(' ').next().unwrap_or_default())
            .collect();
        assert_eq!(entries, ["cm/a.wast:11", "cm/b.wast:2", "cm/b.wast:9"]);
    }

    #[wcmp_macros::test]
    async fn it_folds_a_reason_that_spans_lines_into_one_entry() {
        let list = HEADER.to_owned();
        let reports = [failing(
            "cm/a.wast",
            3,
            &[(2, "the substrate failed:\n  wasm backtrace:\n   0: 0x1e")],
        )];
        let regeneration = regenerate(&list, &reports).expect("regenerates");
        assert_eq!(
            regeneration.text,
            format!(
                "{HEADER}cm/a.wast:2 {PLACEHOLDER_CATEGORY} the substrate failed: wasm backtrace: \
                 0: 0x1e\n"
            )
        );
    }

    #[wcmp_macros::test]
    async fn it_refuses_to_regenerate_a_list_with_a_comment_below_the_first_entry() {
        let list = format!(
            "{HEADER}cm/a.wast:12 cascade no instance to invoke\n\
             # a note about the next line\n\
             cm/a.wast:20 cascade no instance to invoke\n"
        );
        let reports = [failing("cm/a.wast", 30, &[(12, "no instance to invoke")])];
        let err = regenerate(&list, &reports).unwrap_err();
        assert!(err.contains("move it into the header block"), "{err}");
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
    async fn it_splits_an_async_subdirectory_into_its_own_corpus() {
        let reports = [
            report("x/a.wast", 5, &[], &[]),
            report("x/async/y.wast", 3, &[1], &[(1, Category::DeferredFeature)]),
            // A two-level path that is not `async` stays in the
            // top-level corpus, as `wasmtime/gc/` does.
            report("x/gc/z.wast", 4, &[2], &[(2, Category::Defect)]),
        ];
        let summary = Summary::new(&reports);
        assert!(summary.corpora.contains_key("x/async"));
        assert!(!summary.corpora.contains_key("x/gc"));
        let x = &summary.corpora["x"];
        assert_eq!((x.directives, x.passed), (9, 8));
        assert_eq!(x.expected_in(Category::Defect), 1);
        let x_async = &summary.corpora["x/async"];
        assert_eq!((x_async.directives, x_async.passed), (3, 2));
        assert_eq!(x_async.expected_in(Category::DeferredFeature), 1);
        assert_eq!((summary.total.directives, summary.total.passed), (12, 10));
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
