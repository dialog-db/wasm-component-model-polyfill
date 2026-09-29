---
id: 5d0ebe
title: "The browser backend: host functions through generated wrappers"
type: feature
blocked_by: [f9f268]
labels: [PDD025, runtime-layer]
created: 2026-09-28T21:45:55Z
---

## What to build

Step 1 of the migration (build beside). Host functions in the browser backend, through a generated wrapper module for each host function.

A JavaScript function that throws into a guest throws an exception, and a guest's `catch_all` catches it; the fused adapters of the Component Model catch every exception. A host error must be a trap, which no guest catches. So the JavaScript function never throws. It returns a status flag, and the backend keeps the host's error. A generated wrapper module sits between the guest and the JavaScript function: it calls the function, and on the error flag it runs `unreachable`. The trap reaches the host as `Host(error)`, the host's own error. The wrapper is WebAssembly, so it does not break JSPI.

- **Rules.** The closure is `Fn` and re-entrant at any depth. Each call has its own arguments and results, and no call shares a buffer with another call. The closure receives a context that reaches the store.
- **Arity.** A host function of any arity works, including more than eight parameters, without making a function from source.

## Acceptance criteria
- [ ] A guest calls a failing host function inside a `try_table` with `catch_all`. The call fails with `Host` and the host's own error, and the guest's handler does not run.
- [ ] A host function calls into the guest, and the guest calls the same host function again. Each depth returns its own results.
- [ ] A host function of more than eight parameters is called and returns its results.
- [ ] The shared contract tests for host functions pass in the web lane.
- [ ] `lint` and `tests web debug` pass.


## Dispatch log

- 2026-09-29: the seed is the first tip that carries both the browser backend (`ba6877408`) and the Wasmtime memory and trap cases (`2447fc945`), which were built in parallel. If the gate fails on the seed before your change, say so in your report and name the failure.
- 2026-09-29: implementor `card-5d0ebe-61aa517e` dispatched from `e665f640` (browser backend landed as `ba6877408`), in parallel with 215523 in the same crate.
- 2026-09-29: implementor reported done at `006d510b9` (a generated wrapper module per function type, one instance per host function; store pointer set only during a guest call, restored by a guard; new shared contract test for more than eight parameters); `tests all` and `lint` green, web debug 1472 passed. The seed gate was green before the change. `Cargo.lock` was edited by hand (menu gap: nothing updates the lock). Implementor paused. Reviewer `review-5d0ebe-72550551` launched; branch delivered.
- 2026-09-29: reviewer `review-5d0ebe-72550551` **accepted** at `006d510b9` (`tests all` and `lint` green, web 1472/1472 per profile; re-entry, traps under host frames and start functions judged sound; the hand-edited `Cargo.lock` is byte-identical to what `cargo` regenerates). Strongest finding, not blocking in the reviewer's view: undefined behaviour is reachable from safe code if a pending instantiate future is `mem::forget`-ed while the store pointer guard is held across its `await` (filed as bug edf2c7, top of To-do). Performance: each host call makes 3+N+M wasm-bindgen crossings (`wrapper.rs:554-660`), which matters for the benchmark gate at the switch (noted on 7eb780). PDD vs code: `lib.rs` widens what JavaScript carries to include wrapper-to-host calls through wasm-bindgen glue, where the PDD lists only what the JS API alone does; the reviewer thinks the code is reasonable and the owner should reconcile. Landed cleanly as `4c4c05641` (`Cargo.lock` auto-merged; only board files conflicted). Follow-ups: edf2c7, afe0cf. Revert artifact: `sandbox-guest/card-5d0ebe-61aa517e`.
