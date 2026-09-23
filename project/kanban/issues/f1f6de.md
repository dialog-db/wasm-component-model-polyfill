---
id: f1f6de
title: The type projection accepts stream<T> and future<T>
type: feature
blocked_by: []
labels: [PDD021, concurrency, abi]
created: 2026-09-23T05:58:18Z
---

## What to build
Add `Stream` and `Future` to `ValueType` in `src/types/value_type.rs`, each carrying an optional payload `ValueType`, and project `InterfaceType::Stream` and `InterfaceType::Future` to them in `src/component/project.rs`, where the two arms at lines 199-200 refuse them today. The flat representation of either type is one `i32`, the index of a readable end in the handle table of the instance that holds it, so `src/abi/layout.rs` gives both the four-byte scalar shape a handle has and `src/abi/flatten.rs` flattens each to one slot. Validation already refuses a payload that contains a `borrow` and refuses `stream<char>`; the polyfill adds no check of its own. Both types are ordinary in a function type: a synchronous export, a synchronous host function, an `async` export, and an `async` import can carry one, on a parameter and on a result, and the projection accepts them on an import and on an export. Wasmtime exposes the same two as `types::StreamType` and `types::FutureType`. The lift and lower of a value of either type (`src/abi/lift.rs`, `src/abi/lower.rs`) are not part of this card: until the readable end can cross a boundary, a lift or lower that reaches one of the two types fails with `Error::Unsupported` at the call, not at translation. Every corpus line whose reason was the `stream<T>` values or `future<T>` values refusal changes reason to the next refusal on its path (`fixtures/wasi-http.wast:4` among them); refresh each such line from a live run on both targets.

## Acceptance criteria
- [ ] A host reads `ValueType::Stream` and `ValueType::Future` with the payload type on an import and on an export, proved by a repository test on a hand-written component.
- [ ] `validate-no-stream-char.wast` still passes and proves that validation refuses `stream<char>`.
- [ ] A component whose function type carries a stream or a future translates, and a call that lifts or lowers such a value fails with `Error::Unsupported` until the crossing lands, proved by a test.
- [ ] `lint` passes and `tests all` is green on both targets, any async directive that starts passing has its line removed from the expected-failure list, and every line whose reason this card changes is refreshed from a live run.


## Dispatch log
- 2026-09-23T06:08:08Z dispatched implementor `card-f1f6de-1bd0e4bc` (implement session, PDD021 thread, budget 3)
- 2026-09-23T07:19:07Z implementor reported done at `ba3176d` (three commits: feature, rustfmt, smoke fix; public `StreamType`/`FutureType` with optional payload, `ValueType::Stream`/`Future`, projection through the table's type index, four-byte scalar and one `i32` slot; lift, lower and both flatten paths fail with `Error::Unsupported` at the call; new `tests/baseline_stream_future_types.rs`; `tests regenerate` refreshed 12 corpus lines, 10 to the stream-new/future-new trampoline and `fixtures/wasi-http.wast:4` to the task-cancel trampoline, web reasons checked equal by a temporary local probe; `baseline_linking`'s unsupported-feature test moved from stream to error-context; the smoke WASI-HTTP step now expects the task-cancel refusal; `tests/corpus/README.md` updated; `tests all` four lanes green (native 895, web 886), `tests smoke native`/`check` 14/14, `lint` green). Menu gaps: no Rust formatter command; the web lane has no fail-fast passthrough. Fetched, moved to needs-review, paused the implementor.
- 2026-09-23T07:20:33Z launched reviewer `review-f1f6de-c0b3a614`; delivered `sandbox-guest/card-f1f6de-1bd0e4bc` (tip ba3176d) as `delivered/card-f1f6de-1bd0e4bc`.
- Orchestrator note: this branch and f65b25's overlap. This branch pins `fixtures/wasi-http.wast:4` and the smoke WASI-HTTP step (`rust/wcmp-smoke/src/lib.rs`) to the task-cancel refusal, and f65b25 removes that refusal. Both also rewrite `tests/corpus/expected-failures.txt`. Whichever lands second needs a reconciliation and a gate on the composed tip.
