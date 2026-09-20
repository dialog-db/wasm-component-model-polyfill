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

## Review notes
