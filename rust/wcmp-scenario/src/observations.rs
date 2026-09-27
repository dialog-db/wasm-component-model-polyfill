//! What the Wasmtime run observed.

use core::fmt;
use core::str::FromStr;

use crate::call::Call;
use crate::error::{Error, Result};
use crate::expectations::Expectations;
use crate::judge::Reference;
use crate::observation::Observation;
use crate::run::Run;
use crate::stage::Stage;
use crate::syntax::{Document, quote};
use crate::verdict::Verdict;

/// What the Wasmtime run observed: its verdict, how each call ended,
/// and the lines the scenario printed.
///
/// The Wasmtime run writes its observations as a build product, and the
/// polyfill subjects read them to be judged. They are never committed.
/// They read with [`str::parse`] and print with
/// [`Display`](fmt::Display), in the format the crate documentation
/// describes, with one `stage` line and an outcome on every call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observations {
    /// The Wasmtime run's verdict against the expectations.
    pub verdict: Verdict,
    /// Each call the run made, in order, and how it ended. Empty when
    /// the run stopped before its calls.
    pub calls: Vec<Observation>,
    /// The lines the scenario printed, in order.
    pub output: Vec<String>,
}

impl Observations {
    /// Judge the Wasmtime run against `expectations`, and keep what it
    /// saw with its verdict.
    ///
    /// # Errors
    ///
    /// [`Error::CallCount`] when the run does not report one outcome per
    /// entry.
    pub fn observe(expectations: &Expectations, run: Run) -> Result<Self> {
        let verdict = expectations.judge(&run)?;
        let calls = expectations
            .entries
            .iter()
            .zip(run.outcomes)
            .map(|(entry, outcome)| Observation {
                call: entry.call.clone(),
                outcome,
            })
            .collect();
        Ok(Observations {
            verdict,
            calls,
            output: run.output,
        })
    }

    /// The observations of a Wasmtime run that stopped at `stage`, for
    /// `reason`, before it made any call.
    ///
    /// # Errors
    ///
    /// [`Error::StoppedLate`] when `stage` is `call` or later, which a
    /// run reaches only by making its calls.
    pub fn stopped(stage: Stage, reason: impl Into<String>) -> Result<Self> {
        if stage >= Stage::Call {
            return Err(Error::StoppedLate(stage));
        }
        Ok(Observations {
            verdict: Verdict::new(stage, reason),
            calls: Vec::new(),
            output: Vec::new(),
        })
    }

    /// Judge a polyfill subject's run.
    ///
    /// When the Wasmtime run passed, the subject must end every call as
    /// the Wasmtime run did, with the same results, and print the same
    /// lines. An entry with no outcome in the expectations is held to
    /// what the Wasmtime run observed for it, like any other.
    ///
    /// When the Wasmtime run did not pass, its observations set nothing,
    /// and the subject is judged against `expectations` alone, as
    /// [`Expectations::judge`] describes: an entry with no outcome then
    /// accepts any outcome.
    ///
    /// # Errors
    ///
    /// [`Error::StaleObservations`] when the Wasmtime run passed but its
    /// calls are not the calls of `expectations`, and
    /// [`Error::CallCount`] when the run does not report one outcome per
    /// entry.
    pub fn judge(&self, expectations: &Expectations, run: &Run) -> Result<Verdict> {
        if !self.verdict.passed() {
            return expectations.judge(run);
        }
        let calls = expectations.entries.len().max(self.calls.len());
        for index in 0..calls {
            let expected = expectations.entries.get(index).map(|entry| &entry.call);
            let observed = self.calls.get(index).map(|observation| &observation.call);
            if expected != observed {
                let spell = |call: Option<&Call>| match call {
                    Some(call) => call.to_string(),
                    None => "nothing".to_string(),
                };
                return Err(Error::StaleObservations {
                    call: index + 1,
                    expected: spell(expected),
                    observed: spell(observed),
                });
            }
        }
        Reference {
            calls: self
                .calls
                .iter()
                .map(|observation| (&observation.call, Some(&observation.outcome)))
                .collect(),
            output: &self.output,
        }
        .judge(run)
    }
}

impl FromStr for Observations {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        let document = Document::parse(text)?;
        let (_, verdict) = document.stage.ok_or(Error::MissingStage)?;
        let calls = document
            .entries
            .into_iter()
            .map(|(line, entry)| match entry.outcome {
                Some(outcome) => Ok(Observation {
                    call: entry.call,
                    outcome,
                }),
                None => Err(Error::Syntax {
                    line,
                    reason: "an observed call needs its outcome".to_string(),
                }),
            })
            .collect::<Result<_>>()?;
        Ok(Observations {
            verdict,
            calls,
            output: document.output,
        })
    }
}

