---
id: aea6dd
title: Two Zena components link at run time
type: feature
blocked_by: [568112]
labels: [PDD024, zena]
created: 2026-09-27T16:53:37Z
---

## What to build
Scenarios with more than one component, linked at run time, starting with two Zena components.

- A scenario can hold several Zena programs, one WIT world that is the contract between them (each compiles with `--wit` and `--world`), and the wiring: which component's exports satisfy which component's imports, and whether the link is made at run time or by composition. Expectations entries name the component they call.
- At run time, the runner instantiates the exporting component first and gives that instance's exports to the importing component through the polyfill's `Linker`. The Wasmtime run does the same through Wasmtime's `Linker`.
- Scenario 8: Zena imports from Zena at run time, passing a string across the link.
- Regenerate and commit the record lines.

Notes from the review of 264c89 (landed as `b882bd371`):
- The polyfill runner instantiates each component on its own (`tests/zena/runner.rs:81-86`). Run-time-linked scenarios need runner code on both the polyfill and the Wasmtime side.
- The polyfill links and instantiates in one call, so the runner tells `link` from `instantiate` by error variant. With several components, the stage order can differ from the Wasmtime run, which parses all, links all, then instantiates all.

## Acceptance criteria
- [ ] Scenario 8 runs under all three subjects, linked at run time.
- [ ] The record holds lines for scenario 8, and `tests zena` passes the gate.
- [ ] `tests native debug`, `tests web debug`, and `lint` pass.


## Dispatch log
- 2026-09-28 00:07 PDT: implementor `card-aea6dd-383529a3` dispatched (after the weekly GC; runs beside 7bf3c5 and ff3743).
- 2026-09-28: the drive exited after the guest nudges ran out (00:3x PDT) while the agent kept working; the agent finished at 08:43Z with a full summary filed as a non-terminal report instead of `report done`. Treated as done at `d063e00bb` (run-time link through both runners, scenario 8, record lines; `tests all` green at `45823632e`, `lint` green at `d063e00bb`, which only adds a rustfmt commit). Scenario 8 passes on Wasmtime and stops at `parse` on web and native. Design notes: a run-time link only works for async-typed functions (Wasmtime 49 panics on a nested `run_concurrent`; both refuse a concurrent host function for a sync-typed import), so scenario 8 uses `greet: async func`; the polyfill can report `instantiate` where Wasmtime reports `link`; composition links in `wiring.txt` are refused until the wac step exists. Implementor paused.
- 2026-09-28: reviewer `review-aea6dd-f65e025b` launched; delivered tip `d063e00bb`.
