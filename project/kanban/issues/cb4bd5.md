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

- 2026-09-18 reviewer `review-cb4bd5-b6a04f11` on tip `5838c7a`: **accept**. Every non-unwind step taken before its work (`tables.rs:735-768, 852-888`); `unwind_one` pops before the discard; all three exits guarded; `in_turn` reads without clearing. Empirically verified in a throwaway copy: reverting the ordering fails `it_finishes_a_task_exit_a_panic_inside_a_step_interrupted` (`tables.rs:1383`), and with a persistent panic request reproduces the `SIGABRT`; the delivered ordering unwinds once. Gate: native 470, web 355 both profiles; conformance unchanged; `lint` 9/9. Non-blocking, filed as `04518d`: test 4 never asserts the lend it loses; the one-shot `panic_in_step` prevents demonstrating the abort; `unwind_one`'s second justification does not hold; `TaskExit`'s doc names panic sources that do not exist (the guard is defensive; the residual double-panic path is unreachable); `exit_task`'s public doc omits the cost; `store_data.rs:199-206` overstates when poison is cleared (`Func::call` locks before any turn); no tests for a panic in deliver-resolution, count-borrows, remove-record, a task-scope discard, or `exit_subtask`'s unwind.


Follow-up from the review of 52367e (`review-52367e-86a951c4`).

## Dispatch log
- 2026-09-18T00:44:21Z dispatched implementor `card-cb4bd5-a6ad9991` (implement session, PDD018 thread, Opus 5; seeded after f2880a6 landed)
- 2026-09-18T02:03:15Z implementor reported done at `5838c7a` (gate green). Fetched, needs-review, paused the implementor; launched reviewer `review-cb4bd5-b6a04f11`, delivered the branch.