impl fmt::Display for Observations {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "stage {}", self.verdict.stage)?;
        if !self.verdict.reason.is_empty() {
            write!(formatter, " {}", quote(&self.verdict.reason, '"'))?;
        }
        writeln!(formatter)?;
        for observation in &self.calls {
            writeln!(formatter, "{observation}")?;
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
    use crate::outcome::Outcome;
    use crate::value::Value;

    /// A scenario that adds, greets, and calls an export whose outcome
    /// the Wasmtime run decides.
    const EXPECTATIONS: &str = r#"
        call main add(1u32, 2u32) -> 3u32
        call typed main greet("world") -> "hello, world"
        call main boom()
        output "hello, world"
    "#;

    fn expectations() -> Expectations {
        EXPECTATIONS.parse().unwrap()
    }

    fn string(text: &str) -> Value {
        Value::String(text.to_string())
    }

    /// A run that meets the expectations, with `boom` ending as given.
    fn run(boom: Outcome) -> Run {
        Run {
            outcomes: vec![
                Outcome::Results(vec![Value::U32(3)]),
                Outcome::Results(vec![string("hello, world")]),
                boom,
            ],
            output: vec!["hello, world".to_string()],
        }
    }

    fn trap() -> Outcome {
        Outcome::Failure("wasm trap: unreachable".to_string())
    }

    #[wcmp_macros::test]
    fn it_gives_a_failing_wasmtime_run_a_stage_before_pass_and_judges_the_polyfill_against_the_expectations()
     {
        let expectations = expectations();
        // The Wasmtime run returns a wrong sum and prints nothing.
        let mut wasmtime = run(trap());
        wasmtime.outcomes[0] = Outcome::Results(vec![Value::U32(4)]);
        wasmtime.output.clear();
        let observations = Observations::observe(&expectations, wasmtime.clone()).unwrap();

        assert!(observations.verdict.stage < Stage::Pass);
        assert_eq!(observations.verdict.stage, Stage::Mismatch);

        // A subject that matches the expectations passes, though it
        // differs from the Wasmtime run in the sum, the output, and
        // `boom`, whose outcome nothing decided.
        let polyfill = run(Outcome::Results(vec![]));
        assert_eq!(
            observations.judge(&expectations, &polyfill),
            Ok(Verdict::pass())
        );

        // A subject that repeats the Wasmtime run fails the expectations
        // as the Wasmtime run did.
        assert_eq!(
            observations.judge(&expectations, &wasmtime).unwrap().stage,
            Stage::Mismatch
        );
    }

    #[wcmp_macros::test]
    fn it_judges_the_polyfill_against_the_expectations_when_the_wasmtime_run_stopped_early() {
        let expectations = expectations();
        let observations =
            Observations::stopped(Stage::Instantiate, "tags are not supported").unwrap();

        assert_eq!(
            observations.judge(&expectations, &run(trap())),
            Ok(Verdict::pass())
        );
        let mut polyfill = run(trap());
        polyfill.outcomes[1] = Outcome::Failure(String::new());
        assert_eq!(
            observations.judge(&expectations, &polyfill).unwrap().stage,
            Stage::Call
        );
    }

    #[wcmp_macros::test]
    fn it_judges_the_polyfill_against_the_wasmtime_results_and_output_when_the_wasmtime_run_passes()
    {
        let expectations = expectations();
        let observations = Observations::observe(&expectations, run(trap())).unwrap();
        assert_eq!(observations.verdict, Verdict::pass());

        assert_eq!(
            observations.judge(&expectations, &run(trap())),
            Ok(Verdict::pass())
        );

        let mut greeting = run(trap());
        greeting.outcomes[1] = Outcome::Results(vec![string("hello, there")]);
        let mut output = run(trap());
        output.output = vec!["hello,  world".to_string()];
        for polyfill in [greeting, output] {
            assert_eq!(
                observations.judge(&expectations, &polyfill).unwrap().stage,
                Stage::Mismatch
            );
        }
    }

    #[wcmp_macros::test]
    fn it_gives_mismatch_when_one_result_changes_in_the_observations_of_a_passing_scenario() {
        let expectations = expectations();
        let observations = Observations::observe(&expectations, run(trap())).unwrap();
        // The file the Wasmtime run writes, with one result changed.
        let written = observations.to_string();
        assert!(written.contains("call main add(1u32, 2u32) -> 3u32\n"));
        let changed: Observations = written
            .replace("add(1u32, 2u32) -> 3u32", "add(1u32, 2u32) -> 5u32")
            .parse()
            .unwrap();
        assert_ne!(changed, observations);

        // A subject that meets the expectations now differs from the
        // observations it is judged against.
        let verdict = changed.judge(&expectations, &run(trap())).unwrap();
        assert_eq!(verdict.stage, Stage::Mismatch);
        assert_eq!(
            verdict.reason,
            "call 1 `main add(1u32, 2u32)` returned 3u32 where 5u32 was expected"
        );
    }

    #[wcmp_macros::test]
    fn it_holds_an_entry_with_no_outcome_to_what_the_wasmtime_run_observed() {
        let expectations = expectations();

        // The Wasmtime run's `boom` traps: the subject must fail too.
        let failed = Observations::observe(&expectations, run(trap())).unwrap();
        assert_eq!(
            failed.judge(&expectations, &run(Outcome::Failure(String::new()))),
            Ok(Verdict::pass())
        );
        assert_eq!(
            failed
                .judge(&expectations, &run(Outcome::Results(vec![])))
                .unwrap()
                .stage,
            Stage::Mismatch
        );

        // The Wasmtime run's `boom` returns 7u32: the subject must too.
        let seven = Outcome::Results(vec![Value::U32(7)]);
        let returned = Observations::observe(&expectations, run(seven.clone())).unwrap();
        assert_eq!(returned.verdict, Verdict::pass());
        assert_eq!(
            returned.judge(&expectations, &run(seven)),
            Ok(Verdict::pass())
        );
        assert_eq!(
            returned
                .judge(&expectations, &run(Outcome::Results(vec![Value::U32(8)])))
                .unwrap()
                .stage,
            Stage::Mismatch
        );
        assert_eq!(
            returned.judge(&expectations, &run(trap())).unwrap().stage,
            Stage::Call
        );
    }

    #[wcmp_macros::test]
    fn it_prints_observations_that_read_back_the_same() {
        let expectations = expectations();
        let passing = Observations::observe(&expectations, run(trap())).unwrap();
        let mut wrong = run(trap());
        wrong.outcomes[0] = Outcome::Failure("out of \"fuel\"".to_string());
        let failing = Observations::observe(&expectations, wrong).unwrap();
        let stopped = Observations::stopped(Stage::Link, "no import `wasi:cli/stdout`").unwrap();

        assert_eq!(
            failing.to_string().lines().next(),
            Some(r#"stage call "call 1 `main add(1u32, 2u32)` failed: out of \"fuel\"""#)
        );
        for observations in [passing, failing, stopped] {
            assert_eq!(
                observations.to_string().parse::<Observations>().unwrap(),
                observations
            );
        }
    }

    #[wcmp_macros::test]
    fn it_refuses_observations_without_a_stage_or_an_outcome() {
        assert_eq!(
            "call main add(1u32, 2u32) -> 3u32".parse::<Observations>(),
            Err(Error::MissingStage)
        );
        assert!(matches!(
            "stage pass\ncall main boom()".parse::<Observations>(),
            Err(Error::Syntax { line: 2, .. })
        ));
        assert!(matches!(
            "stage pass\nstage call".parse::<Observations>(),
            Err(Error::Syntax { line: 2, .. })
        ));
        assert_eq!(
            Observations::stopped(Stage::Mismatch, ""),
            Err(Error::StoppedLate(Stage::Mismatch))
        );
    }

    #[wcmp_macros::test]
    fn it_refuses_the_observations_of_another_version_of_the_scenario() {
        let expectations = expectations();
        let observations = Observations::observe(&expectations, run(trap())).unwrap();
        let mut edited = expectations.clone();
        edited.entries[1].call.typed = false;
        let mut longer = expectations.clone();
        longer.entries.push(longer.entries[0].clone());

        assert_eq!(
            observations.judge(&edited, &run(trap())),
            Err(Error::StaleObservations {
                call: 2,
                expected: "main greet(\"world\")".to_string(),
                observed: "typed main greet(\"world\")".to_string(),
            })
        );
        assert!(matches!(
            observations.judge(&longer, &run(trap())),
            Err(Error::StaleObservations { call: 4, .. })
        ));
    }
}
