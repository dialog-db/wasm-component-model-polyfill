---
id: 8c7efb
title: The gate holds a callee and reentrance does not trap
type: feature
blocked_by: [3e657c]
labels: [PDD020, concurrency]
created: 2026-09-19T06:42:49Z
---

## What to build
Settle the reentrance rules on the prepare-and-start protocol. No call traps for reentrance: the adapters of Wasmtime 49 emit no `CannotEnterComponent` for a call between a parent and a child or into the caller's own instance, and the polyfill raises it nowhere; a sync-typed callee can be entered at any depth, from a child, a parent, a sibling, a destructor, or the host, and the synchronous baseline serves each as a nested call on the real stack. The entry gate is the only serialization: an async-typed callee lifted synchronously or with a callback needs the exclusive thread of its instance, so a reentrant call into it waits at the gate while the holder runs core code; a callback holder releases between events, and the call proceeds; a synchronous holder releases on return, so a cycle through it goes idle and fails with the deadlock cause. A subtask held at the gate by backpressure reads `STARTING`, and a wait on it with nothing else ready fails with the deadlock cause. The host can always enter; Wasmtime 49 refuses a host entry only into a store a trap poisoned, and the poisoning rules are out of scope. The may-leave flag is unchanged: a lowered import called from a realloc or a post-return fails with the cannot-leave cause of PDD019. Verify each rule against `Scheduler::enter_implicit_thread` and `open_entry_gate` in `src/concurrency/scheduler.rs` and the two start trampolines, fix what the corpus files find, and write the rules into the docs of the gate.

## Acceptance criteria
- [ ] `backpressure-deadlock.wast` passes: the subtask reads `STARTING` under backpressure and the wait fails with the deadlock message.
- [ ] The Wasmtime `reentrance.wast` passes: a callback export calls a child's callback export, which calls back into the root through a table and waits at the root's gate while the root's own task runs, completes when it exits, and the host reads the root's result.
- [ ] The eight owned directives of the Component Model `reentrance.wast` pass: the five synchronous cases through the adapters of Wasmtime 49 alone, the sync-typed reentry while the exclusive thread is held, the callback cycle that waits at the gate and completes, and the synchronous cycle that fails with the deadlock message.
- [ ] A repository test proves that a held sync export runs when the gate opens and delivers `RETURNED` to a caller that read `STARTING`.
- [ ] `lint` passes and `tests all` is green on both targets, and any async directive that starts passing has its line removed from the expected-failure list.

## Dispatch log
- 2026-09-21T03:15:43Z dispatched implementor `card-8c7efb-d2ee433a` (implement session, PDD020 thread, budget 3, seed `48b2865` with 3e657c ready and tip-gated)

## Review notes
