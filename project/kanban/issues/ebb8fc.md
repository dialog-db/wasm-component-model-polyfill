---
id: ebb8fc
title: Waitable set built-ins from one task
type: feature
blocked_by: [e4cb68, 2061e4]
labels: [PDD019, concurrency]
created: 2026-09-18T07:19:48Z
---

## What to build
Map `Trampoline::WaitableSetNew`, `WaitableSetWait`, `WaitableSetPoll`, `WaitableSetDrop`, and `WaitableJoin` in `src/executor/translate.rs` to specs and build them over the store's records (`TaskTables::insert_waitable_set`, `begin_wait`, `end_wait`, `next_ready_waitable`, `take_pending_event`, `drop_waitable_set`, `join_waitable_set`, and `HandleTables::wait_on_waitable_set` in `src/resource/tables.rs`) through the current instance's handle table, in the waitable set entry kind the handle table reserves. Every one traps with the cannot-leave cause when the instance's `may_leave` is clear, and each but `new` traps when the index does not name a waitable set. `waitable-set.new` inserts a set record and returns its index. `waitable-set.wait` takes a set index and a pointer: with an event, it takes the first in join order, writes the two payloads as `u32` values at the pointer and the pointer plus four in the built-in's memory, and returns the code; with none, it asks `SuspendSeam::suspend` to suspend the thread until the set holds an event. `waitable-set.poll` takes the same arguments, never blocks, and returns the none code and writes nothing when the set is empty. `waitable-set.drop` removes the entry and traps with the waitable causes when the set still holds a waitable or a thread waits on it. `waitable.join` takes a waitable index and a set index; zero removes the waitable from its set, otherwise the waitable moves into the named set and leaves its previous one; it traps when the first index is not a waitable, when the second is not a set, and when the waitable has a synchronous waiter. This card also brings the lazy blocking rule to the nested-turn fallback of the seam (`src/concurrency/suspend_seam.rs` and `StoreContext::nested_turn` in `src/store/store_context.rs`): a task whose instance may not suspend first runs only the ready work of its own instance, then fails with the cannot-block cause when the condition still does not hold; a task allowed to block fails with the deadlock cause when the store goes idle and with the stack-switch cause when a host task is still pending. For that rule to hold, the task a start function runs in during instantiation and the task of `Func::call` into a synchronous export both may not suspend: set the instance's may-not-suspend flag for the length of the call and restore it, as the enter and exit intrinsics do for a synchronous call between components. In this design every waitable kind is reserved, so `waitable.join` finds nothing to join in the corpus; prove it by inserting a subtask waitable through the store's records. The first directive of `dont-block-start.wast` passes after this card; its second stays deferred with the sibling-call reason.

## Acceptance criteria
- [ ] The first directive of `dont-block-start.wast` fails instantiation with the cannot-block message and its line leaves the list; the two components of `task-builtins.wast` that define only `waitable-set.wait` and only `waitable-set.poll` instantiate.
- [ ] `waitable-set.wait` from a synchronous export on a set that holds an event returns that event without blocking, and on an empty set fails with the cannot-block cause, each proved by a test.
- [ ] `waitable-set.poll` on an empty set returns the none code and writes nothing; `waitable-set.drop` on a set that holds a waitable, and on a set a thread waits on, fails with the waitable causes; `waitable.join` with a set index of zero removes the waitable from its set, and it traps on a first index that is not a waitable, a second index that is not a set, and a waitable with a synchronous waiter; each proved by a test.
- [ ] Each of the five built-ins fails with the cannot-leave cause when a realloc calls it, proved by a test.
- [ ] A synchronous task that must block runs only the ready work of its own instance before it fails with the cannot-block cause, proved through the store's records with a queued item of another instance that does not run.
- [ ] `tests all` passes on both targets, and any async directive that starts passing has its line removed from the expected-failure list.


## Dispatch log
- 2026-09-18T13:18:23Z dispatched implementor `card-ebb8fc-9779c7dc` (implement session, PDD019 thread, budget 3; seed at 6671fd6)
