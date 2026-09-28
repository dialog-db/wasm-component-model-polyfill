//! The polyfill subjects of the Zena toolchain compatibility tests: each
//! compiled Zena scenario run through the polyfill, in the browser on
//! `wasm32-unknown-unknown` (the `web` subject) and natively (the
//! `native` subject).
//!
//! The build compiles every scenario under `tests/zena/scenarios` with
//! the pinned Zena toolchain, runs each one through Wasmtime, and packs
//! the expectations, the compiled programs, and the Wasmtime run's
//! observations into one bundle. `WCMP_ZENA_SCENARIOS` names the bundle
//! when the tests compile, and the tests embed it, so the browser lane
//! reads the same bytes as the native one. The scenarios are found in
//! the bundle, so a new scenario needs no code here.
//!
//! For each scenario the runner (`zena/runner.rs`) parses every
//! component with `Component::new`, links and instantiates it through
//! the polyfill's `Linker` with the test host functions
//! (`zena/host.rs`) and the scenario's run-time links, each exporter
//! before its importer, and makes each call of the expectations, keeping
//! the lines the scenario prints. A call goes through `Func::call` with
//! `Val` arguments, or through `TypedFunc` when the expectations mark
//! it as typed. The runner makes typed calls for a closed set of
//! signatures of scalars and strings, so a scenario still needs no
//! Rust code of its own. The scenario model judges the subject: against
//! the Wasmtime run's observations when that run passed, and against
//! the expectations otherwise. Each subject reports a stage and the
//! text that says why, as a `Report` whose line is the scenario, the
//! subject, the stage, and the reason.
//!
//! A scenario whose wiring asks for a composition reaches the runner
//! as one component: the build composed its components with `wac` into
//! one under the importer's name. When `wac` refused them, the build
//! kept its exit status and output instead, and every subject stops at
//! `compose` before any component runs.
//!
//! Where a subject stops is an outcome, not a failure, so no test
//! asserts that a scenario passes. The committed record,
//! `tests/zena/record.txt`, holds the stage of every scenario for every
//! subject, and the gate fails the run when a stage here differs from
//! it in either direction, when a scenario has no line for a subject or
//! a line names no scenario, and when the record was made from another
//! Zena revision than the build compiled with. It compares the stages
//! only; the reasons are for a person. Each target holds the Wasmtime
//! stage and its own polyfill subject to the record. The tests also
//! fail when the bundle or a scenario cannot be read or judged, which
//! is a fault of the build or of the runner. The engine has the default
//! configuration, so the suspend provider is on.
//!
//! One target cannot run the other target's subject, so no lane alone
//! holds every line of the record. `tests zena` runs the browser lane's
//! scenario run first and keeps what it prints: each report on a line
//! of its own after `PRINTED`. It then runs, natively,
//! `it_holds_all_three_subjects_to_the_record_and_prints_the_table`,
//! which reads those lines from the file `WCMP_ZENA_WEB_RUN` names,
//! adds the Wasmtime and native reports, prints the compatibility table
//! to the file `WCMP_ZENA_TABLE` names, and holds all three subjects to
//! the record. `tests zena regenerate` sets `WCMP_ZENA_REGENERATE` to a
//! file, and the test writes the record of the run there instead of
//! holding the run to the committed one. The nextest profiles leave
//! that test out of every other lane.

#![cfg(test)]

#[path = "zena/bundle.rs"]
mod bundle;
#[path = "zena/host.rs"]
mod host;
#[path = "zena/runner.rs"]
mod runner;

use wasm_component_model_polyfill::{Component, Engine, Instance, Linker, Store, Val};
use wcmp_macros::component;
use wcmp_scenario::{
    Call, Difference, Observations, Outcome, Record, Report, Stage, Subject, Value, Verdict,
};

use bundle::{Program, Scenario};
use host::Host;
use runner::PolyfillRun;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// Every Zena scenario, as the build left it. The flake's test archives
/// set the variable; see `tests/zena/bundle.sh` for the format.
const BUNDLE: &[u8] = include_bytes!(env!(
    "WCMP_ZENA_SCENARIOS",
    "the Zena scenario tests embed the bundle the build writes; run them through `tests native` or `tests web`"
));

/// The scenarios of `tests/zena/record-check`, built, run through
/// Wasmtime, and bundled like every scenario: [`REFUSED`] and
/// [`UNPLUGGED`]. Neither is a scenario of the record.
const RECORD_CHECK: &[u8] = include_bytes!(env!(
    "WCMP_ZENA_RECORD_CHECK",
    "the Zena scenario tests embed the bundle the build writes; run them through `tests native` or `tests web`"
));

/// The record-check scenario whose one program Zena refuses.
const REFUSED: &str = "refused";

/// The record-check scenario whose composition `wac` refuses: its
/// exporter exports nothing its importer imports.
const UNPLUGGED: &str = "unplugged";

/// The committed record: the stage of every Zena scenario for every
/// subject, and the Zena revision it was made from.
const RECORD: &str = include_str!("zena/record.txt");

/// The name of scenario 1, the smallest program with an export.
const SCALAR_EXPORT: &str = "scalar-export";

/// The name of scenario 2, a string in and a string out.
const STRING_ROUNDTRIP: &str = "string-roundtrip";

/// Every Zena scenario in the bundle.
fn zena_scenarios() -> Vec<Scenario> {
    bundle::scenarios(BUNDLE).unwrap_or_else(|error| panic!("the Zena bundle: {error}"))
}

/// What a report line printed by
/// `it_runs_every_zena_scenario_and_reports_a_stage_and_its_reason`
/// starts with, so `tests zena` can pick the browser's lines out of
/// the test runner's output.
const PRINTED: &str = "zena report: ";

/// Print `line` where the test runner shows it: to standard output
/// natively, and to the console in the browser, whose standard output
/// goes nowhere.
fn say(line: &str) {
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_test::console_log!("{line}");
    #[cfg(not(target_arch = "wasm32"))]
    println!("{line}");
}

fn polyfill() -> PolyfillRun {
    PolyfillRun::new().unwrap_or_else(|error| panic!("the polyfill run: {error}"))
}

/// Run `scenario` through the polyfill on this target and report where
/// it stopped.
async fn report(polyfill: &PolyfillRun, scenario: &Scenario) -> Report {
    let verdict = polyfill
        .run(scenario)
        .await
        .unwrap_or_else(|error| panic!("scenario {}: {error}", scenario.name));
    Report {
        scenario: scenario.name.clone(),
        subject: Subject::polyfill(),
        verdict,
    }
}

#[wcmp_macros::test]
async fn it_runs_every_zena_scenario_and_reports_a_stage_and_its_reason() {
    let polyfill = polyfill();
    let mut reports = Vec::new();
    for scenario in zena_scenarios() {
        let report = report(&polyfill, &scenario).await;
        say(&format!("{PRINTED}{report}"));
        reports.push(report);
    }
    assert!(
        reports
            .iter()
            .any(|report| report.scenario == SCALAR_EXPORT),
        "scenario 1, `{SCALAR_EXPORT}`, is not in the bundle"
    );
    for report in &reports {
        assert_eq!(report.to_string().parse::<Report>().as_ref(), Ok(report));
        assert!(
            report.verdict.passed() || !report.verdict.reason.is_empty(),
            "`{report}` stopped before `pass` and gives no reason"
        );
    }
}

#[wcmp_macros::test]
async fn it_reports_a_mismatch_when_one_result_of_a_passing_scenario_changes() {
    let polyfill = polyfill();
    let mut tampered = Vec::new();
    for scenario in zena_scenarios() {
        if !scenario.observations.verdict.passed()
            || !report(&polyfill, &scenario).await.verdict.passed()
        {
            continue;
        }
        let Some(changed) = with_one_result_changed(&scenario.observations) else {
            continue;
        };
        let scenario = Scenario {
            observations: changed,
            ..scenario
        };
        let report = report(&polyfill, &scenario).await;
        assert_eq!(report.verdict.stage, Stage::Mismatch, "{report}");
        tampered.push(scenario.name);
    }
    if tampered.is_empty() {
        println!(
            "no Zena scenario passes on `{}` with a result to change",
            Subject::polyfill()
        );
    } else {
        println!(
            "`{}` reports `mismatch` for changed observations of: {}",
            Subject::polyfill(),
            tampered.join(", ")
        );
    }
}

