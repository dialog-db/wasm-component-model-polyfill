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
- 2026-09-20T07:26:41Z reviewer `review-2dadf3-3e5ac884` on tip `49932cf`: **accept**, blocks on nothing. Gates in the reviewer's VM: `tests all` native 625 / web 613 both profiles, conformance identical to the README, `lint` 9/9; both tests verified to fail with the release hunk reverted. Release matches the trap path (handles delivered before the record goes; lend sets disjoint so the reversed order is safe); `end_export_task` is a strict superset of `abandon_export_task` since `TaskExit`'s unwind step is a no-op off-stack; the on-stack case (callee blocked below the trampoline) lands in the `StartFailure` slot and never reaches `release_wait`. Findings, none blocking: (1) **reproducible, follow-up card**: `sync_start_call.rs:419-423` removes the task record but leaves every queued item naming that task — with a plain component (sync-typed caller, callback callee whose first status word is YIELD, no third task) the caller's nested turn may not run the callee's instance, the wait fails cannot-block, and the next driver turn now fails with `polyfill internal invariant violated: an export's task is not in the store` (`callback_task.rs:193-197`, `:124`) where it used to fail with the domain error `async-lifted export failed to produce a result`; same shape for the held-callback item both tests create, a high-priority item from a wait on a filled set, and the gate entry the implementor named — a design question PDD019/020 do not answer; (2) `sync_start_call.rs:44-46, 409-411` claim the release gives back the callee's exclusive hold, but on every reachable path the callee released before the wait could fail (instrumented: `callee_instance_held_by_callee=false` in both tests), so `exit_implicit_thread` is a no-op and the `!any_instance_is_held` assertion is a regression guard, not a proof; (3) `:402-404` says the wait fails with a scheduler cause but `run_nested_turns` also propagates an item's own bookkeeping failure. For the owner: PDD020 does not state the wait-failure release rule; it lives only in the Rust module doc. Menu gap: no single-test runner (used raw `cargo test` for three probes outside the branch).
- 2026-09-20T07:26:41Z landed as `5839cc6` (clean squash; `sync_start_call.rs` byte-identical to the branch tip; `tests/baseline_prepared_call.rs` auto-merged with the 1b6304 landing, delta verified against each parent). Landing commit made with `jj commit`. Card stays needs-review until the composed tip is gated. Pair removed.
