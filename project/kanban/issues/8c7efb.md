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
- 2026-09-21T06:16:50Z implementor reported done at `5152d01` (fix `5fc6c2f`, test `752a50e`, docs `7aa3e77`, reflow on seed `8b04854`). Root cause of both `reentrance.wast` defects: a turn ran the work it had just released (gate or held callback) before its driver could consult its condition, so a held callee overwrote the caller's answer — fix: a turn that releases work ends with Progress and hands it to the next turn (Wasmtime's `poll_until` polls the future ahead of every item); both defect directives now pass, two list lines removed, native 1692 → 1694, browser 1677 → 1681, defect column 0; criterion-4 test on a backpressure-held callee; five reentrance rules on `enter_implicit_thread`, release half on `open_entry_gate`; six collateral tests updated to the next-turn rule; `tests all` native 708 / web 698, `lint` 9/9. For the owner: PDD020's Test Cases says the Wasmtime `reentrance.wast` callee "completes when it exits" but it cannot (its `call_indirect` would trap; the directive passes because the held callee never runs); the next-turn rule is stated nowhere in the corpus and PDD018's turn pseudocode omits the gate. Menu gaps: `wasm-tools` not on the dev-shell PATH; no per-file conformance runner. Overlaps 98606b (in review) on `baseline_async_start_call.rs` and 17776b (in review) on `store_context.rs`. Fetched, moved to needs-review, paused the implementor.
- 2026-09-21T06:16:50Z launched reviewer `review-8c7efb-4301081d`; delivered `sandbox-guest/card-8c7efb-d2ee433a` (tip 5152d01) as `delivered/card-8c7efb-d2ee433a`; the directive asks for a specific verdict on the scheduler-rule change and to reconcile the two readings of the shim's `call_indirect` index.

## Review notes