#[wcmp_macros::test]
async fn it_gives_each_typed_call_the_outcome_of_the_same_untyped_call() {
    let polyfill = polyfill();
    let mut typed = Vec::new();
    for scenario in zena_scenarios() {
        let pairs = typed_pairs(&scenario.expectations);
        if pairs.is_empty() {
            continue;
        }
        typed.push(scenario.name.clone());
        // The Wasmtime run's observations hold its calls only when it
        // reached them, and so does this subject's run.
        let wasmtime: Vec<Outcome> = scenario
            .observations
            .calls
            .iter()
            .map(|observation| observation.outcome.clone())
            .collect();
        let observed = polyfill
            .observe(&scenario)
            .await
            .unwrap_or_else(|error| panic!("scenario {}: {error}", scenario.name))
            .ok()
            .map(|run| run.outcomes);
        let subjects = [
            (
                Subject::Wasmtime,
                (!wasmtime.is_empty()).then_some(wasmtime),
            ),
            (Subject::polyfill(), observed),
        ];
        for (subject, outcomes) in subjects {
            let Some(outcomes) = outcomes else {
                continue;
            };
            for &(untyped, typed) in &pairs {
                assert_eq!(
                    outcomes[typed],
                    outcomes[untyped],
                    "`{subject}` on {}: `{}` ended as {} where `{}` ended as {}",
                    scenario.name,
                    scenario.expectations.entries[typed].call,
                    outcomes[typed],
                    scenario.expectations.entries[untyped].call,
                    outcomes[untyped]
                );
            }
        }
    }
    for name in [SCALAR_EXPORT, STRING_ROUNDTRIP] {
        assert!(
            typed.iter().any(|typed| typed == name),
            "scenario `{name}` makes no typed call beside the same untyped call"
        );
    }
}

/// For each typed call in `expectations` that has an untyped twin, a
/// call of the same export with the same arguments: the index of the
/// untyped call and the index of the typed one.
fn typed_pairs(expectations: &wcmp_scenario::Expectations) -> Vec<(usize, usize)> {
    let calls = || {
        expectations
            .entries
            .iter()
            .map(|entry| &entry.call)
            .enumerate()
    };
    calls()
        .filter(|(_, call)| call.typed)
        .filter_map(|(typed, call)| {
            calls()
                .find(|(_, other)| !other.typed && same_call(call, other))
                .map(|(untyped, _)| (untyped, typed))
        })
        .collect()
}

/// Whether `a` and `b` call the same export with the same arguments,
/// typed or not.
fn same_call(a: &Call, b: &Call) -> bool {
    a.component == b.component && a.export == b.export && a.arguments == b.arguments
}

/// `observations` with the first result of the first call that returned
/// one changed to another value of the same type, or `None` when no
/// call returned a result.
fn with_one_result_changed(observations: &Observations) -> Option<Observations> {
    let mut changed = observations.clone();
    let result =
        changed
            .calls
            .iter_mut()
            .find_map(|observation| match &mut observation.outcome {
                Outcome::Results(results) => results.first_mut(),
                Outcome::Failure(_) => None,
            })?;
    let other = match &*result {
        Value::Bool(value) => Value::Bool(!*value),
        Value::S8(value) => Value::S8(value.wrapping_add(1)),
        Value::U8(value) => Value::U8(value.wrapping_add(1)),
        Value::S16(value) => Value::S16(value.wrapping_add(1)),
        Value::U16(value) => Value::U16(value.wrapping_add(1)),
        Value::S32(value) => Value::S32(value.wrapping_add(1)),
        Value::U32(value) => Value::U32(value.wrapping_add(1)),
        Value::S64(value) => Value::S64(value.wrapping_add(1)),
        Value::U64(value) => Value::U64(value.wrapping_add(1)),
        Value::F32(value) if value.is_nan() => Value::F32(0.0),
        Value::F32(value) => Value::F32(f32::NAN.copysign(*value)),
        Value::F64(value) if value.is_nan() => Value::F64(0.0),
        Value::F64(value) => Value::F64(f64::NAN.copysign(*value)),
        Value::Char(value) => Value::Char(if *value == 'a' { 'b' } else { 'a' }),
        Value::String(value) => Value::String(format!("{value}!")),
    };
    *result = other;
    Some(changed)
}

/// Where the Wasmtime run and this target's polyfill subject stopped on
/// every scenario in `bundle`: the Wasmtime stage from the observations
/// the build wrote, and the polyfill's from a run here.
async fn subjects(bundle: &[u8]) -> Vec<Report> {
    let polyfill = polyfill();
    let mut reports = Vec::new();
    for scenario in
        bundle::scenarios(bundle).unwrap_or_else(|error| panic!("a Zena bundle: {error}"))
    {
        reports.push(Report {
            scenario: scenario.name.clone(),
            subject: Subject::Wasmtime,
            verdict: scenario.observations.verdict.clone(),
        });
        reports.push(report(&polyfill, &scenario).await);
    }
    reports
}

/// The revision of the toolchain the build compiled the scenarios with,
/// which it takes from the flake lock.
fn built_revision() -> String {
    bundle::revision(BUNDLE).unwrap_or_else(|error| panic!("the Zena bundle: {error}"))
}

/// The committed record.
fn committed_record() -> Record {
    RECORD
        .parse()
        .unwrap_or_else(|error| panic!("tests/zena/record.txt: {error}"))
}

/// The polyfill subject of the other target.
fn other_polyfill() -> Subject {
    match Subject::polyfill() {
        Subject::Web => Subject::Native,
        _ => Subject::Web,
    }
}

/// A record at `pin` that holds `reports` and, for the other target's
/// polyfill subject, which this target cannot run, a copy of this
/// target's line. The gate compares no stage of that subject here, so
/// the record matches the run.
fn record_of(pin: &str, reports: &[Report]) -> Record {
    let mut lines = reports.to_vec();
    lines.extend(
        reports
            .iter()
            .filter(|report| report.subject == Subject::polyfill())
            .map(|report| Report {
                subject: other_polyfill(),
                ..report.clone()
            }),
    );
    Record::new("zena", pin, lines)
}

/// What a failing gate says after its differences: how to write the
/// record again from a run.
const REGENERATE: &str = "`tests zena regenerate` writes the record again from all three subjects, the browser included; `tests zena regenerate --dry-run` prints the difference it would write.";

/// One line per item, for a failure message.
fn lines<T: core::fmt::Display>(items: &[T]) -> String {
    items.iter().map(|item| format!("  {item}\n")).collect()
}

#[wcmp_macros::test]
async fn it_holds_every_subject_of_every_scenario_to_the_committed_record() {
    let record = committed_record();
    assert_eq!(
        record.toolchain, "zena",
        "the header of tests/zena/record.txt"
    );
    let reports = subjects(BUNDLE).await;
    let differences = record.differences(&built_revision(), &reports);
    assert!(
        differences.is_empty(),
        "the Zena run differs from tests/zena/record.txt:\n{}where this run stopped:\n{}{REGENERATE}",
        lines(&differences),
        lines(&reports)
    );
}

/// The browser's reports, from the output of its scenario run that
/// `tests zena` keeps in the file `WCMP_ZENA_WEB_RUN` names: every line
/// that starts with `PRINTED`.
#[cfg(not(target_arch = "wasm32"))]
fn browser_reports() -> Vec<Report> {
    let path = std::env::var("WCMP_ZENA_WEB_RUN").unwrap_or_else(|_| {
        panic!("WCMP_ZENA_WEB_RUN names no file; this test reads the browser's run that `tests zena` keeps")
    });
    let output = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"));
    let reports: Vec<Report> = output
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix(PRINTED))
        .map(|line| {
            line.parse()
                .unwrap_or_else(|error| panic!("{path}: `{line}`: {error}"))
        })
        .collect();
    for report in &reports {
        assert_eq!(report.subject, Subject::Web, "{path}: `{report}`");
    }
    reports
}

#[cfg(not(target_arch = "wasm32"))]
#[wcmp_macros::test]
async fn it_holds_all_three_subjects_to_the_record_and_prints_the_table() {
    let browser = browser_reports();
    let mut reports = subjects(BUNDLE).await;
    let bundled = zena_scenarios();
    let mut scenarios: Vec<&str> = bundled
        .iter()
        .map(|scenario| scenario.name.as_str())
        .collect();
    let mut reported: Vec<&str> = browser
        .iter()
        .map(|report| report.scenario.as_str())
        .collect();
    scenarios.sort_unstable();
    reported.sort_unstable();
    assert_eq!(
        reported, scenarios,
        "the browser's run reports these scenarios where the bundle holds those"
    );
    reports.extend(browser.iter().cloned());

    let pin = built_revision();
    let run = Record::new("zena", pin.clone(), reports.clone());
    let table = wcmp_scenario::Table::new(&run).to_string();
    match std::env::var("WCMP_ZENA_TABLE") {
        Ok(path) => std::fs::write(&path, &table).unwrap_or_else(|error| panic!("{path}: {error}")),
        Err(_) => println!("{table}"),
    }
    if let Ok(path) = std::env::var("WCMP_ZENA_REGENERATE") {
        std::fs::write(&path, run.to_string()).unwrap_or_else(|error| panic!("{path}: {error}"));
        return;
    }
    let differences = committed_record().differences(&pin, &reports);
    assert!(
        differences.is_empty(),
        "the Zena run of all three subjects differs from tests/zena/record.txt:\n{}where this run stopped:\n{}{REGENERATE}",
        lines(&differences),
        lines(&run.lines)
    );
}

