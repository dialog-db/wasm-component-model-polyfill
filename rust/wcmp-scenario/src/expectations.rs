//! A scenario's expectations file.

use core::fmt;
use core::str::FromStr;

use crate::entry::Entry;
use crate::error::{Error, Result};
use crate::judge::Reference;
use crate::run::Run;
use crate::syntax::{Document, quote};
use crate::verdict::Verdict;

/// A scenario's expectations: the calls to make, in order, and the lines
/// the scenario prints.
///
/// A contributor writes the file by hand, in the format the crate
/// documentation describes. It reads with [`str::parse`] and prints
/// back with [`Display`](fmt::Display).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Expectations {
    /// The calls to make, in order, each with the outcome it must have.
    pub entries: Vec<Entry>,
    /// The lines the scenario prints, in order.
    pub output: Vec<String>,
}

impl Expectations {
    /// Judge a run against these expectations.
    ///
    /// This is how the Wasmtime run is judged, and how a polyfill
    /// subject is judged when the Wasmtime run did not pass. An entry
    /// with no outcome accepts any outcome here. The design leaves the
    /// Wasmtime run to decide such an entry, and a run that did not
    /// pass decides nothing, so a polyfill subject is then held only to
    /// the entries that give an outcome.
    ///
    /// # Errors
    ///
    /// [`Error::CallCount`] when the run does not report one outcome per
    /// entry.
    pub fn judge(&self, run: &Run) -> Result<Verdict> {
        Reference {
            calls: self
                .entries
                .iter()
                .map(|entry| (&entry.call, entry.outcome.as_ref()))
                .collect(),
            output: &self.output,
        }
        .judge(run)
    }
}

impl FromStr for Expectations {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        let document = Document::parse(text)?;
        if let Some((line, _)) = document.stage {
            return Err(Error::Syntax {
                line,
                reason: "a `stage` line belongs in observations, not in expectations".to_string(),
            });
        }
        Ok(Expectations {
            entries: document
                .entries
                .into_iter()
                .map(|(_, entry)| entry)
                .collect(),
            output: document.output,
        })
    }
}

