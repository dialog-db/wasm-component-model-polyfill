//! What each generated test checks.

use std::collections::BTreeMap;

use wcmp_wasm_core::{Capability, Engine};

use crate::expected_failures::ExpectedFailures;
use crate::runner;
use crate::script::{SCRIPTS, Script};
use crate::script_run::ScriptRun;
use crate::suite::Suite;

/// Runs the script `source` on `engine`, directive by directive, in a
/// store of its own, and returns what each directive did.
pub async fn run_script(engine: &Engine, source: &str) -> ScriptRun {
    runner::run(engine, source).await
}

/// Runs the script of the suite at `path` on `engine` where the engine
/// declares what the script needs, and holds the run to `expected`, the
/// backend's list of expected failures.
///
/// Panics, naming each directive, when a directive fails that the list
/// does not name (`unexpected:`), or the list names a directive that does
/// not fail (`stale expectation:`). A script the backend does not run
/// passes, unless the list names one of its directives.
pub async fn check_script(engine: &Engine, path: &str, expected: &str) {
    let expected = ExpectedFailures::parse(expected).unwrap_or_else(|error| panic!("{error}"));
    let script =
        Script::find(path).unwrap_or_else(|| panic!("the suite holds no script at `{path}`"));
    let suite =
        Suite::of(script.source()).unwrap_or_else(|error| panic!("{path} does not parse: {error}"));
    let listed = expected.for_script(path).collect::<Vec<_>>();

    if !suite.runs_with(engine.capabilities()) {
        println!("{path} ({suite}): the backend does not run it");
        let stale = listed
            .iter()
            .map(|entry| {
                format!(
                    "stale expectation: {path}:{} {}: the backend does not run the script",
                    entry.line(),
                    entry.citation()
                )
            })
            .collect::<Vec<_>>();
        assert!(stale.is_empty(), "{}", stale.join("\n"));
        return;
    }

    let run = runner::run(engine, script.source()).await;
    let mut problems = Vec::new();
    for (line, reason) in run.failures() {
        if !listed.iter().any(|entry| entry.line() == *line) {
            problems.push(format!("unexpected: {path}:{line} {reason}"));
        }
    }
    for entry in &listed {
        if !run.failures().iter().any(|(line, _)| *line == entry.line()) {
            problems.push(format!(
                "stale expectation: {path}:{} {}",
                entry.line(),
                entry.citation()
            ));
        }
    }
    for (line, reason) in run.skipped() {
        println!("skipped: {path}:{line} {reason}");
    }
    let summary = format!(
        "{path} ({suite}): {} directives, {} passed, {} expected failures, {} skipped",
        run.directives(),
        run.passed(),
        run.failures().len()
            - problems
                .iter()
                .filter(|p| p.starts_with("unexpected"))
                .count(),
        run.skipped().len(),
    );
    println!("{summary}");
    assert!(problems.is_empty(), "{summary}\n{}", problems.join("\n"));
}

/// Checks the backend's list of expected failures, `expected`: each entry
/// cites a defect of the engine and names a script of the suite.
pub fn check_expected_failures(expected: &str) {
    let list = ExpectedFailures::parse(expected).unwrap_or_else(|error| panic!("{error}"));
    let unknown = list
        .iter()
        .filter(|entry| Script::find(entry.path()).is_none())
        .map(|entry| {
            format!(
                "{}:{} names no script of the suite",
                entry.path(),
                entry.line()
            )
        })
        .collect::<Vec<_>>();
    assert!(unknown.is_empty(), "{}", unknown.join("\n"));
}

/// Prints the suite of every script, and whether a backend that declares
/// the capabilities of `engine` runs it, and names each declared
/// capability that no script needs.
pub fn report_suites(engine: &Engine) {
    let declared = engine.capabilities();
    let mut suites = BTreeMap::<String, (Suite, Vec<&str>)>::new();
    for script in SCRIPTS {
        let suite = Suite::of(script.source())
            .unwrap_or_else(|error| panic!("{} does not parse: {error}", script.path()));
        suites
            .entry(suite.to_string())
            .or_insert_with(|| (suite, Vec::new()))
            .1
            .push(script.path());
    }

    println!(
        "faithfulness suite: {} scripts of the pinned test suite; the backend declares {:?}",
        SCRIPTS.len(),
        declared
    );
    for (name, (suite, scripts)) in &suites {
        let runs = if suite.runs_with(declared) {
            "run"
        } else {
            "not run"
        };
        println!("  {name}: {} scripts, {runs}", scripts.len());
        if let Suite::Outside(_) = suite {
            println!("    {}", scripts.join(" "));
        }
        if suite.runs_with(declared) {
            for path in scripts {
                let source = Script::find(path)
                    .map(|script| script.source())
                    .unwrap_or_default();
                let lifted = Suite::lifted_refusals(source, declared)
                    .unwrap_or_else(|error| panic!("{path} does not parse: {error}"));
                for (line, lifting) in lifted {
                    println!(
                        "    skipped: {path}:{line} asserts a refusal that {} lifts",
                        Suite::Capabilities(lifting)
                    );
                }
            }
        }
    }
    for capability in declared
        .iter()
        .filter(|capability| capability.is_wasm_feature())
    {
        let needed = suites.values().any(|(suite, _)| match suite {
            Suite::Capabilities(needed) => needed.contains(capability),
            Suite::Outside(_) => false,
        });
        if !needed {
            println!(
                "  {}: declared, and no script of the pinned test suite needs it",
                Capability::name(capability)
            );
        }
    }
}