#[wcmp_macros::test]
async fn it_fails_the_gate_when_a_recorded_stage_is_one_step_later_or_earlier_than_the_run() {
    let pin = built_revision();
    let reports = [subjects(BUNDLE).await, subjects(RECORD_CHECK).await].concat();
    let matching = record_of(&pin, &reports);
    assert!(matching.differences(&pin, &reports).is_empty());
    let (mut later, mut earlier) = (0, 0);
    for report in &reports {
        let index = Stage::ALL
            .iter()
            .position(|&stage| stage == report.verdict.stage)
            .unwrap();
        for (neighbor, count) in [
            (index.checked_add(1), &mut later),
            (index.checked_sub(1), &mut earlier),
        ] {
            let Some(recorded) = neighbor.and_then(|neighbor| Stage::ALL.get(neighbor).copied())
            else {
                continue;
            };
            let mut record = matching.clone();
            let line = record
                .lines
                .iter_mut()
                .find(|line| line.scenario == report.scenario && line.subject == report.subject)
                .unwrap();
            line.verdict.stage = recorded;
            assert_eq!(
                record.differences(&pin, &reports),
                [Difference::Stage {
                    scenario: report.scenario.clone(),
                    subject: report.subject,
                    recorded,
                    verdict: report.verdict.clone(),
                }],
                "`{report}` against a record of `{recorded}`"
            );
            *count += 1;
        }
    }
    // The scenario that Zena refuses stops at the first stage and has a
    // later one; scenario 1 stops after it and has an earlier one.
    assert!(
        later > 0 && earlier > 0,
        "{later} later and {earlier} earlier stages for:\n{}",
        lines(&reports)
    );
}

#[wcmp_macros::test]
async fn it_fails_the_gate_once_for_a_missing_line_and_once_for_a_line_of_no_scenario() {
    let pin = built_revision();
    let reports = subjects(BUNDLE).await;
    let mut record = record_of(&pin, &reports);
    record
        .lines
        .retain(|line| !(line.scenario == SCALAR_EXPORT && line.subject == Subject::polyfill()));
    let stray: Report = "no-such-scenario wasmtime pass".parse().unwrap();
    record.lines.push(stray.clone());
    assert_eq!(
        record.differences(&pin, &reports),
        [
            Difference::Missing {
                scenario: SCALAR_EXPORT.to_string(),
                subject: Subject::polyfill(),
            },
            Difference::Unknown(stray),
        ]
    );
}

#[wcmp_macros::test]
async fn it_fails_the_gate_when_the_record_names_another_revision_and_names_both() {
    let pin = built_revision();
    let other = if pin.starts_with('0') {
        "f".repeat(pin.len())
    } else {
        "0".repeat(pin.len())
    };
    let reports = subjects(BUNDLE).await;
    let record = record_of(&other, &reports);
    let differences = record.differences(&pin, &reports);
    assert_eq!(
        differences,
        [Difference::Pin {
            recorded: other.clone(),
            built: pin.clone(),
        }]
    );
    let message = differences[0].to_string();
    assert!(
        message.contains(&pin) && message.contains(&other),
        "{message}"
    );
}

#[wcmp_macros::test]
async fn it_records_compile_for_every_subject_of_a_program_zena_refuses() {
    let scenario = record_check(REFUSED);
    // The build succeeded without a component, and kept Zena's exit
    // status and its error output.
    let [program] = &scenario.programs[..] else {
        panic!(
            "{} holds {} programs",
            scenario.name,
            scenario.programs.len()
        );
    };
    assert_ne!(program.status, 0);
    assert!(program.component.is_none());
    assert!(program.log.contains("Error"), "{}", program.log);

    let refused = Verdict::not_compiled(&program.name, program.status, &program.log);
    assert_every_subject_stops(&scenario.name, &refused, &program.log).await;
}

#[wcmp_macros::test]
async fn it_records_compose_for_every_subject_of_a_composition_wac_refuses() {
    let scenario = record_check(UNPLUGGED);
    // Both programs compiled, and the build succeeded: it left the
    // importer alone under its name, with `wac`'s exit status and its
    // error output, and removed the exporter it could not plug in.
    let [program] = &scenario.programs[..] else {
        panic!(
            "{} holds {} programs",
            scenario.name,
            scenario.programs.len()
        );
    };
    assert_eq!(program.status, 0);
    let status = program
        .compose_status
        .unwrap_or_else(|| panic!("the build composed nothing into {}", program.name));
    assert_ne!(status, 0);
    assert!(
        program.compose_log.contains("error"),
        "{}",
        program.compose_log
    );

    let refused = Verdict::not_composed(&program.name, status, &program.compose_log);
    assert_every_subject_stops(&scenario.name, &refused, &program.compose_log).await;
}

/// The scenario `name` of the record-check bundle.
fn record_check(name: &str) -> Scenario {
    bundle::scenarios(RECORD_CHECK)
        .unwrap_or_else(|error| panic!("the record-check bundle: {error}"))
        .into_iter()
        .find(|scenario| scenario.name == name)
        .unwrap_or_else(|| panic!("the record-check bundle has no scenario {name}"))
}

/// Hold the Wasmtime run and this target's polyfill subject of the
/// record-check scenario `name` to `verdict`, and check that a record
/// written from the run keeps `output`, the output of the tool that
/// stopped the scenario, on the line of every subject.
async fn assert_every_subject_stops(name: &str, verdict: &Verdict, output: &str) {
    let reports: Vec<Report> = subjects(RECORD_CHECK)
        .await
        .into_iter()
        .filter(|report| report.scenario == name)
        .collect();
    assert_eq!(reports.len(), 2, "{}", lines(&reports));
    for report in &reports {
        assert_eq!(&report.verdict, verdict, "{report}");
    }
    let record: Record = record_of(&built_revision(), &reports)
        .to_string()
        .parse()
        .unwrap();
    for subject in Subject::ALL {
        let line = record.line(name, subject).unwrap();
        assert_eq!(line.verdict.stage, verdict.stage, "{line}");
        assert!(
            line.verdict.reason.contains(output.trim()),
            "`{line}` does not keep the tool's output: {output}"
        );
    }
}

/// A component with a scalar export at its root, the same function
/// inside an exported interface, and an export that traps.
const CALCULATOR: &[u8] = component!(
    r#"
    (component
      (core module $m
        (func (export "add") (param i32 i32) (result i32)
          local.get 0
          local.get 1
          i32.add)
        (func (export "boom") unreachable))
      (core instance $i (instantiate $m))
      (func $add (param "a" s32) (param "b" s32) (result s32)
        (canon lift (core func $i "add")))
      (func $boom (canon lift (core func $i "boom")))
      (export "add" (func $add))
      (export "boom" (func $boom))
      (instance $api (export "add" (func $add)))
      (export "local:demo/api" (instance $api)))
    "#
);

/// A component whose `greet` prints `hello` and `world` through
/// `wasi:cli/stdout@0.3.0`, as the pinned Zena's console does, and
/// returns 2. It creates a `stream<u8>`, hands the readable end to
/// `write-via-stream`, writes both lines in one write, which the host
/// takes whole before the write returns, and drops the writable end.
const PRINTER: &[u8] = component!(
    r#"
    (component
      (type (instance
        (type (enum "io" "illegal-byte-sequence" "pipe"))
        (export "error-code" (type (eq 0)))))
      (import "wasi:cli/types@0.3.0" (instance $types (type 0)))
      (alias export $types "error-code" (type))
      (type (instance
        (alias outer 1 1 (type))
        (export "error-code" (type (eq 0)))
        (type (stream u8))
        (type (result (error 1)))
        (type (future 3))
        (type (func (param "data" 2) (result 4)))
        (export "write-via-stream" (func (type 5)))))
      (import "wasi:cli/stdout@0.3.0" (instance $stdout (type 2)))
      (type $bytes (stream u8))
      (core module $libc
        (memory (export "memory") 1)
        (data (i32.const 0) "hello\nworld\n"))
      (core instance $libc (instantiate $libc))
      (alias export $stdout "write-via-stream" (func $write-via-stream))
      (core func $write-via-stream (canon lower (func $write-via-stream)))
      (core func $stream-new (canon stream.new $bytes))
      (core func $write
        (canon stream.write $bytes async (memory (core memory $libc "memory"))))
      (core func $drop-writable (canon stream.drop-writable $bytes))
      (core module $main
        (import "" "write-via-stream" (func $write-via-stream (param i32) (result i32)))
        (import "" "stream.new" (func $stream-new (result i64)))
        (import "" "stream.write" (func $write (param i32 i32 i32) (result i32)))
        (import "" "stream.drop-writable" (func $drop-writable (param i32)))
        (func (export "greet") (result i32)
          (local $pair i64)
          (local $writable i32)
          (local.set $pair (call $stream-new))
          (local.set $writable (i32.wrap_i64 (i64.shr_u (local.get $pair) (i64.const 32))))
          (drop (call $write-via-stream (i32.wrap_i64 (local.get $pair))))
          ;; Twelve bytes, completed: 12 << 4.
          (if (i32.ne (call $write (local.get $writable) (i32.const 0) (i32.const 12))
                (i32.const 192))
            (then unreachable))
          (call $drop-writable (local.get $writable))
          (i32.const 2)))
      (core instance $main (instantiate $main (with "" (instance
        (export "write-via-stream" (func $write-via-stream))
        (export "stream.new" (func $stream-new))
        (export "stream.write" (func $write))
        (export "stream.drop-writable" (func $drop-writable))))))
      (func (export "greet") (result s32) (canon lift (core func $main "greet"))))
    "#
);

