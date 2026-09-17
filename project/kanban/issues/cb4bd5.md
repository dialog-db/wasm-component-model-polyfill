---
id: cb4bd5
title: Keep the exit guards from aborting on a panic inside a step
type: bug
blocked_by: []
labels: [PDD018, concurrency]
created: 2026-09-18T00:42:44Z
---

## What to build

The `TaskExit` guard in `src/resource/tables.rs` (around lines 620-702) finishes a task exit that a panic interrupted, but `run` advances its step only after a step returns, so a panic inside a step makes `Drop` re-run that same step during the unwind — a double panic, which aborts the process (reproduced by the reviewer with a panic injected into the undo-lends arm). Advance the step before running it (or mark the failing step as taken) so `Drop` continues from the next step, and make the doc's "the finishing drop cannot panic" claim true rather than asserted. Give `exit_subtask` (around line 123) and `discard_scope`'s subtask branch (around line 319) the same guard, since they have the same multi-step shape. Drive the tests through `exit_task` and `discard_scope` themselves, with the panic injected inside a step, not through a hand-built guard that panics before any step. Also stop `turn_in_flight()` from clearing poison as a side effect of a query; put the recovery where the turn is entered.

## Acceptance criteria

- [ ] A panic inside any step of `exit_task`, `discard_scope`, or `exit_subtask` leaves the tables consistent and does not abort the process, proved by native tests that inject the panic inside a step.
- [ ] `turn_in_flight()` has no side effect.
- [ ] `lint` passes and `tests all` is green on both targets with the conformance summary unchanged.

## Review notes


Follow-up from the review of 52367e (`review-52367e-86a951c4`).
