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


## Dispatch log
- 2026-09-26T23:16:39Z dispatched implementor `card-dd3ad3-6c49b2cc` from tip `ecbd808` (implement session, PDD023 thread, budget 3, full gates on implementor and reviewer)
- 2026-09-27T00:10:07Z implementor reported done at `8a490f62d` (one commit on seed `7e89184e6`; store-wide `ErrorContextRecord` in `TaskTables` with `ErrorContextId`, `HandleKind::ErrorContext` carries it; `executor/error_context_builtins.rs`; three `TrampolineSpec` variants; new public `Error::ErrorContext(ErrorContextCause)` with `DebugMessagePointerOutOfBounds` and `NotAnErrorContext` (message adds the handle index); `public-api.txt` re-recorded; the 14 owned lines of `error-context.wast` and `error-context-trap-in-post-return.wast` pass on both targets and leave the list; tests: byte-for-byte read-back in each encoding with record count 1→0, bounds-before-realloc with a trapping realloc, other-kind handle, gate off/on; behavior change: a closed error-context gate now maps to `Error::Unsupported` via `GATE_REFUSALS` instead of `InvalidComponentBinary`, and the old prepared_call refusal baseline is removed; native debug 1233, web debug 1228, `lint` 13 checks, `tests all` green with no unexpected/stale lines). Menu gap: no Rust formatter in the menu, so it ran `cargo fmt --all` directly. The drive exited on the nudge timeout; the report came from `work-state.json`. Fetched, moved to needs-review, paused the implementor.
- 2026-09-27T00:13:08Z launched reviewer `review-dd3ad3-cae275e9` (full gate); delivered `sandbox-guest/card-dd3ad3-6c49b2cc` (tip 8a490f62d, seed 7e89184e6 on ecbd808) as `delivered/card-dd3ad3-6c49b2cc`. Nothing has landed since the seed.

## Review notes
- 2026-09-27T00:56:00Z reviewer `review-dd3ad3-cae275e9` on tip `8a490f62d`: **accept**. `tests all` six lanes green (native debug 1233, web debug 1228), no unexpected/stale lines; `lint` green; `api list` differs from the seed only by `Error::ErrorContext` and `ErrorContextCause` (2 variants, `#[non_exhaustive]`). Criteria met: `src/baseline/error_context.rs:323` (read-back in each encoding, count 1→0), `:450` (bounds before a trapping realloc), `:475` (other-kind handle), `:528` (gate off/on). Record life traced (`error_context_record.rs:153`, `task_tables.rs:1172`, `:1190-1206`); `debug-message` order may-leave → lookup → 8-byte bounds → realloc → write matches Wasmtime v49.0.0-rc.1 `futures_and_streams.rs:4297-4326`; no lock held across realloc; encodings correct (one exact-size realloc, which the ABI allows). `GATE_REFUSALS` (`translate.rs:1360`) consistent with the threading/stackful gates; nothing else observed `InvalidComponentBinary`. No wasm32-split code touched beyond an unused import in `prepared_call.rs`; web lanes ran all 7 new tests. Non-blocking: (a) the round-trip test cannot observe realloc's alignment/size for utf16 and latin1+utf16 (the first bump allocation is aligned anyway); (b) no empty-string utf16 case; (c) the `NotAnErrorContext` doc says "as the waitable causes do", but those read "handle index N is not a waitable" (`handle_lookup_error.rs:95`); (d) `error_context_builtins.rs:258-299` repeats the `trap`/`trap_if_cannot_leave`/`lock_tables` helpers four other built-in files copy. Design note for the owner: PDD023 does not say what happens when the debug-message address is not 4-aligned; the reference does not trap, Wasmtime has only a `debug_assert` (`func/typed.rs:1352` at v49), and the polyfill writes to the address.
- 2026-09-27T00:56:39Z landed as `c844a515b` (clean squash onto `e89c71c55`; nothing else had landed since the seed). Card to ready; pair removed; `sandbox-guest/card-dd3ad3-6c49b2cc` kept as the revert artifact.