/// A component whose `greet` prints `hello` through
/// `wasi:cli/stdout@0.3.0` and writes `oops` through
/// `wasi:cli/stderr@0.3.0`, as the pinned Zena's `console.log` and
/// `console.error` do, and returns 2. Both interfaces have the one
/// shape, so both imports share one instance type. Each write goes to a
/// `stream<u8>` of its own, whose writable end it drops after the write.
const COMPLAINER: &[u8] = component!(
    r#"
    (component
      (type (instance
        (type (enum "io" "illegal-byte-sequence" "pipe"))
        (export "error-code" (type (eq 0)))))
      (import "wasi:cli/types@0.3.0" (instance $types (type 0)))
      (alias export $types "error-code" (type))
      (type (instance
        (alias outer 1 1 (type))
        (export "error-code" (type (eq 0)))
        (type (stream u8))
        (type (result (error 1)))
        (type (future 3))
        (type (func (param "data" 2) (result 4)))
        (export "write-via-stream" (func (type 5)))))
      (import "wasi:cli/stdout@0.3.0" (instance $stdout (type 2)))
      (import "wasi:cli/stderr@0.3.0" (instance $stderr (type 2)))
      (type $bytes (stream u8))
      (core module $libc
        (memory (export "memory") 1)
        (data (i32.const 0) "hello\n")
        (data (i32.const 16) "oops\n"))
      (core instance $libc (instantiate $libc))
      (alias export $stdout "write-via-stream" (func $write-stdout))
      (alias export $stderr "write-via-stream" (func $write-stderr))
      (core func $write-stdout (canon lower (func $write-stdout)))
      (core func $write-stderr (canon lower (func $write-stderr)))
      (core func $stream-new (canon stream.new $bytes))
      (core func $write
        (canon stream.write $bytes async (memory (core memory $libc "memory"))))
      (core func $drop-writable (canon stream.drop-writable $bytes))
      (core module $main
        (import "" "write-stdout" (func $write-stdout (param i32) (result i32)))
        (import "" "write-stderr" (func $write-stderr (param i32) (result i32)))
        (import "" "stream.new" (func $stream-new (result i64)))
        (import "" "stream.write" (func $write (param i32 i32 i32) (result i32)))
        (import "" "stream.drop-writable" (func $drop-writable (param i32)))
        (func (export "greet") (result i32)
          (local $pair i64)
          (local $writable i32)
          (local.set $pair (call $stream-new))
          (local.set $writable (i32.wrap_i64 (i64.shr_u (local.get $pair) (i64.const 32))))
          (drop (call $write-stdout (i32.wrap_i64 (local.get $pair))))
          ;; Six bytes, completed: 6 << 4.
          (if (i32.ne (call $write (local.get $writable) (i32.const 0) (i32.const 6))
                (i32.const 96))
            (then unreachable))
          (call $drop-writable (local.get $writable))
          (local.set $pair (call $stream-new))
          (local.set $writable (i32.wrap_i64 (i64.shr_u (local.get $pair) (i64.const 32))))
          (drop (call $write-stderr (i32.wrap_i64 (local.get $pair))))
          ;; Five bytes, completed: 5 << 4.
          (if (i32.ne (call $write (local.get $writable) (i32.const 16) (i32.const 5))
                (i32.const 80))
            (then unreachable))
          (call $drop-writable (local.get $writable))
          (i32.const 2)))
      (core instance $main (instantiate $main (with "" (instance
        (export "write-stdout" (func $write-stdout))
        (export "write-stderr" (func $write-stderr))
        (export "stream.new" (func $stream-new))
        (export "stream.write" (func $write))
        (export "stream.drop-writable" (func $drop-writable))))))
      (func (export "greet") (result s32) (canon lift (core func $main "greet"))))
    "#
);

/// A component whose `shout` passes its string through the test
/// interface's `echo` and returns what `echo` returned.
const ECHOER: &[u8] = component!(
    r#"
    (component
      (import "wcmp:scenario/host" (instance $host
        (export "echo" (func (param "text" string) (result string)))))
      (core module $memory
        (memory (export "memory") 1)
        (global $next (mut i32) (i32.const 1024))
        (func (export "realloc") (param i32 i32 i32 i32) (result i32)
          (local $pointer i32)
          global.get $next
          local.get 2
          i32.const 1
          i32.sub
          i32.add
          i32.const 0
          local.get 2
          i32.sub
          i32.and
          local.tee $pointer
          local.get 3
          i32.add
          global.set $next
          local.get $pointer))
      (core instance $memory (instantiate $memory))
      (alias core export $memory "memory" (core memory $mem))
      (alias core export $memory "realloc" (core func $realloc))
      (alias export $host "echo" (func $echo))
      (core func $echo_lowered
        (canon lower (func $echo) (memory $mem) (realloc $realloc)))
      (core module $main
        (import "host" "echo" (func $echo (param i32 i32 i32)))
        (func (export "shout") (param i32 i32) (result i32)
          local.get 0
          local.get 1
          i32.const 16
          call $echo
          i32.const 16))
      (core instance $main (instantiate $main
        (with "host" (instance (export "echo" (func $echo_lowered))))))
      (func (export "shout") (param "text" string) (result string)
        (canon lift (core func $main "shout") (memory $mem) (realloc $realloc))))
    "#
);

