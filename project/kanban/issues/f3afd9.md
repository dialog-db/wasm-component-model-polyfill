---
id: f3afd9
title: Accessor::spawn and Access::spawn start background host tasks, as Wasmtime does
type: feature
blocked_by: [3200e7]
labels: [bindgen, bindgen-deferred, concurrency]
created: 2026-10-16T23:39:35Z
---

## What to build
Add background tasks that a host function starts, Wasmtime's `Accessor::spawn` and `Access::spawn` (`crates/wasmtime/src/runtime/component/concurrent.rs:244,520` at `v49.0.0-rc.1`), with `AccessorTask` and `JoinHandle`. Wasmtime uses them for host functions that return a `stream` or a `future` and must keep writing after the function returns.

PDD028 lists this as a difference between the Wasmtime and polyfill glue modules. Settle whether the polyfill's host tasks (PDD018) already express it and whether a short design is needed; follow Wasmtime where cheap.

## Acceptance criteria
- [ ] `Accessor::spawn` and `Access::spawn` exist with Wasmtime's shapes, and `JoinHandle` cancels the task.
- [ ] A host function returns a `stream` and a spawned task writes to it after the function returns, on both targets.
- [ ] `lint` passes and `tests all` is green.

