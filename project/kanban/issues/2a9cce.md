---
id: 2a9cce
title: Waitable and waitable set records with event delivery
type: feature
blocked_by: [377cb2]
labels: [PDD018, concurrency]
created: 2026-09-16T05:54:44Z
---

## What to build
Add the store's waitable records and waitable set records, with no built-in. A waitable is a subtask, a readable or writable stream end, or a readable or writable future end. Every waitable record holds one pending event slot, the set it joined if any, and the synchronous-waiter flag. A waitable set holds the list of its waitables and the count of threads waiting on it. An event is a code and two payloads; the codes are the reference's `EventCode`: none (0), subtask (1), stream read (2), stream write (3), future read (4), future write (5), and task cancelled (6). For a subtask event the payloads are the subtask's index in the handle table and its state; for a copy event, the waitable's index and the copy result. Record readiness by filling the pending event slot. Deliver an event when a thread waits on or polls a set that contains the waitable, and when a callback returns the wait code with that set; delivery empties the slot. Enforce the record-level rules: a set delivers events in the order its waitables joined it; a wait on a set that already holds an event returns at once; joining a waitable to a set removes it from its previous set, and joining one that has a synchronous waiter traps; dropping a set that still holds waitables traps, and so does dropping a set a thread is waiting on; dropping a subtask whose resolution was not delivered traps. Delivering a subtask's resolution decrements the counts on its lenders. Expose these as store operations for the built-ins of later features to call.

## Acceptance criteria
- [ ] A test fills events on two waitables of one set and reads them back in join order.
- [ ] A test shows a wait on a set that already holds an event returns at once, and that delivery empties the slot.
- [ ] A test shows joining moves a waitable between sets, and that joining one with a synchronous waiter traps.
- [ ] Tests show the three drop traps: a set that still holds waitables, a set a thread is waiting on, and a subtask whose resolution was not delivered.
- [ ] A test shows delivering a subtask's resolution decrements its lenders' counts.
- [ ] The tests run on both targets.


## Dispatch log
- 2026-09-16T20:44:08Z dispatched implementor `card-2a9cce-b0055f33` (implement session, PDD018 thread, Opus 5; seeded after 87cf207 landed)
- 2026-09-16T21:57:35Z implementor reported done at `a079b7b` (gate green; new `Error::Waitable` variant; waitable state on the `Subtask` record). Fetched, needs-review, paused the implementor; launched reviewer `review-2a9cce-b7a48f16`, delivered the branch.

## Review notes

- 2026-09-16 reviewer `review-2a9cce-b7a48f16` on tip `a079b7b`: **accept**. All six criteria and all seven record rules enforced in the records (`waitable_state.rs:20-32`, `waitable_set.rs:13-18`); `EventCode` matches `definitions.py:695-702`; nine integration tests run in both lanes and assert record state; trap strings match `cm/async/drop-waitable-set.wast:84`, `wasmtime/async/drop-host.wast:56`, `sync-and-async-waitable.wast`. `Error::Waitable(WaitableCause)` judged the right shape (one variant per subsystem with a structured cause, like `Abi` and `Scheduler`); the fourth trap is required by `definitions.py:793` and absent in Wasmtime. Non-blocking, filed as `8ab86d`: `join_waitable_set` and `begin_wait` mutate before validating (internal-error paths); `TaskTables::take_pending_event` is public without delivering the resolution; `discard_scope`/`exit_subtask` remove a subtask without leaving its set (latent ABA with index reuse). For the owner: `waitable_set.rs:9-11` and PDD018 line ~473 say join order is what Wasmtime delivers, but Wasmtime pops a `BTreeSet<Waitable>` (identity order); and PDD018 §Error Model Growth still names only the scheduler variant. Gate: `tests all` native 379, web 306 both green; `lint` 9/9. Menu gap noted: no test-name filter on `tests`.
