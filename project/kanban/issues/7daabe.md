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

- [x] No PDD states a rule that the polyfill no longer follows.
- [x] Each revised PDD is marked for the owner's re-acceptance.
- [x] `markdown` passes.

## Review notes

- 2026-10-20: Revised in place for Wasmtime 50 and spec `2f1e56f`: PDD015,
  PDD016, PDD018, PDD019, PDD020, PDD021, PDD022, PDD023, PDD025, PDD027, and
  PDD028. Each one waits for the owner's re-acceptance. PDD000 to PDD014,
  PDD017, PDD024, and PDD026 were checked and needed no change. Every
  Wasmtime citation in a revised PDD points at `v50.0.0`.
- 2026-10-20: Owner items found in the sweep. PDD003 still lists `may_enter`,
  which spec #705 removed before the old pin. Its gate list lacks ➡️
  (`stream.forward`) and 📡 (getters and setters), and its streams table has
  no `forward` row, which needs a target such as Deferred. PDD022 does not
  state that a call's first thread is counted but never refused at the handle
  cap. That departure is only in `notes/differences-from-wasmtime.md`.
