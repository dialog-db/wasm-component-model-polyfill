---
id: db2bc9
title: Keep tests zena gating under an inherited regenerate variable, and test its browser parsing
type: bug
blocked_by: []
labels: [zena, backlog-burndown-001-q4]
created: 2026-09-28T05:22:00Z
---

## What to build
Close the gaps the independent review of card 568112 found in `tests zena` (landed as `179f08a0c`). None blocked the card.

- **An inherited variable skips the gate.** When `WCMP_ZENA_REGENERATE` is set in the caller's environment, the three-subject test writes the record instead of gating it (`flake.nix:945-948`, `tests/zena.rs:361`). With a wrong `web` stage and the variable set, `tests zena` passed. Unset the variable everywhere except inside `tests zena regenerate`.
- **Browser report parsing has no test.** Nothing automated covers `browser_reports` (`tests/zena.rs:313-330`) or the check that the browser run covers every scenario. The reviewer checked empty, mislabelled, duplicate, wrong-stage and garbage input by hand. Turn those into tests.
- **Menu text.** The `tests zena` menu description is very long; shorten it.

The three-subject test runs only in `tests zena` and `tests all`, not in `tests native debug` (`.config/nextest.toml:15`), so it can go stale between `tests all` runs. Clippy still compiles it. This card does not change that.

## Acceptance criteria
- [x] With `WCMP_ZENA_REGENERATE` exported in the shell and a wrong stage in the record, `tests zena` fails.
- [x] Each malformed browser-output case above has a test that fails when its check breaks.
- [x] The `tests zena` menu description fits on one line of the menu.
- [x] `tests zena` and `tests native debug` pass.
- [x] `lint` passes.

## Review notes
- 2026-10-08 delivered (host session, backlog-burndown-001-q4), commit
  `fix(zena): Keep gating under an inherited regenerate variable`.
  - Inherited variable: `testWorkspace`, which every `tests` lane runs first,
    unsets `WCMP_ZENA_REGENERATE`, `WCMP_ZENA_WEB_RUN` and `WCMP_ZENA_REPORT`,
    so only `tests zena` sets them, after it. Checked by hand: with
    `WCMP_ZENA_REGENERATE` exported and `async-sleep web` changed to `link` in
    the record, `tests zena` failed ("`async-sleep web` stopped at `pass` where
    the record holds `link`") and wrote no record.
  - Browser parsing: `browser_reports_in(output, scenarios)` is a pure function
    that refuses a line that is not a report, a report of another subject, a
    scenario the bundle lacks, a scenario reported twice, and a run that misses
    a scenario. Seven tests (both targets): it reads every report and nothing
    else; empty output and output with no report line; a missing and a stray
    scenario; each other subject; a duplicate; five kinds of garbage (empty,
    word, no stage, unknown stage, unknown subject); and a wrong stage in the
    browser's run fails the gate with a `Stage` difference. Each asserts the
    refusal's message, so removing a check fails its test.
  - Menu: "Zena gate and report on every subject; `regenerate [--dry-run]`
    rewrites the record".
- 2026-10-08 gate on the series tip (`fix(zena): Move the Zena input back
  from the fork to Zena`): `tests all` green on every lane (native debug and
  release 1730, web debug and release 1716, no-provider 169 on each target,
  Wasmi 1447, fidelity 264/267/264, Zena Browser 18/18, Native 18/18, Wasmi
  2/18, Wasmtime 18/18, demo 68/68), with no `unexpected:` or `stale
  expectation:` line, and the full `lint` green. Conformance: native
  2412/2443, browser 2407, Wasmi 2352, no provider 2356.
- 2026-10-08 context-free review, round 4: **accepted**, with no code finding.
