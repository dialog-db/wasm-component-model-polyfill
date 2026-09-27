---
id: aa22ed
title: A trapping caller's outstanding lends come back, and the lend tests pin the delivery moment
type: chore
blocked_by: []
labels: [resource]
created: 2026-09-22T01:32:37Z
disposition: cancelled
disposition_at: 2026-09-27T05:40:39Z
---

## What to build
The review of card `558fcd` accepted the rule that a call's lends live on the record of the call and left findings that need one card. First, a hole the reviewer traced but did not execute: a prepared-call subtask is never on the scope stack, so `unwind_one`'s discard (`src/resource/tables.rs:416`) never reaches it; a caller that traps while an asynchronous prepared call is outstanding leaves its lent owning entry at `lend_count == 1` with nothing left to give it back, where before the callee task's exit returned it. The same shape already exists for guest-to-host subtasks. Decide what a trapping caller's outstanding lends do and make the unwind reach them. Second, the tests: `tests/baseline_async_start_call.rs:562-564` has the caller's callback call `subtask.drop` before `drop-thing`, so the positive test pins "released no later than `subtask.drop`" rather than the delivery moment — swap the two lines; the negative control `it_keeps_a_handle_lent_until_the_resolution_is_delivered` traps on both the old and the new code, so it does not discriminate; the host-lend test manufactures the lend through `HandleTables::lend_to` from inside a host import rather than through a real borrow lower (card dda886 adds that lower; once it lands, lend through it); the `Func::call` half of the rule stated at `src/instance/func.rs:181-186` has no test; the nested guest-to-guest-to-guest lend and the enter/exit-sync-call lend that the rule's third bullet asserts have none either. Third, the diff deleted the sentence in `src/concurrency/task.rs` that said Wasmtime keeps the lender list on the callee's task (`concurrent.rs:2432-2444`, released at `validate_scope_exit`), and the rule at `tables.rs:481-527` names only the borrow-entry departure; state the guest-to-guest departure from Wasmtime in one sentence beside it.

## Acceptance criteria
- [ ] A caller that traps with an asynchronous prepared call outstanding gives its lent owning entry back (or the design states why not), proved by a native test: the caller lends a borrow through an async guest-to-guest call, traps, and a second call into the caller's instance drops that owning handle.
- [ ] The positive delivery test pins the delivery moment, and a negative control fails on the old placement of the lend.
- [ ] `Func::call`'s host lend, a nested guest-to-guest-to-guest lend, and the sync-call lend each have a test.
- [ ] The rule at `HandleTables::lend_to` names the guest-to-guest departure from Wasmtime beside the borrow-entry one.
- [ ] `lint` passes and `tests all` is green on both targets with the conformance summary unchanged.

## Triage notes
- 2026-09-26: merged into `dced20` ("Resource handles: a stale host index is refused, a host borrow lowers back, a defining guest's borrow reaches a host, and lends come back after a trap") and cancelled. Its What to build and acceptance criteria carry over there in full.
