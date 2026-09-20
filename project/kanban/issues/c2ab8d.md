---
id: c2ab8d
title: The browser backend refuses a re-entrant host function with a structured error
type: bug
blocked_by: []
labels: [PDD020, concurrency]
created: 2026-09-20T17:38:45Z
---

## What to build
The vendored `rust/vendor/js_wasm_runtime_layer` wraps each host function in one wasm-bindgen `Closure<dyn FnMut>` (`src/func.rs` around line 71) whose results buffer (around line 149) is reused across calls, and wasm-bindgen panics with `closure invoked recursively or after being dropped` when a `Closure` is re-entered. A nested turn opened from inside a lowered host import (a blocking built-in, `thread.yield`, or now a synchronous lower of a host async function) runs on the same stack inside that `Closure`, so on the web target any item the nested turn runs that calls the same lowered import re-enters it and aborts with a panic rather than a trap. Three reviews raised this (cards 708ab2, 05de62, 639a90) and no guard exists; the design corpus does not state the divergence. Give the backend a re-entry guard: detect the re-entrant call in the wrapper before wasm-bindgen does, and fail it with a structured error the polyfill maps to a `SchedulerCause` (name it) rather than a panic, so the guest sees a trap with a message naming the limitation. Record the patch in `PATCHES.md` and state the native/web divergence in the polyfill's docs on the seam.

## Acceptance criteria
- [ ] A web test in which a nested turn runs an item that calls the lowered import that opened the turn fails with the structured cause and its message, not a panic; the same component passes natively.
- [ ] `PATCHES.md` records the guard and the seam's doc states the divergence.
- [ ] `lint` passes and `tests all` is green on both targets with the conformance summary unchanged.