/// A component that imports `wasi:clocks/monotonic-clock@0.3.0` whole,
/// in the shape the pinned Zena's timer imports it, and calls each of
/// its functions.
///
/// `sleep-for` and `sleep-until` are lifted `async` with a callback, as
/// Zena lifts an async export. Each lowers its wait with the `async`
/// option. A wait that returned at once returns; one that started
/// joins its subtask to a waitable set and waits on it, and the
/// callback returns once the subtask has. `waited` says whether a wait
/// had to start. `now` and `resolution` pass the clock's reading and
/// its resolution through.
const CLOCK_USER: &[u8] = component!(
    r#"
    (component
      (type (instance
        (type u64)
        (export "duration" (type (eq 0)))))
      (import "wasi:clocks/types@0.3.0" (instance $types (type 0)))
      (alias export $types "duration" (type))
      (type (instance
        (alias outer 1 1 (type))
        (export "duration" (type (eq 0)))
        (type u64)
        (export "mark" (type (eq 2)))
        (type (func (result 3)))
        (export "now" (func (type 4)))
        (type (func (result 1)))
        (export "get-resolution" (func (type 5)))
        (type (func async (param "when" 3)))
        (export "wait-until" (func (type 6)))
        (type (func async (param "how-long" 1)))
        (export "wait-for" (func (type 7)))))
      (import "wasi:clocks/monotonic-clock@0.3.0" (instance $clock (type 2)))
      (alias export $clock "now" (func $now))
      (alias export $clock "get-resolution" (func $get-resolution))
      (alias export $clock "wait-until" (func $wait-until))
      (alias export $clock "wait-for" (func $wait-for))
      (core func $now (canon lower (func $now)))
      (core func $get-resolution (canon lower (func $get-resolution)))
      (core func $wait-until (canon lower (func $wait-until) async))
      (core func $wait-for (canon lower (func $wait-for) async))
      (core func $task-return (canon task.return))
      (core func $set-new (canon waitable-set.new))
      (core func $join (canon waitable.join))
      (core func $subtask-drop (canon subtask.drop))
      (core module $m
        (import "" "now" (func $now (result i64)))
        (import "" "get-resolution" (func $get-resolution (result i64)))
        (import "" "wait-until" (func $wait-until (param i64) (result i32)))
        (import "" "wait-for" (func $wait-for (param i64) (result i32)))
        (import "" "task.return" (func $task-return))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (import "" "waitable.join" (func $join (param i32 i32)))
        (import "" "subtask.drop" (func $subtask-drop (param i32)))
        (global $set (mut i32) (i32.const 0))
        (global $waited (mut i32) (i32.const 0))
        ;; Return when the lowered wait that answered `status` has
        ;; returned, or else wait on its subtask.
        (func $park (param $status i32) (result i32)
          (if (i32.eq (i32.and (local.get $status) (i32.const 0xf)) (i32.const 2))
            (then
              (call $task-return)
              (return (i32.const 0))))
          (global.set $waited (i32.const 1))
          (global.set $set (call $set-new))
          (call $join (i32.shr_u (local.get $status) (i32.const 4)) (global.get $set))
          (i32.or (i32.shl (global.get $set) (i32.const 4)) (i32.const 2)))
        (func (export "sleep-for") (param i64) (result i32)
          (call $park (call $wait-for (local.get 0))))
        (func (export "sleep-until") (param i64) (result i32)
          (call $park (call $wait-until (local.get 0))))
        ;; A subtask that returned is dropped and ends the task; any
        ;; other event waits on.
        (func (export "callback") (param $event i32) (param $index i32) (param $status i32)
          (result i32)
          (if (i32.and
                (i32.eq (local.get $event) (i32.const 1))
                (i32.eq (local.get $status) (i32.const 2)))
            (then
              (call $subtask-drop (local.get $index))
              (call $task-return)
              (return (i32.const 0))))
          (i32.or (i32.shl (global.get $set) (i32.const 4)) (i32.const 2)))
        (func (export "now") (result i64) (call $now))
        (func (export "resolution") (result i64) (call $get-resolution))
        (func (export "waited") (result i32) (global.get $waited)))
      (core instance $m (instantiate $m (with "" (instance
        (export "now" (func $now))
        (export "get-resolution" (func $get-resolution))
        (export "wait-until" (func $wait-until))
        (export "wait-for" (func $wait-for))
        (export "task.return" (func $task-return))
        (export "waitable-set.new" (func $set-new))
        (export "waitable.join" (func $join))
        (export "subtask.drop" (func $subtask-drop))))))
      (func (export "sleep-for") async (param "how-long" u64)
        (canon lift (core func $m "sleep-for") async (callback (core func $m "callback"))))
      (func (export "sleep-until") async (param "when" u64)
        (canon lift (core func $m "sleep-until") async (callback (core func $m "callback"))))
      (func (export "now") (result u64) (canon lift (core func $m "now")))
      (func (export "resolution") (result u64) (canon lift (core func $m "resolution")))
      (func (export "waited") (result bool) (canon lift (core func $m "waited"))))
    "#
);

/// A component that imports a function no linker here supplies.
const UNLINKABLE: &[u8] = component!(
    r#"
    (component
      (import "wcmp:scenario/missing" (instance
        (export "nothing" (func)))))
    "#
);

/// A component whose core module traps in its start function.
const TRAPS_ON_START: &[u8] = component!(
    r#"
    (component
      (core module $m
        (func $start unreachable)
        (start $start))
      (core instance (instantiate $m)))
    "#
);

/// A component that exports the interface `local:demo/greeter`,
/// whose `async` function `greet` takes a name and returns `hello, `
/// followed by the name, written to fresh memory. It is lifted
/// synchronously, which an `async` function allows.
const GREETER: &[u8] = component!(
    r#"
    (component
      (core module $m
        (memory (export "memory") 1)
        (data (i32.const 0) "hello, ")
        (global $next (mut i32) (i32.const 1024))
        (func $realloc (export "realloc") (param i32 i32 i32 i32) (result i32)
          (local $pointer i32)
          global.get $next
          local.get 2
          i32.const 1
          i32.sub
          i32.add
          i32.const 0
          local.get 2
          i32.sub
          i32.and
          local.tee $pointer
          local.get 3
          i32.add
          global.set $next
          local.get $pointer)
        (func (export "greet") (param $name i32) (param $length i32) (result i32)
          (local $greeting i32)
          (local.set $length (i32.add (local.get $length) (i32.const 7)))
          (local.set $greeting
            (call $realloc (i32.const 0) (i32.const 0) (i32.const 1) (local.get $length)))
          (memory.copy (local.get $greeting) (i32.const 0) (i32.const 7))
          (memory.copy
            (i32.add (local.get $greeting) (i32.const 7))
            (local.get $name)
            (i32.sub (local.get $length) (i32.const 7)))
          (i32.store (i32.const 16) (local.get $greeting))
          (i32.store (i32.const 20) (local.get $length))
          i32.const 16))
      (core instance $i (instantiate $m))
      (type $greet (func async (param "name" string) (result string)))
      (func $greet (type $greet)
        (canon lift (core func $i "greet")
          (memory (core memory $i "memory")) (realloc (core func $i "realloc"))))
      (instance $greeter (export "greet" (func $greet)))
      (export "local:demo/greeter" (instance $greeter)))
    "#
);

/// [`GREETER`] lifted as Zena lifts an `async` export: `async` with a
/// callback. `greet` writes its greeting and yields once instead of
/// returning, so a caller cannot have the result from the first poll;
/// the callback it is given back returns the greeting through
/// `task.return`.
const CALLBACK_GREETER: &[u8] = component!(
    r#"
    (component
      (core module $libc
        (memory (export "memory") 1)
        (data (i32.const 0) "hello, ")
        (global $next (mut i32) (i32.const 1024))
        (func (export "realloc") (param i32 i32 i32 i32) (result i32)
          (local $pointer i32)
          global.get $next
          local.get 2
          i32.const 1
          i32.sub
          i32.add
          i32.const 0
          local.get 2
          i32.sub
          i32.and
          local.tee $pointer
          local.get 3
          i32.add
          global.set $next
          local.get $pointer))
      (core instance $libc (instantiate $libc))
      (alias core export $libc "memory" (core memory $mem))
      (alias core export $libc "realloc" (core func $realloc))
      (core func $task-return (canon task.return (result string) (memory $mem)))
      (core module $main
        (import "libc" "memory" (memory 1))
        (import "libc" "realloc" (func $realloc (param i32 i32 i32 i32) (result i32)))
        (import "" "task.return" (func $task-return (param i32 i32)))
        (global $greeting (mut i32) (i32.const 0))
        (global $length (mut i32) (i32.const 0))
        (func (export "greet") (param $name i32) (param $name-length i32) (result i32)
          (global.set $length (i32.add (local.get $name-length) (i32.const 7)))
          (global.set $greeting
            (call $realloc (i32.const 0) (i32.const 0) (i32.const 1) (global.get $length)))
          (memory.copy (global.get $greeting) (i32.const 0) (i32.const 7))
          (memory.copy
            (i32.add (global.get $greeting) (i32.const 7))
            (local.get $name)
            (local.get $name-length))
          ;; YIELD.
          (i32.const 1))
        (func (export "greet-callback") (param i32 i32 i32) (result i32)
          (call $task-return (global.get $greeting) (global.get $length))
          ;; EXIT.
          (i32.const 0)))
      (core instance $main (instantiate $main
        (with "libc" (instance $libc))
        (with "" (instance (export "task.return" (func $task-return))))))
      (type $greet (func async (param "name" string) (result string)))
      (func $greet (type $greet)
        (canon lift (core func $main "greet") async
          (callback (core func $main "greet-callback")) (memory $mem) (realloc $realloc)))
      (instance $greeter (export "greet" (func $greet)))
      (export "local:demo/greeter" (instance $greeter)))
    "#
);

/// [`GREETER`] with a `greet` that is not `async`.
const SYNC_GREETER: &[u8] = component!(
    r#"
    (component
      (core module $m
        (memory (export "memory") 1)
        (func (export "realloc") (param i32 i32 i32 i32) (result i32)
          i32.const 1024)
        (func (export "greet") (param i32 i32) (result i32)
          (i32.store (i32.const 16) (i32.const 0))
          (i32.store (i32.const 20) (i32.const 0))
          i32.const 16))
      (core instance $i (instantiate $m))
      (func $greet (param "name" string) (result string)
        (canon lift (core func $i "greet")
          (memory (core memory $i "memory")) (realloc (core func $i "realloc"))))
      (instance $greeter (export "greet" (func $greet)))
      (export "local:demo/greeter" (instance $greeter)))
    "#
);

