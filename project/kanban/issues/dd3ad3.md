---
id: dd3ad3
title: The three error-context built-ins run
type: feature
blocked_by: []
labels: [PDD023, concurrency]
created: 2026-09-26T23:08:55Z
---

## What to build
Run the three `error-context` built-ins inside one component. The translator accepts them on every target when `EngineConfig::wasm_component_model_error_context` is on (`src/engine_config.rs:79`), which stays off by default, as in Wasmtime. Today it refuses them as unsupported trampolines; the baseline at `src/baseline/prepared_call.rs:864-874` pins that refusal and changes with this card.

An error context is one store-wide record holding the debug message and a count of the guest handles that name it. A handle in an instance's table has the error-context kind PDD018 reserved (`HandleKind::ErrorContext`, `src/resource/handle_kind.rs:89`). The record leaves the store when its count reaches zero.

Each built-in first fails with the cannot-leave trap when the instance's may-leave flag is clear.

- `error-context.new` takes the canon options `memory` and `string-encoding`. It reads the debug message from guest memory in that encoding, keeps it exactly as written (Wasmtime keeps it; the reference allows an empty string), creates the record with a count of one, and returns the new handle. A message out of bounds fails with PDD008's string bounds checks.
- `error-context.debug-message` takes `memory`, `realloc`, and `string-encoding`. It first checks that the eight bytes at the guest's address are in bounds, and only then calls `realloc`, which is Wasmtime's order. An address out of bounds fails with "invalid debug message pointer: out of bounds". It writes the message through `realloc` and stores the pointer and length at the address.
- `error-context.drop` removes the handle and subtracts one from the count.
- In `debug-message` and `drop`, a handle of another kind fails with "handle is not an error-context".

Each new message is a structured cause of `wcmp::Error`, matched by substring in the corpus. Follow Wasmtime's built-ins (`futures_and_streams.rs:4245-4330` at `v49.0.0-rc.1`).

The `error-context` value type, transfer between components, the overflow count, and the host surface are the next cards of this design.

## Acceptance criteria
- [ ] `wasmtime/async/error-context.wast` (5, 16, 30, 39, 84, 85, 86) and `wasmtime/error-context-trap-in-post-return.wast` (3, and 45 through 50) pass whole on both targets.
- [ ] A repository test on both targets creates an error context, reads its debug message back byte for byte in each string encoding, and drops it; the record leaves the store at the drop.
- [ ] A repository test proves that an out-of-bounds address fails with "invalid debug message pointer: out of bounds" before `realloc` runs, and that a handle of another kind fails with "handle is not an error-context".
- [ ] With the gate off, the translator still refuses the built-ins.
- [ ] `lint` passes and `tests all` is green on both targets, any directive that starts passing has its line removed from the expected-failure list, and every line whose reason this card changes is refreshed from a live run.

