//! A backend's list of expected failures.

use std::collections::HashSet;

use crate::citation::Citation;
use crate::expected_failure::ExpectedFailure;

/// The directives of the suite that a backend's engine fails, each with
/// the defect of the engine that explains it.
///
/// The list is a text file, one entry per line:
///
/// ```text
/// # A line that starts with `#` is a comment.
/// <script>:<line> <citation> <reason>
/// ```
///
/// `<script>` is the path of the script under the root of the test suite,
/// and `<line>` the line of the directive, counted from one. `<citation>`
/// is a [`Citation`]: an issue of the engine, or a line of its source at a
/// fixed commit. `<reason>` says what fails. An entry without a citation,
/// or without a reason, is refused, and so is a directive listed twice.
#[derive(Clone, Debug, Default)]
pub struct ExpectedFailures {
    entries: Vec<ExpectedFailure>,
}

impl ExpectedFailures {
    /// The list that `text` holds, or the first thing wrong with it, named
    /// by its line in `text`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut entries = Vec::new();
        let mut seen = HashSet::new();
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let at =
                |message: String| format!("line {} of the expected failures: {message}", index + 1);
            let (directive, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
            let (path, number) = directive
                .rsplit_once(':')
                .ok_or_else(|| at(format!("`{directive}` is not `<script>:<line>`")))?;
            let number = number
                .parse::<usize>()
                .ok()
                .filter(|number| *number > 0 && !path.is_empty())
                .ok_or_else(|| at(format!("`{directive}` is not `<script>:<line>`")))?;
            let rest = rest.trim_start();
            let (citation, reason) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            let citation = Citation::parse(citation).ok_or_else(|| {
                at(format!(
                    "`{directive}` cites no defect of the engine: an entry cites an issue of \
                     the engine or a line of its source at a fixed commit, before its reason"
                ))
            })?;
            let reason = reason.trim();
            if reason.is_empty() {
                return Err(at(format!("`{directive}` gives no reason")));
            }
            if !seen.insert((path.to_string(), number)) {
                return Err(at(format!("`{directive}` is listed twice")));
            }
            entries.push(ExpectedFailure::new(
                path.to_string(),
                number,
                citation,
                reason.to_string(),
            ));
        }
        Ok(Self { entries })
    }

    /// Every entry, in the order of the list.
    pub fn iter(&self) -> impl Iterator<Item = &ExpectedFailure> {
        self.entries.iter()
    }

    /// The entries for the script at `path`.
    pub fn for_script<'a>(&'a self, path: &'a str) -> impl Iterator<Item = &'a ExpectedFailure> {
        self.entries
            .iter()
            .filter(move |entry| entry.path() == path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    fn it_reads_each_entry_with_its_citation_and_reason() {
        let list = ExpectedFailures::parse(
            "# A comment.\n\
             \n\
             address.wast:12 https://github.com/o/r/issues/7 the load traps\n\
             proposals/threads/atomic.wast:3 https://github.com/o/r/blob/abc1234/src/a.rs#L9 a wait\n",
        )
        .expect("the list is well formed");
        let entries = list.iter().collect::<Vec<_>>();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path(), "address.wast");
        assert_eq!(entries[0].line(), 12);
        assert_eq!(
            entries[0].citation().url(),
            "https://github.com/o/r/issues/7"
        );
        assert_eq!(entries[0].reason(), "the load traps");
        assert_eq!(entries[1].path(), "proposals/threads/atomic.wast");
        assert_eq!(list.for_script("address.wast").count(), 1);
    }

    #[wcmp_macros::test]
    fn it_refuses_an_expected_failure_without_a_citation() {
        for text in [
            "address.wast:12 the load traps\n",
            "address.wast:12 wasmtime#12 the load traps\n",
            "address.wast:12\n",
        ] {
            let error = ExpectedFailures::parse(text).expect_err("the entry cites nothing");
            assert!(
                error.contains("line 1") && error.contains("cites no defect of the engine"),
                "{text:?}: {error}"
            );
        }
    }

    #[wcmp_macros::test]
    fn it_refuses_an_entry_without_a_reason_or_a_line_or_twice_listed() {
        for (text, fault) in [
            ("address.wast:12 https://crbug.com/1\n", "gives no reason"),
            (
                "address.wast https://crbug.com/1 why\n",
                "is not `<script>:<line>`",
            ),
            (
                "address.wast:0 https://crbug.com/1 why\n",
                "is not `<script>:<line>`",
            ),
            (
                "a.wast:1 https://crbug.com/1 why\na.wast:1 https://crbug.com/2 why\n",
                "is listed twice",
            ),
        ] {
            let error = ExpectedFailures::parse(text).expect_err("the entry is refused");
            assert!(error.contains(fault), "{text:?}: {error}");
        }
    }
}