/// A component that imports `local:demo/greeter` and exports
/// `welcome`, which passes its name to `greet` and returns what
/// `greet` returned. It lowers `greet` synchronously.
const CALLER: &[u8] = component!(
    r#"
    (component
      (import "local:demo/greeter" (instance $greeter
        (export "greet" (func async (param "name" string) (result string)))))
      (core module $memory
        (memory (export "memory") 1)
        (global $next (mut i32) (i32.const 1024))
        (func (export "realloc") (param i32 i32 i32 i32) (result i32)
          (local $pointer i32)
          global.get $next
          local.get 2
          i32.const 1
          i32.sub
          i32.add
          i32.const 0
          local.get 2
          i32.sub
          i32.and
          local.tee $pointer
          local.get 3
          i32.add
          global.set $next
          local.get $pointer))
      (core instance $memory (instantiate $memory))
      (alias core export $memory "memory" (core memory $mem))
      (alias core export $memory "realloc" (core func $realloc))
      (alias export $greeter "greet" (func $greet))
      (core func $greet_lowered
        (canon lower (func $greet) (memory $mem) (realloc $realloc)))
      (core module $main
        (import "greeter" "greet" (func $greet (param i32 i32 i32)))
        (func (export "welcome") (param i32 i32) (result i32)
          local.get 0
          local.get 1
          i32.const 16
          call $greet
          i32.const 16))
      (core instance $main (instantiate $main
        (with "greeter" (instance (export "greet" (func $greet_lowered))))))
      (type $welcome (func async (param "name" string) (result string)))
      (func $welcome (type $welcome)
        (canon lift (core func $main "welcome") (memory $mem) (realloc $realloc)))
      (export "welcome" (func $welcome)))
    "#
);

/// [`CALLER`] as Zena builds an importer: it lowers `greet` with the
/// `async` option and lifts `welcome` `async` with a callback.
///
/// `welcome` calls `greet` with the return area at address 16. A call
/// that returned has its greeting there already, and `welcome` returns
/// it at once. A call that started joins its subtask to a waitable set
/// and waits on it; the callback waits again until the subtask has
/// returned, then drops the subtask and the set and returns the
/// greeting. `waited` says whether the callback ran, so a test can
/// tell which of the two ways the call took.
const ASYNC_CALLER: &[u8] = component!(
    r#"
    (component
      (import "local:demo/greeter" (instance $greeter
        (export "greet" (func async (param "name" string) (result string)))))
      (core module $libc
        (memory (export "memory") 1)
        (global $next (mut i32) (i32.const 1024))
        (func (export "realloc") (param i32 i32 i32 i32) (result i32)
          (local $pointer i32)
          global.get $next
          local.get 2
          i32.const 1
          i32.sub
          i32.add
          i32.const 0
          local.get 2
          i32.sub
          i32.and
          local.tee $pointer
          local.get 3
          i32.add
          global.set $next
          local.get $pointer))
      (core instance $libc (instantiate $libc))
      (alias core export $libc "memory" (core memory $mem))
      (alias core export $libc "realloc" (core func $realloc))
      (alias export $greeter "greet" (func $greet))
      (core func $greet
        (canon lower (func $greet) async (memory $mem) (realloc $realloc)))
      (core func $task-return (canon task.return (result string) (memory $mem)))
      (core func $set-new (canon waitable-set.new))
      (core func $join (canon waitable.join))
      (core func $subtask-drop (canon subtask.drop))
      (core func $set-drop (canon waitable-set.drop))
      (core module $main
        (import "libc" "memory" (memory 1))
        (import "" "greet" (func $greet (param i32 i32 i32) (result i32)))
        (import "" "task.return" (func $task-return (param i32 i32)))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (import "" "waitable.join" (func $join (param i32 i32)))
        (import "" "subtask.drop" (func $subtask-drop (param i32)))
        (import "" "waitable-set.drop" (func $set-drop (param i32)))
        (global $set (mut i32) (i32.const 0))
        (global $waited (mut i32) (i32.const 0))
        (func $return-greeting
          (call $task-return (i32.load (i32.const 16)) (i32.load (i32.const 20))))
        (func (export "welcome") (param i32 i32) (result i32)
          (local $status i32)
          (local.set $status (call $greet (local.get 0) (local.get 1) (i32.const 16)))
          ;; RETURNED.
          (if (i32.eq (i32.and (local.get $status) (i32.const 0xf)) (i32.const 2))
            (then
              (call $return-greeting)
              (return (i32.const 0))))
          (global.set $set (call $set-new))
          (call $join (i32.shr_u (local.get $status) (i32.const 4)) (global.get $set))
          ;; WAIT on the set.
          (i32.or (i32.shl (global.get $set) (i32.const 4)) (i32.const 2)))
        (func (export "welcome-callback")
          (param $event i32) (param $subtask i32) (param $state i32) (result i32)
          (global.set $waited (i32.const 1))
          ;; Only the subtask's events, SUBTASK, reach the set.
          (if (i32.ne (local.get $event) (i32.const 1)) (then unreachable))
          ;; Until the subtask has RETURNED, WAIT on the set again.
          (if (i32.ne (local.get $state) (i32.const 2))
            (then (return (i32.or (i32.shl (global.get $set) (i32.const 4)) (i32.const 2)))))
          (call $subtask-drop (local.get $subtask))
          (call $set-drop (global.get $set))
          (call $return-greeting)
          ;; EXIT.
          (i32.const 0))
        (func (export "waited") (result i32) (global.get $waited)))
      (core instance $main (instantiate $main
        (with "libc" (instance $libc))
        (with "" (instance
          (export "greet" (func $greet))
          (export "task.return" (func $task-return))
          (export "waitable-set.new" (func $set-new))
          (export "waitable.join" (func $join))
          (export "subtask.drop" (func $subtask-drop))
          (export "waitable-set.drop" (func $set-drop))))))
      (type $welcome (func async (param "name" string) (result string)))
      (func $welcome (type $welcome)
        (canon lift (core func $main "welcome") async
          (callback (core func $main "welcome-callback")) (memory $mem) (realloc $realloc)))
      (export "welcome" (func $welcome))
      (func (export "waited") (result bool) (canon lift (core func $main "waited"))))
    "#
);

/// The wiring of [`CALLER`] to a greeter.
const CALLER_TO_GREETER: &str = "run-time caller local:demo/greeter greeter";

/// A scenario of `programs`, linked as `wiring` says, with
/// `expectations` and the Wasmtime run's `observations` in the text
/// format of the scenario model.
fn scenario(
    programs: Vec<Program>,
    wiring: &str,
    expectations: &str,
    observations: &str,
) -> Scenario {
    Scenario {
        name: "test".to_string(),
        expectations: expectations.parse().unwrap(),
        observations: observations.parse().unwrap(),
        wiring: wiring.parse().unwrap(),
        programs,
    }
}

/// Run `programs` through the polyfill and judge them.
async fn run(programs: Vec<Program>, expectations: &str, observations: &str) -> Verdict {
    run_wired(programs, "", expectations, observations).await
}

/// Run `programs`, linked as `wiring` says, through the polyfill and
/// judge them.
async fn run_wired(
    programs: Vec<Program>,
    wiring: &str,
    expectations: &str,
    observations: &str,
) -> Verdict {
    polyfill()
        .run(&scenario(programs, wiring, expectations, observations))
        .await
        .unwrap()
}

/// Run [`CALCULATOR`] and judge it.
async fn calculate(expectations: &str, observations: &str) -> Verdict {
    run(
        vec![Program::compiled("calc", CALCULATOR)],
        expectations,
        observations,
    )
    .await
}

/// Observations of a Wasmtime run that stopped before its calls, so a
/// subject is judged against the expectations alone.
const WASMTIME_STOPPED: &str = r#"stage instantiate "component calc: refused""#;

#[wcmp_macros::test]
async fn it_passes_a_scenario_whose_calls_end_as_the_wasmtime_run_saw() {
    let verdict = calculate(
        "
        call calc add(1s32, 2s32) -> 3s32
        call calc local:demo/api#add(2s32, 2s32) -> 4s32
        call calc boom()
        ",
        r#"
        stage pass
        call calc add(1s32, 2s32) -> 3s32
        call calc local:demo/api#add(2s32, 2s32) -> 4s32
        call calc boom() -> fail "wasm trap: unreachable"
        "#,
    )
    .await;
    assert_eq!(verdict, Verdict::pass());
}

#[wcmp_macros::test]
async fn it_reports_a_mismatch_when_the_observations_hold_another_result() {
    let verdict = calculate(
        "call calc add(1s32, 2s32) -> 3s32",
        "stage pass\ncall calc add(1s32, 2s32) -> 4s32",
    )
    .await;
    assert_eq!(
        verdict,
        Verdict::new(
            Stage::Mismatch,
            "call 1 `calc add(1s32, 2s32)` returned 3s32 where 4s32 was expected"
        )
    );
    // An entry with no outcome is held to what the Wasmtime run saw.
    let succeeded = calculate("call calc boom()", "stage pass\ncall calc boom() -> ()").await;
    assert_eq!(succeeded.stage, Stage::Call, "{}", succeeded.reason);
}

