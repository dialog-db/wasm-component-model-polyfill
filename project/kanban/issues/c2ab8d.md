---
id: c2ab8d
title: The browser backend refuses a re-entrant host function with a structured error
type: bug
blocked_by: [639a90]
labels: [PDD020, concurrency]
created: 2026-09-20T17:38:45Z
---

## What to build
The vendored `rust/vendor/js_wasm_runtime_layer` wraps each host function in one wasm-bindgen `Closure<dyn FnMut>` (`src/func.rs` around line 71) whose results buffer (around line 149) is reused across calls, and wasm-bindgen panics with `closure invoked recursively or after being dropped` when a `Closure` is re-entered. A nested turn opened from inside a lowered host import (a blocking built-in, `thread.yield`, or now a synchronous lower of a host async function) runs on the same stack inside that `Closure`, so on the web target any item the nested turn runs that calls the same lowered import re-enters it and aborts with a panic rather than a trap. Three reviews raised this (cards 708ab2, 05de62, 639a90) and no guard exists; the design corpus does not state the divergence. Give the backend a re-entry guard: detect the re-entrant call in the wrapper before wasm-bindgen does, and fail it with a structured error the polyfill maps to a `SchedulerCause` (name it) rather than a panic, so the guest sees a trap with a message naming the limitation. Record the patch in `PATCHES.md` and state the native/web divergence in the polyfill's docs on the seam.

## Acceptance criteria
- [ ] A web test in which a nested turn runs an item that calls the lowered import that opened the turn fails with the structured cause and its message, not a panic; the same component passes natively.
- [ ] `PATCHES.md` records the guard and the seam's doc states the divergence.
- [ ] `lint` passes and `tests all` is green on both targets with the conformance summary unchanged.

## Dispatch log
- 2026-09-21T10:07:21Z dispatched implementor `card-c2ab8d-fbd412ef` (implement session, PDD020 thread, budget 3, seed `1e0d2b4` with every original card at ready and the closer 9cf784 in flight)
- 2026-09-21T12:54:17Z implementor reported done at `25ab242` (one commit on seed `ff363b1`; the vendored wrapper's body moves behind a `RefCell` and the JS shim becomes a `Closure<dyn Fn>` that borrows for one call; a re-entrant call is refused with a new public `ReentrantHostCall` error recorded in patch 4's first-host-error slot and thrown as a JS Error; new `SchedulerCause::ReentrantHostCall`; new workspace-private `backend::substrate_failure` downcasts at the seven `Func::call` sites; web test on `SYNC_YIELDS` asserts the cause and message, native asserts the same component completes; PATCHES.md entry 10; divergence stated in `backend.rs` and `SuspendSeam`'s doc; `tests all` four lanes, `lint` green, conformance unchanged). For the owner: the card's premise is overstated — wasm-bindgen 0.2.108 throws a catchable JS exception on re-entry, not a panic; the guard is per host function so it also covers a re-entered host `async` import under a synchronous lower, proved for the `thread.yield` shape only; the design corpus still does not state the divergence. No overlap with other landings. Fetched, moved to needs-review, paused the implementor.
- 2026-09-21T12:54:17Z launched reviewer `review-c2ab8d-0650fb2d`; delivered `sandbox-guest/card-c2ab8d-fbd412ef` (tip 25ab242) as `delivered/card-c2ab8d-fbd412ef`.

## Review notes
