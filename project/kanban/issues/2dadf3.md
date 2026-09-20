---
id: 2dadf3
title: A synchronous start removes its subtask when the wait fails
type: bug
blocked_by: [7e0512]
labels: [PDD020, concurrency]
created: 2026-09-19T22:32:22Z
---

## What to build
In `src/executor/sync_start_call.rs` (around line 170), only the trap path calls `remove_subtask`. When `SuspendSeam::suspend` returns an error (the cannot-block, deadlock, or stack-switch cause), the caller's call fails but the subtask record and the callee's task stay in the store. Make the failure of the wait release what the trap path releases: remove the subtask, abandon the callee's task, and release the callee's exclusive thread, so the store is left as it was before the call.

## Acceptance criteria
- [ ] A synchronous start whose wait fails with the cannot-block cause leaves no task, no subtask, and no held instance in the store, proved by a test that pins the cause and asserts the counts.
- [ ] The same holds for the deadlock cause.
- [ ] `lint` passes and `tests all` is green on both targets with the conformance summary unchanged.

## Dispatch log
- 2026-09-20T04:45:52Z dispatched implementor `card-2dadf3-b22e6007` (implement session, PDD020 thread, budget 3, seed `74821db` with 708ab2 and 5e5da6 ready)
- 2026-09-20T05:58:19Z implementor reported done at `49932cf` (one commit on seed `dbaf14e`; `release_wait` on the `Err` from `SuspendSeam::suspend` abandons and removes the subtask and calls `end_export_task` (chosen over `abandon_export_task` because the callee's scope is off the stack whenever a wait can fail); two tests on `WAITS_FOR_EVER` and a new `WAITS_FOR_EVER_UNDER_AN_ASYNC_CALLER` pin the cannot-block and deadlock messages and assert task/subtask counts 0 and no held instance, both verified to fail with the fix reverted; `tests all` native 625 / web 613, conformance unchanged, `lint` 9/9). Flagged for the owner: a callee still held at the entry gate when the wait fails leaves its start item queued and `waiting_to_enter` raised, and that item would later run against a removed task record — not reachable from the tests, same shape as `CallbackTask`'s failure path. Overlaps the 1b6304 landing on `tests/baseline_prepared_call.rs`. Fetched, moved to needs-review, paused the implementor.
- 2026-09-20T05:58:19Z launched reviewer `review-2dadf3-3e5ac884`; delivered `sandbox-guest/card-2dadf3-b22e6007` (tip 49932cf) as `delivered/card-2dadf3-b22e6007`.

## Review notes
