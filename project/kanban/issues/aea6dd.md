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

