---
id: 9debd3
title: Thread-transparent adapters run with no task of their own, and the docs say so
type: feature
blocked_by: [90d64c]
labels: [crates-io, wasmtime-50]
created: 2026-10-20T19:10:42Z
---

## What to build

Wasmtime 50 skips `enter-sync-call` and `exit-sync-call` for a
thread-transparent adapter (`thread_transparency.rs:141-157`). Such an adapter
has no async side and no handle in its signature, and its callee has no
built-in that reads or writes thread state. The callee runs on the caller's
task and thread. No guest can see the difference.

The polyfill inherits this from `wasmtime-environ`. This card proves it and
records it.

## Acceptance criteria

- [ ] A test runs an opaque, then a transparent, then an opaque callee on both
      targets, and the task records show no task for the middle call.
- [ ] A `context.get` in the innermost callee sees a fresh thread.

## Review notes

