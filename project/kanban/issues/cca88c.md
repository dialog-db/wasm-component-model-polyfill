---
id: cca88c
title: Give the subtask, thread, and waitable set identities a generation
type: bug
blocked_by: []
labels: [PDD018, concurrency]
created: 2026-09-17T05:01:51Z
---

## What to build

`TaskId` now carries the generation of its record-table slot, so a stale task identity cannot alias a later task. `SubtaskId`, `ThreadId`, and `WaitableSetId` still index their tables bare (`src/concurrency/task_tables.rs` around lines 238, 243, 248, 319, 333, 500), so a reused subtask index lets `add_lender` succeed against the wrong record, and `HandleLookupError::NoCallInFlight`'s doc ("the scope the caller named has already ended") holds for a task scope but not a subtask scope. Give the three remaining identities the same generation, route every accessor through a generation check, and make the doc true. Also: `create_task` discards `insert`'s return without asserting it equals the index whose generation was read; add the `debug_assert`. Add a test per identity that reuses a freed index and shows the stale identity resolves to nothing.

## Acceptance criteria

- [ ] A stale `SubtaskId`, `ThreadId`, or `WaitableSetId` resolves to no record after its slot is reused, proved by a test each.
- [ ] `lint` passes and `tests all` is green on both targets with the conformance summary unchanged.

## Review notes


Follow-up from the review of 5e41bf (`review-5e41bf-9c15c273`).

## Dispatch log
- 2026-09-17T07:50:19Z dispatched implementor `card-cca88c-15a5025c` (implement session, PDD018 thread, Opus 5; seeded after 2218778 landed)