impl fmt::Display for Expectations {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for entry in &self.entries {
            writeln!(formatter, "{entry}")?;
        }
        for line in &self.output {
            writeln!(formatter, "output {}", quote(line, '"'))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call::Call;
    use crate::outcome::Outcome;
    use crate::stage::Stage;
    use crate::value::Value;

    const EXAMPLE: &str = r#"
        # Every form a line can take.
        call main add(1u32, 2u32) -> 3u32
        call typed main local:demo/api#greet("world") -> "hello, world"
        call main pair() -> (true, 'x')
        call main nothing() -> ()
        call main boom() -> fail
        call main boom() -> fail "unreachable"
        call main boom()

        output "tab\there"
    "#;

    fn call(export: &str, arguments: Vec<Value>) -> Call {
        Call {
            component: "main".to_string(),
            export: export.to_string(),
            arguments,
            typed: false,
        }
    }

    #[wcmp_macros::test]
    fn it_reads_every_form_of_a_line() {
        let expectations: Expectations = EXAMPLE.parse().unwrap();
        let outcomes: Vec<_> = expectations
            .entries
            .iter()
            .map(|entry| entry.outcome.clone())
            .collect();

        assert_eq!(
            expectations.entries[0].call,
            call("add", vec![Value::U32(1), Value::U32(2)])
        );
        assert_eq!(
            expectations.entries[1].call,
            Call {
                typed: true,
                ..call(
                    "local:demo/api#greet",
                    vec![Value::String("world".to_string())]
                )
            }
        );
        assert_eq!(
            outcomes,
            [
                Some(Outcome::Results(vec![Value::U32(3)])),
                Some(Outcome::Results(vec![Value::String(
                    "hello, world".to_string()
                )])),
                Some(Outcome::Results(vec![Value::Bool(true), Value::Char('x')])),
                Some(Outcome::Results(vec![])),
                Some(Outcome::Failure(String::new())),
                Some(Outcome::Failure("unreachable".to_string())),
                None,
            ]
        );
        assert_eq!(expectations.output, ["tab\there"]);
    }

    #[wcmp_macros::test]
    fn it_prints_a_file_that_reads_back_the_same() {
        let expectations: Expectations = EXAMPLE.parse().unwrap();
        let printed = expectations.to_string();

        assert_eq!(printed.parse::<Expectations>().unwrap(), expectations);
        assert!(printed.contains("call main boom() -> fail \"unreachable\"\n"));
        assert!(printed.contains("call main boom()\n"));
    }

    #[wcmp_macros::test]
    fn it_reads_every_scalar_type_and_the_escapes() {
        let expectations: Expectations = r#"
            call main f(-8s8, 8u8, -16s16, 16u16, -32s32, 32u32, -64s64, 64u64) -> ()
            call main g(1.5f32, -0f64, NaNf64, inff32, -inff64, false) -> ()
            call main h('\'', '\u{1F600}', "\"\\\n\r\t\0\u{7f}é") -> ()
        "#
        .parse()
        .unwrap();
        let arguments: Vec<Value> = expectations
            .entries
            .iter()
            .flat_map(|entry| entry.call.arguments.clone())
            .collect();

        assert_eq!(
            arguments,
            [
                Value::S8(-8),
                Value::U8(8),
                Value::S16(-16),
                Value::U16(16),
                Value::S32(-32),
                Value::U32(32),
                Value::S64(-64),
                Value::U64(64),
                Value::F32(1.5),
                Value::F64(-0.0),
                Value::F64(f64::NAN),
                Value::F32(f32::INFINITY),
                Value::F64(f64::NEG_INFINITY),
                Value::Bool(false),
                Value::Char('\''),
                Value::Char('😀'),
                Value::String("\"\\\n\r\t\0\u{7f}é".to_string()),
            ]
        );
        assert_eq!(
            expectations.to_string().parse::<Expectations>().unwrap(),
            expectations
        );
    }

    #[wcmp_macros::test]
    fn it_names_the_line_a_mistake_is_on() {
        let cases = [
            ("call main add(1) -> 1u32", 1),
            ("\ncall main add(1u32 -> 1u32", 2),
            ("\n\ncall main add(300u8)", 3),
            ("call main add() -> 1u32 2u32", 1),
            ("call main add", 1),
            ("call main add() ->", 1),
            ("output hello", 1),
            ("output \"open", 1),
            ("output \"\\q\"", 1),
            ("call main f('ab')", 1),
            ("returns main f()", 1),
            ("stage pass", 1),
        ];
        for (text, line) in cases {
            match text.parse::<Expectations>() {
                Err(Error::Syntax { line: actual, .. }) => assert_eq!(actual, line, "{text:?}"),
                other => panic!("{text:?} read as {other:?}"),
            }
        }
    }

    #[wcmp_macros::test]
    fn it_passes_a_run_that_meets_every_expectation() {
        let expectations: Expectations = EXAMPLE.parse().unwrap();
        let run = Run {
            outcomes: vec![
                Outcome::Results(vec![Value::U32(3)]),
                Outcome::Results(vec![Value::String("hello, world".to_string())]),
                Outcome::Results(vec![Value::Bool(true), Value::Char('x')]),
                Outcome::Results(vec![]),
                Outcome::Failure("a different message".to_string()),
                Outcome::Failure(String::new()),
                Outcome::Results(vec![Value::U32(7)]),
            ],
            output: vec!["tab\there".to_string()],
        };

        assert_eq!(expectations.judge(&run), Ok(Verdict::pass()));
    }

    #[wcmp_macros::test]
    fn it_stops_at_call_when_a_call_fails_where_a_result_is_expected() {
        let expectations: Expectations = "
            call main add(1u32, 2u32) -> 4u32
            call main add(1u32, 2u32) -> 3u32
            output \"done\"
        "
        .parse()
        .unwrap();
        let run = Run {
            // The first call differs and the second fails; the failure
            // is the earlier stage.
            outcomes: vec![
                Outcome::Results(vec![Value::U32(3)]),
                Outcome::Failure("wasm trap: unreachable".to_string()),
            ],
            output: vec![],
        };

        let verdict = expectations.judge(&run).unwrap();
        assert_eq!(verdict.stage, Stage::Call);
        assert_eq!(
            verdict.reason,
            "call 2 `main add(1u32, 2u32)` failed: wasm trap: unreachable"
        );
    }

    #[wcmp_macros::test]
    fn it_gives_mismatch_for_a_different_result_a_success_that_had_to_fail_or_a_different_line() {
        let expectations: Expectations = "
            call main add(1u32, 2u32) -> 3u32
            call main boom() -> fail
            output \"done\"
        "
        .parse()
        .unwrap();
        let meets = Run {
            outcomes: vec![
                Outcome::Results(vec![Value::U32(3)]),
                Outcome::Failure(String::new()),
            ],
            output: vec!["done".to_string()],
        };
        let mut result = meets.clone();
        result.outcomes[0] = Outcome::Results(vec![Value::U32(4)]);
        let mut success = meets.clone();
        success.outcomes[1] = Outcome::Results(vec![]);
        let mut line = meets.clone();
        line.output.push("extra".to_string());

        let verdicts = [result, success, line].map(|run| expectations.judge(&run).unwrap());
        assert!(
            verdicts
                .iter()
                .all(|verdict| verdict.stage == Stage::Mismatch)
        );
        assert_eq!(
            verdicts.map(|verdict| verdict.reason),
            [
                "call 1 `main add(1u32, 2u32)` returned 4u32 where 3u32 was expected",
                "call 2 `main boom()` returned () where it had to fail",
                "output line 2 is \"extra\" where nothing was expected",
            ]
        );
    }

    #[wcmp_macros::test]
    fn it_refuses_a_run_with_a_different_number_of_outcomes() {
        let expectations: Expectations = "call main boom()".parse().unwrap();

        assert_eq!(
            expectations.judge(&Run::default()),
            Err(Error::CallCount {
                expected: 1,
                observed: 0
            })
        );
    }
}
