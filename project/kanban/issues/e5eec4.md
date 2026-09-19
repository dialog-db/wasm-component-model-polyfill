---
id: e5eec4
title: Restore the integration proof that a host function blocks and resumes
type: chore
blocked_by: [f38326]
labels: [PDD019, concurrency]
created: 2026-09-18T17:08:59Z
---

## What to build
Card `ebb8fc` renamed `it_suspends_a_host_function_the_guest_called_until_a_host_task_completes` in `src/concurrency/suspend_seam.rs` to `it_refuses_a_block_in_a_host_function_a_synchronous_export_called`, because a host function called from a synchronous export now runs in a task that may not block, as the reference and Wasmtime require. The review noted that the renamed test was the only integration-level proof that a host function can block through a real trampoline and be resumed by the nested turn polling a host task and running its lowering; the unit-level proof at `suspend_seam.rs` (around line 570 at `0408a60`) survives. The positive path is unreachable from a synchronous export by design and becomes reachable again once a host call into a callback export runs as a task that may block. When that card lands, add back an integration test with the old shape on the new path: a callback export calls a host function whose result depends on a host task; the nested turn polls the host task, runs its lowering, and the host function returns; assert the observable result and the record shape. Also cover the two gaps the same review named: `waitable-set.wait`, `poll`, and `drop` each trap on an index that does not name a waitable set, and `Scheduler::take_ready_in`'s switch-slot branch takes an item of the asked-for instance and skips one of another.

## Acceptance criteria
- [ ] An integration test proves a host function called from a callback export blocks on a host task and resumes through the nested turn, through a real trampoline.
- [ ] `wait`, `poll`, and `drop` each have a test that traps on an index that is not a waitable set.
- [ ] `take_ready_in`'s switch-slot branch has a unit test.
- [ ] `lint` passes and `tests all` is green on both targets with the conformance summary unchanged.


## Dispatch log
- 2026-09-19T01:51:28Z dispatched implementor `card-e5eec4-22813239` (implement session, PDD019 thread, budget 3; seed at 562fe7f)
- 2026-09-19T04:05:07Z implementor reported done at `17ad922` (five commits; `CALLBACK_CALLS_THE_HOST` + `it_suspends_a_host_function_a_callback_export_called_until_a_host_task_completes` in the seam, three not-a-set traps for wait/poll/drop, two `take_ready_in` switch-slot tests; `lint`, `tests all`, `tests conformance` green, baseline unchanged). Fetched, moved to needs-review, paused the implementor.
- 2026-09-19T04:05:51Z launched reviewer `review-e5eec4-ed2afb0f`; delivered `sandbox-guest/card-e5eec4-22813239` (tip 17ad922) as `delivered/card-e5eec4-22813239`.
