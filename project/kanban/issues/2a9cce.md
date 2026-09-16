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

