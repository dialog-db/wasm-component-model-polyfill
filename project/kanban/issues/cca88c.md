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

- 2026-09-17 reviewer `review-cca88c-bef027c7` on tip `d80df21`: **accept**. All three identities carry a generation; `from_index` gone (only `InstanceId` keeps it, over a never-shrinking `Vec`); identities minted in four places inside `TaskTables`; `insert_with_generation` returns the stored pair because `remove`, not `insert`, advances the generation. `HandleKind::Subtask { subtask }`/`WaitableSet { set }` judged entailed, not a widening: `waitable_from_handle` was the one site re-minting an id from a raw entry index, and a bare `u32` there would reintroduce the aliasing the card closes; nothing guest-visible changed. Falsified: weakening the three `*_index` helpers to the bare index fails exactly the four new tests. Gate: native 422, web 322 both profiles; conformance unchanged; `lint` 9/9. Nits: a dead `if let` arm in `drop_waitable_set` (`task_tables.rs:607`); `insert_waitable_set`'s doc still says "the identity's index"; `RecordTable::generation` wraps. For the owner: "generation" appears nowhere in PDD018, so §The Handle Table's "the index of the subtask record" is stale for all four identities now. Landing note (orchestrator): `d552c8` landed after this card's seed with `guard.insert_subtask(caller, subtask.index())` (`store.rs:476`) and a `HandleKind::Subtask { .. }` pattern (`:1022`), both of which this card's signature change breaks — a squash would not compile; reconciling onto the tip after `d34ad1` lands.


Follow-up from the review of 5e41bf (`review-5e41bf-9c15c273`).

## Dispatch log
- 2026-09-17T07:50:19Z dispatched implementor `card-cca88c-15a5025c` (implement session, PDD018 thread, Opus 5; seeded after 2218778 landed)
- 2026-09-17T09:07:49Z implementor reported done at `d80df21` (gate green; `HandleKind::Subtask`/`WaitableSet` now carry the identity). Fetched, needs-review, paused the implementor; launched reviewer `review-cca88c-bef027c7`, delivered the branch.
- 2026-09-17T09:51:05Z reviewer accepted; removed the reviewer; landing held behind d34ad1 because d552c8 (eae3065) added callers of the APIs this card changed — a reconciliation sandbox will adapt `store.rs:476` and `:1022` onto the tip.
- 2026-09-17T10:31:42Z launched reconciliation sandbox `card-cca88c-08667081` with the branch delivered (adapt the d552c8 callers onto 99c1b0e).
- 2026-09-17 reconciliation by `card-cca88c-08667081`, tip `7ddfb9b` (merge of `d80df21` into `777c8d4`): no textual conflict; two callers adapted mechanically — `store.rs:548` `insert_subtask(caller, subtask.index())` → `insert_subtask(caller, subtask)` and the test expectation at `store.rs:1094` to `HandleKind::Subtask { subtask }`; grep found no other host-added site. No behaviour choice; nothing else changed on either side. Gate on the merged tree: `tests all` native 445/445, web 345/345 both profiles; conformance 1514/2392, 0 unexpected, 0 stale; `lint` 9/9. Mechanical, so not returned to review.