#[wcmp_macros::test]
async fn it_judges_against_the_expectations_when_the_wasmtime_run_did_not_pass() {
    let passes = calculate(
        "call calc add(1s32, 2s32) -> 3s32\ncall calc boom()",
        WASMTIME_STOPPED,
    )
    .await;
    assert_eq!(passes, Verdict::pass());
    let differs = calculate("call calc add(1s32, 2s32) -> 4s32", WASMTIME_STOPPED).await;
    assert_eq!(differs.stage, Stage::Mismatch);
    let traps = calculate("call calc boom() -> ()", WASMTIME_STOPPED).await;
    assert_eq!(traps.stage, Stage::Call);
    assert!(
        traps.reason.starts_with("call 1 `calc boom()` failed: "),
        "{}",
        traps.reason
    );
}

#[wcmp_macros::test]
async fn it_captures_the_lines_a_scenario_prints_through_p3_standard_output() {
    let expectations = "call printer greet() -> 2s32\noutput \"hello\"\noutput \"world\"";
    let printer = || vec![Program::compiled("printer", PRINTER)];
    let same = run(
        printer(),
        expectations,
        "stage pass\ncall printer greet() -> 2s32\noutput \"hello\"\noutput \"world\"",
    )
    .await;
    assert_eq!(same, Verdict::pass());
    let other = run(
        printer(),
        expectations,
        "stage pass\ncall printer greet() -> 2s32\noutput \"hello\"\noutput \"there\"",
    )
    .await;
    assert_eq!(
        other,
        Verdict::new(
            Stage::Mismatch,
            r#"output line 2 is "world" where "there" was expected"#
        )
    );
}

#[wcmp_macros::test]
async fn it_keeps_what_a_scenario_writes_to_p3_standard_error_apart_from_its_output() {
    let (mut store, instance) = instantiate(COMPLAINER).await;
    assert_eq!(
        call_one(&mut store, &instance, "greet", &[]).await,
        Val::S32(2)
    );
    assert_eq!(store.data().lines(), ["hello"]);
    assert_eq!(store.data().error_lines(), ["oops"]);
    // Standard error is not compared, so only the output line is held
    // to the Wasmtime run.
    let verdict = run(
        vec![Program::compiled("complainer", COMPLAINER)],
        "call complainer greet() -> 2s32\noutput \"hello\"",
        "stage pass\ncall complainer greet() -> 2s32\noutput \"hello\"",
    )
    .await;
    assert_eq!(verdict, Verdict::pass());
}

#[wcmp_macros::test]
async fn it_supplies_the_test_interface_that_returns_its_string() {
    let verdict = run(
        vec![Program::compiled("echoer", ECHOER)],
        r#"call echoer shout("hello, polyfill") -> "hello, polyfill""#,
        r#"stage pass
        call echoer shout("hello, polyfill") -> "hello, polyfill""#,
    )
    .await;
    assert_eq!(verdict, Verdict::pass());
}

/// Nanoseconds in a millisecond, the unit of `wasi:clocks`.
const MILLISECOND: u64 = 1_000_000;

