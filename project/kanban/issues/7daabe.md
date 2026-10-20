---
id: 7daabe
title: The design docs describe the Wasmtime 50 behavior
type: docs
blocked_by: [90d64c, 0137e0, 643072, fdb910, 9debd3]
labels: [crates-io, wasmtime-50, PDD]
created: 2026-10-20T19:11:39Z
---

## What to build

PDD018, PDD020, and PDD023 state behavior of Wasmtime 49 that the port
changes: the cancellable immediate, the routing of sync calls through
`enter-sync-call`, the same-instance call, and the enter intrinsic that pushes
a task on every sync call. Revise each document in place. Write the revised
text as the design the project intends, not as a change log.

Every changed passage goes through the simple-english skill and then the
humanizer skill.

## Acceptance criteria

- [ ] No PDD states a rule that the polyfill no longer follows.
- [ ] Each revised PDD is marked for the owner's re-acceptance.
- [ ] `markdown` passes.

## Review notes

