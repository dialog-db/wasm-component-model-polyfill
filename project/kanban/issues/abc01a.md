---
id: abc01a
title: A failed instantiation leaves no records in the store
type: chore
blocked_by: []
labels: [correctness-performance-checkup-01, concurrency]
created: 2026-09-21T17:05:56Z
---

## What to build
`src/executor/instantiate.rs:93-101` inserts one instance record per component instance, and lines 141-168 register destructors and resource names, before any fallible step of the plan runs. A start function that traps (around lines 264-272) or a link error from resolving the resource runtime returns without removing them; the start task's drop does restore the may-not-suspend flag (`src/executor/start_task.rs:76-86`), so the effect is growth per failed attempt rather than a wrong flag. Reserve the records and commit them once the plan has run, or remove them on the failure paths, so a failed instantiation leaves the store as it found it. Wasmtime's store also keeps partial instances, so this is hygiene rather than parity; say so in the docs of `Linker::instantiate`.

## Acceptance criteria
- [ ] After an instantiation that traps in a start function, and after one that fails to link a resource runtime, the store's instance records, destructor registrations, and resource names equal those before the attempt, proved by tests.
- [ ] `lint` passes and `tests all` is green on both targets with the conformance summary unchanged.



## Dispatch log
- 2026-09-22T17:45:08Z dispatched implementor `card-abc01a-c64e5715` from tip `6886b73`. The host drive was lost with its shell before the directive reached the VM, which came up idle; redelivered with `sandbox prompt --redeliver`.

## Review notes
- 2026-09-22T20:22:33Z **host crash recovery.** The host rebooted at about 20:02Z. The branch carries the card's work at `cbf464b` "fix(executor): A failed instantiation leaves no records in the store"; its last report (18:35:39Z) said it was implemented and green on the native lane and had found the web lane failing to compile at its dispatch seed in `src/baseline/prepared_call.rs` — the pre-existing break the host fixed as `35990fe`. My fix note never reached the VM (the prompt transport refused while turn 1 was in flight), so the branch does not carry the repair and the implementor never filed a terminal report. Fetched from the on-disk mirror. Moved to needs-review on the strength of the native result; the host's composed-tip gate (which already contains `35990fe`) runs the web lanes on landing. The reviewer should confirm the work is complete, since no `done` summary exists. Implementor stopped, not removed.
- 2026-09-22T20:31:14Z launched reviewer `review-abc01a-ebcb6515` (reads-only) after crash recovery; delivered `sandbox-guest/card-abc01a-c64e5715` (tip cbf464b) as `delivered/card-abc01a-c64e5715`.
