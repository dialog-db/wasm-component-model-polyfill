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

