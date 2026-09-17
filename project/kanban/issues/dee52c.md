---
id: dee52c
title: Correct the host call and accessor docs after the store reshape
type: docs
blocked_by: []
labels: [PDD018, concurrency]
created: 2026-09-17T22:07:48Z
---

## What to build

Three doc comments the re-review of the suspend-seam reachability left false after the store reshape. `src/linker/host_call.rs` (lines 18-20) still says no runtime-layer type appears in `HostCall` and that the context is a borrowed view onto the polyfill's own state; at the tip `HostCall` holds a `StoreContext`, which wraps `wasm_runtime_layer::StoreContextMut`, and `call.store().runtime_mut()` reaches it in two hops — state what a host function can now reach, and whether that is intended. `src/concurrency/accessor.rs` (lines 29-30 and 77-79) still promises a recursive-driver refusal for a call into an export or an instantiation entered from inside the closure; with the entry points concrete, neither can be written from a closure that holds only a `StoreContext` — say that the refusal covers a nested `run_concurrent` and that the other two are compile-time impossibilities. `src/concurrency/suspend_seam.rs` (line 1132) still says the trampoline reaches the store "paired with the handle it captured"; nothing is captured now. Also move the wake obligation from the private `block_on_host_task` doc onto the public `SuspendProvider` trait doc where an implementor reads it. Comments only.

## Acceptance criteria

- [ ] No doc comment under `src/` claims `HostCall` holds no runtime-layer type, promises a refusal for a call that cannot be expressed, or says a trampoline captured a handle.
- [ ] `SuspendProvider`'s doc states both obligations (keep the store aware of the pending body; supply the wake).
- [ ] `lint` passes.

## Review notes


Follow-up from the re-review of af244b (`review-af244b-b81315fc`).
