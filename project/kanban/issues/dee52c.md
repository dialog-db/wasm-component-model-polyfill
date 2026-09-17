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

- 2026-09-17 reviewer `review-dee52c-c870ef91` on tip `5e36ab9`: **accept**. Every rewritten comment verified true (`host_call.rs:20-33`, `accessor.rs:28-35,79-85` backed by `accessor.rs:237`, `suspend_seam.rs:1129-1132`, `suspend_provider.rs:45-66`); diff comments-only. Gate: native 463, web 354 both profiles; conformance unchanged; `lint` 9/9. **For the owner (design):** the runtime-layer exposure is real and wider than this card — `lib.rs:209` re-exports `StoreContext` (35 `pub` methods, 32 doc-marked "workspace-internal") and `Store::inner`/`inner_mut` (`store.rs:224,231`) expose `wasm_runtime_layer::Store`; both arrived with the `8af18fc` reshape. Options: narrow the surface (split the runtime reach off the re-exported type — a second type, or an exception to the no-`pub(crate)` rule) or amend PDD011 line 63 / PDD005 line 25 (a foundational promise other PDDs lean on). Nits for a later card: `func.rs:105` and `linker.rs:153` promise a refusal whose precondition cannot be reached while the caller holds `&mut Store`; `module/mod.rs:8` is false given `CoreExternType::from_runtime`/`CoreValueType::from_runtime`; `Store::context`/`inner` docs say "not re-exported by lib.rs" while public.


Follow-up from the re-review of af244b (`review-af244b-b81315fc`).

## Dispatch log
- 2026-09-17T22:08:54Z dispatched implementor `card-dee52c-534585b0` (implement session, PDD018 thread, Opus 5; seeded after 8af18fc landed)
- 2026-09-17T22:52:46Z implementor reported done at `5e36ab9` (comments only; gate green; flagged the PDD011/PDD005 runtime-layer-exposure contradiction for the owner). Fetched, needs-review, paused the implementor; launched reviewer `review-dee52c-c870ef91`, delivered the branch.
- 2026-09-17T23:33:33Z reviewer accepted; landed as `d6c22d4`; card to ready; removed the pair. Revert artifact: `sandbox-guest/card-dee52c-534585b0`.