/// `component` instantiated in a store of its own, through a linker
/// with the test host functions.
async fn instantiate(component: &[u8]) -> (Store<Host>, Instance) {
    let engine = Engine::new().expect("engine");
    let mut linker = Linker::new(&engine);
    host::define(&mut linker).expect("the test host functions");
    let component = Component::new(&engine, component)
        .await
        .expect("the component parses");
    let mut store = Store::new(&engine, Host::default()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the component links and instantiates");
    (store, instance)
}

/// Call the export `name` of `instance` with `arguments`, and answer
/// its one result.
async fn call_one(
    store: &mut Store<Host>,
    instance: &Instance,
    name: &str,
    arguments: &[Val],
) -> Val {
    let func = instance
        .get_func(name)
        .unwrap_or_else(|| panic!("no export {name}"));
    let results = func
        .call(store, arguments)
        .await
        .unwrap_or_else(|error| panic!("{name}: {error}"));
    let [result] = &results[..] else {
        panic!("{name} returned {results:?}");
    };
    result.clone()
}

/// What the export `now` of [`CLOCK_USER`] reads.
async fn now(store: &mut Store<Host>, instance: &Instance) -> u64 {
    match call_one(store, instance, "now", &[]).await {
        Val::U64(nanos) => nanos,
        other => panic!("now returned {other:?}"),
    }
}

#[wcmp_macros::test]
async fn it_sleeps_on_the_timer_of_this_target_through_the_p3_wait_for() {
    let (mut store, instance) = instantiate(CLOCK_USER).await;
    let before = now(&mut store, &instance).await;
    let func = instance.get_func("sleep-for").expect("sleep-for");
    let results = func
        .call(&mut store, &[Val::U64(20 * MILLISECOND)])
        .await
        .expect("sleep-for");
    assert!(results.is_empty(), "{results:?}");
    let after = now(&mut store, &instance).await;
    assert_eq!(
        call_one(&mut store, &instance, "waited", &[]).await,
        Val::Bool(true),
        "`wait-for` returned before the guest could wait on it"
    );
    // A millisecond of slack for a browser clock that is coarser than
    // its timer.
    let slept = after.saturating_sub(before);
    assert!(
        slept >= 19 * MILLISECOND,
        "a wait of 20 ms ended after {slept} ns"
    );

    // The runner links the same interface, and its async export passes.
    let verdict = run(
        vec![Program::compiled("clock", CLOCK_USER)],
        "call clock sleep-for(1000000u64) -> ()",
        "stage pass\ncall clock sleep-for(1000000u64) -> ()",
    )
    .await;
    assert_eq!(verdict, Verdict::pass());
}

#[wcmp_macros::test]
async fn it_fails_the_call_of_a_method_the_test_host_does_not_implement() {
    for (call, method) in [
        ("call clock resolution() -> 1u64", "get-resolution"),
        ("call clock sleep-until(0u64) -> ()", "wait-until"),
    ] {
        let verdict = run(
            vec![Program::compiled("clock", CLOCK_USER)],
            call,
            WASMTIME_STOPPED,
        )
        .await;
        assert_eq!(verdict.stage, Stage::Call, "{call}: {}", verdict.reason);
        let named = format!(
            "`wasi:clocks/monotonic-clock@0.3.0#{method}` in the scenario runner's test host"
        );
        assert!(
            verdict.reason.contains(&named),
            "{call}: {}",
            verdict.reason
        );
    }
}

#[wcmp_macros::test]
async fn it_fails_a_call_to_a_missing_component_or_export() {
    let verdict = calculate(
        "
        call other add(1s32, 2s32) -> 3s32
        call calc subtract(1s32, 2s32) -> 3s32
        call typed calc local:demo/other#add(1s32, 2s32) -> 3s32
        ",
        WASMTIME_STOPPED,
    )
    .await;
    assert_eq!(
        verdict,
        Verdict::new(
            Stage::Call,
            "call 1 `other add(1s32, 2s32)` failed: the scenario has no component other"
        )
    );
    for (expectations, reason) in [
        (
            "call calc subtract(1s32, 2s32) -> 3s32",
            "component calc has no function export subtract",
        ),
        (
            "call typed calc local:demo/other#add(1s32, 2s32) -> 3s32",
            "component calc has no function export local:demo/other#add",
        ),
    ] {
        let verdict = calculate(expectations, WASMTIME_STOPPED).await;
        assert_eq!(verdict.stage, Stage::Call);
        assert!(verdict.reason.ends_with(reason), "{}", verdict.reason);
    }
}

#[wcmp_macros::test]
async fn it_makes_a_typed_call_that_ends_as_the_untyped_call_does() {
    let calls = "
        call calc add(1s32, 2s32) -> 3s32
        call typed calc add(1s32, 2s32) -> 3s32
        call typed calc local:demo/api#add(2s32, 2s32) -> 4s32
        call calc boom() -> fail
        call typed calc boom() -> fail
        ";
    assert_eq!(calculate(calls, WASMTIME_STOPPED).await, Verdict::pass());
    let verdict = run(
        vec![Program::compiled("echoer", ECHOER)],
        r#"
        call echoer shout("hello, polyfill") -> "hello, polyfill"
        call typed echoer shout("hello, polyfill") -> "hello, polyfill"
        "#,
        WASMTIME_STOPPED,
    )
    .await;
    assert_eq!(verdict, Verdict::pass());
}

#[wcmp_macros::test]
async fn it_fails_a_typed_call_outside_the_closed_set_or_of_other_types() {
    let outside = calculate("call typed calc add(1s32, 2u32) -> 3s32", WASMTIME_STOPPED).await;
    assert_eq!(outside.stage, Stage::Call);
    let untyped = wcmp_scenario::Error::Untyped {
        signature: "(s32, u32) -> s32".to_string(),
    };
    assert!(
        outside.reason.ends_with(&untyped.to_string()),
        "{}",
        outside.reason
    );
    // The arguments are `u32`, and `Func::typed` refuses them for an
    // export that takes `s32`.
    let other = calculate("call typed calc add(1u32, 2u32) -> 3u32", WASMTIME_STOPPED).await;
    assert_eq!(other.stage, Stage::Call, "{}", other.reason);
}

#[wcmp_macros::test]
async fn it_holds_a_call_with_no_outcome_to_whether_the_wasmtime_call_failed() {
    // The polyfill's `add` returns where the Wasmtime run saw it fail.
    let succeeded = calculate(
        "call calc add(1s32, 2s32)",
        "stage pass\ncall calc add(1s32, 2s32) -> fail",
    )
    .await;
    assert_eq!(succeeded.stage, Stage::Mismatch, "{}", succeeded.reason);
    // Both fail, whatever each one's message says.
    let failed = calculate(
        "call calc boom() -> fail\ncall calc boom()",
        "stage pass\ncall calc boom() -> fail \"uncaught\"\ncall calc boom() -> fail \"cannot enter\"",
    )
    .await;
    assert_eq!(failed, Verdict::pass());
}

#[wcmp_macros::test]
async fn it_stops_at_the_first_step_that_fails() {
    let refused = Program {
        name: "refused".to_string(),
        status: 1,
        log: "refused.zena:4:3 - Error: Type mismatch\n".to_string(),
        component: None,
        compose_status: None,
        compose_log: String::new(),
    };
    // A composition the build could not make sorts first here, and
    // `compile` still comes before `compose`.
    let unplugged = Program {
        compose_status: Some(1),
        compose_log: "error: the socket component had no matching imports\n".to_string(),
        ..Program::compiled("calc", CALCULATOR)
    };
    let compile = run(
        vec![unplugged.clone(), refused],
        "call calc add(1s32, 2s32) -> 3s32",
        r#"stage compile "program refused did not compile (exit 1): refused.zena:4:3 - Error: Type mismatch""#,
    )
    .await;
    assert_eq!(
        compile,
        Verdict::new(
            Stage::Compile,
            "program refused did not compile (exit 1): refused.zena:4:3 - Error: Type mismatch"
        )
    );

    // Then `compose` comes before `parse`.
    let compose = run(
        vec![
            unplugged,
            Program::compiled("zzz", b"\0asm not a component"),
        ],
        "call calc add(1s32, 2s32) -> 3s32",
        r#"stage compose "the composition into calc failed (exit 1): error: the socket component had no matching imports""#,
    )
    .await;
    assert_eq!(
        compose,
        Verdict::new(
            Stage::Compose,
            "the composition into calc failed (exit 1): error: the socket component had no matching imports"
        )
    );

    let parse = run(
        vec![Program::compiled("bad", b"\0asm not a component")],
        "",
        WASMTIME_STOPPED,
    )
    .await;
    assert_eq!(parse.stage, Stage::Parse);
    assert!(
        parse.reason.starts_with("component bad: "),
        "{}",
        parse.reason
    );

    let link = run(
        vec![Program::compiled("unlinkable", UNLINKABLE)],
        "",
        WASMTIME_STOPPED,
    )
    .await;
    assert_eq!(link.stage, Stage::Link, "{}", link.reason);
    assert!(
        link.reason.contains("wcmp:scenario/missing"),
        "{}",
        link.reason
    );

    let instantiate = run(
        vec![Program::compiled("traps", TRAPS_ON_START)],
        "",
        WASMTIME_STOPPED,
    )
    .await;
    assert_eq!(
        instantiate.stage,
        Stage::Instantiate,
        "{}",
        instantiate.reason
    );
    assert!(
        instantiate.reason.starts_with("component traps: "),
        "{}",
        instantiate.reason
    );
}

#[wcmp_macros::test]
async fn it_links_async_functions_of_one_component_to_imports_of_another_at_run_time() {
    // The importer's name sorts first, so the wiring orders the
    // exporter before it.
    let programs = || {
        vec![
            Program::compiled("caller", CALLER),
            Program::compiled("greeter", GREETER),
        ]
    };
    // The greeting holds the name, so an argument lost or corrupted on
    // its way across the link fails the call.
    let linked = run_wired(
        programs(),
        CALLER_TO_GREETER,
        r#"
        call caller welcome("world") -> "hello, world"
        call caller welcome("") -> "hello, "
        call greeter local:demo/greeter#greet("zena") -> "hello, zena"
        "#,
        WASMTIME_STOPPED,
    )
    .await;
    assert_eq!(linked, Verdict::pass());

    let unwired = run(
        programs(),
        r#"call caller welcome("world") -> "hello, world""#,
        WASMTIME_STOPPED,
    )
    .await;
    assert_eq!(unwired.stage, Stage::Link, "{}", unwired.reason);
    assert!(
        unwired.reason.starts_with("component caller: ")
            && unwired.reason.contains("local:demo/greeter"),
        "{}",
        unwired.reason
    );
}

#[wcmp_macros::test]
async fn it_links_an_async_lowered_import_to_a_callback_lifted_export_as_zena_builds_them() {
    // The greeter yields before it returns, so the caller's call has
    // no result on its first poll: the caller waits on the subtask and
    // its callback runs.
    let verdict = run_wired(
        vec![
            Program::compiled("caller", ASYNC_CALLER),
            Program::compiled("greeter", CALLBACK_GREETER),
        ],
        CALLER_TO_GREETER,
        r#"
        call caller welcome("world") -> "hello, world"
        call caller waited() -> true
        call caller welcome("zena") -> "hello, zena"
        call greeter local:demo/greeter#greet("zena") -> "hello, zena"
        "#,
        WASMTIME_STOPPED,
    )
    .await;
    assert_eq!(verdict, Verdict::pass());
}

#[wcmp_macros::test]
async fn it_makes_typed_and_untyped_calls_across_a_run_time_link() {
    let verdict = run_wired(
        vec![
            Program::compiled("caller", CALLER),
            Program::compiled("greeter", GREETER),
        ],
        CALLER_TO_GREETER,
        r#"
        call caller welcome("world") -> "hello, world"
        call typed caller welcome("world") -> "hello, world"
        call greeter local:demo/greeter#greet("zena") -> "hello, zena"
        call typed greeter local:demo/greeter#greet("zena") -> "hello, zena"
        "#,
        WASMTIME_STOPPED,
    )
    .await;
    assert_eq!(verdict, Verdict::pass());
}

#[wcmp_macros::test]
async fn it_stops_at_link_when_the_exported_function_is_not_async() {
    let synchronous = run_wired(
        vec![
            Program::compiled("caller", CALLER),
            Program::compiled("greeter", SYNC_GREETER),
        ],
        CALLER_TO_GREETER,
        "",
        WASMTIME_STOPPED,
    )
    .await;
    assert_eq!(
        synchronous,
        Verdict::new(
            Stage::Link,
            "component caller: local:demo/greeter#greet is a synchronous function of component greeter, and a run-time link forwards only an `async` one"
        )
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_wiring_that_names_a_missing_component_or_leaves_no_order() {
    let calculator = || vec![Program::compiled("calc", CALCULATOR)];
    let missing = polyfill()
        .run(&scenario(
            calculator(),
            "run-time calc local:demo/api partner",
            "",
            WASMTIME_STOPPED,
        ))
        .await;
    assert!(
        matches!(missing, Err(wcmp_scenario::Error::UnknownComponent { .. })),
        "{missing:?}"
    );
    let cycle = polyfill()
        .run(&scenario(
            calculator(),
            "run-time calc local:demo/api calc",
            "",
            WASMTIME_STOPPED,
        ))
        .await;
    assert_eq!(
        cycle,
        Err(wcmp_scenario::Error::LinkCycle(vec!["calc".to_string()]))
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_link_whose_import_or_export_a_component_lacks_instead_of_stopping_at_link() {
    let wired = |greeter: &[u8], wiring: &str| {
        scenario(
            vec![
                Program::compiled("caller", CALLER),
                Program::compiled("greeter", greeter),
            ],
            wiring,
            "",
            WASMTIME_STOPPED,
        )
    };
    let misspelled = "run-time caller local:demo/greter greeter";
    assert_eq!(
        polyfill().run(&wired(GREETER, misspelled)).await,
        Err(wcmp_scenario::Error::UnknownImport {
            link: misspelled.to_string(),
            component: "caller".to_string(),
        })
    );
    assert_eq!(
        polyfill().run(&wired(CALCULATOR, CALLER_TO_GREETER)).await,
        Err(wcmp_scenario::Error::UnknownExport {
            link: CALLER_TO_GREETER.to_string(),
            component: "greeter".to_string(),
        })
    );
}
