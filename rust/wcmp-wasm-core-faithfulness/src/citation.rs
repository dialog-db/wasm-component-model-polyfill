//! The citation of an engine defect.

use core::fmt;

/// The citation of a defect of an engine: an issue in the engine's
/// tracker, or a line of the engine's source at a fixed commit.
///
/// A citation is a URL of one of these forms:
///
/// - `https://github.com/<owner>/<repo>/issues/<number>`, an issue.
/// - `https://github.com/<owner>/<repo>/blob/<commit>/<path>#L<line>`, or
///   `#L<line>-L<line>`, a line of the source. The commit is a hexadecimal
///   commit hash, so the line does not move.
/// - `https://issues.chromium.org/issues/<number>` or
///   `https://crbug.com/<number>`, a Chromium issue.
/// - `https://bugzilla.mozilla.org/show_bug.cgi?id=<number>`, a Firefox
///   bug, and `https://bugs.webkit.org/show_bug.cgi?id=<number>`, a WebKit
///   bug.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Citation {
    url: String,
}

impl Citation {
    /// The citation `url`, where it has one of the forms above.
    pub fn parse(url: &str) -> Option<Citation> {
        let cites = github(url)
            || numbered(url, "https://issues.chromium.org/issues/")
            || numbered(url, "https://crbug.com/")
            || numbered(url, "https://bugzilla.mozilla.org/show_bug.cgi?id=")
            || numbered(url, "https://bugs.webkit.org/show_bug.cgi?id=");
        cites.then(|| Citation {
            url: url.to_string(),
        })
    }

    /// The URL of the citation.
    pub fn url(&self) -> &str {
        &self.url
    }
}

impl fmt::Display for Citation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.url)
    }
}

/// Whether `url` is a GitHub issue, or a line of a GitHub source file at a
/// commit.
fn github(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://github.com/") else {
        return false;
    };
    let mut parts = rest.splitn(4, '/');
    let (Some(owner), Some(repo), Some(kind), Some(rest)) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    if owner.is_empty() || repo.is_empty() {
        return false;
    }
    match kind {
        "issues" => is_number(rest),
        "blob" => source_line(rest),
        _ => false,
    }
}

/// Whether `rest`, what follows `blob/` in a GitHub URL, names a commit, a
/// file, and a line or a range of lines.
fn source_line(rest: &str) -> bool {
    let Some((commit, rest)) = rest.split_once('/') else {
        return false;
    };
    let Some((path, lines)) = rest.split_once("#L") else {
        return false;
    };
    let lines_hold = match lines.split_once("-L") {
        Some((first, last)) => is_number(first) && is_number(last),
        None => is_number(lines),
    };
    (7..=40).contains(&commit.len())
        && commit.chars().all(|c| c.is_ascii_hexdigit())
        && !path.is_empty()
        && lines_hold
}

/// Whether `url` is `prefix` followed by a number and nothing else.
fn numbered(url: &str, prefix: &str) -> bool {
    url.strip_prefix(prefix).is_some_and(is_number)
}

/// Whether `text` is a decimal number.
fn is_number(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    fn it_reads_an_issue_and_a_source_line_as_citations() {
        for url in [
            "https://github.com/bytecodealliance/wasmtime/issues/10248",
            "https://github.com/bytecodealliance/wasmtime/blob/0123abc/crates/wasmtime/src/lib.rs#L12",
            "https://github.com/wasmi-labs/wasmi/blob/2970aa8f00/crates/core/src/trap.rs#L1-L20",
            "https://issues.chromium.org/issues/40001",
            "https://crbug.com/12",
            "https://bugzilla.mozilla.org/show_bug.cgi?id=1900000",
            "https://bugs.webkit.org/show_bug.cgi?id=270000",
        ] {
            assert_eq!(
                Citation::parse(url).as_ref().map(Citation::url),
                Some(url),
                "{url} is a citation"
            );
        }
    }

    #[wcmp_macros::test]
    fn it_refuses_what_names_neither_an_issue_nor_a_source_line() {
        for text in [
            "",
            "wasmtime#10248",
            "a known difference",
            "https://example.com/issues/1",
            "https://github.com/bytecodealliance/wasmtime",
            "https://github.com/bytecodealliance/wasmtime/pull/10248",
            "https://github.com/bytecodealliance/wasmtime/issues/",
            "https://github.com/bytecodealliance/wasmtime/issues/12a",
            // A branch moves, so a line on it is no citation.
            "https://github.com/bytecodealliance/wasmtime/blob/main/src/lib.rs#L12",
            "https://github.com/bytecodealliance/wasmtime/blob/0123abc/src/lib.rs",
            "https://crbug.com/",
        ] {
            assert_eq!(Citation::parse(text), None, "{text:?} is no citation");
        }
    }
}
