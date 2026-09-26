---
id: 7c4deb
title: The README states the cancellation, error-context, and poisoned-store support
type: docs
blocked_by: [072663, fed20f]
labels: [PDD023, concurrency]
created: 2026-09-26T23:12:28Z
---

## What to build
Update the project `README.md` once every other PDD023 card has landed, so that it states what the polyfill now supports. The last such update was the stack-switching commit `52b72db1f`, which is the pattern to follow. Every statement must come from the landed code and a live run, not from the design.

- **Feature support.** Mark each row with its true status and a short note:
  - `task.cancel` and `subtask.cancel`, which are 🟡 today and fail at the call with `Error::Unsupported`. Say that a guest callee and a host callee can be cancelled, and that a host callee is cancelled by dropping its future in a turn.
  - "Cancellation", marked ❌ today. Name the `cancellable` immediate, which the polyfill honors as Wasmtime 49 does although the reference removed it.
  - "Trap poisoning of an instance", marked ❌ today. Say that a trap poisons the whole store, list the entries a poisoned store refuses with "cannot enter component instance", and name the two departures from Wasmtime: instantiation is refused too, and queued guest work and host futures are discarded at the moment of poison.
  - The `error-context` type and the three `error-context` built-ins, marked ❌ today. Say that they stay behind `wasm_component_model_error_context`, off by default, and that the host sees an error context through `Val::ErrorContext` and `ErrorContext` with no operation, as in Wasmtime.
- **Host API.** State where a trap surfaces: the first trap ends the driver that is polling, and the next entry fails with the cannot-enter trap. State that the store caps its live records at 1,000,000.
- **Conformance.** Refresh the summary and its numbers from the closing corpus run of card `072663`. Remove cancellation, `error-context`, and trap poisoning from the list of what the `async` rows still fail on (`README.md:386-390` today). Keep any line that still fails for another reason.
- **Toward Component Model 1.0, and Design documents.** Update any statement that these features are only designed or not built.
- **Smoke test.** If the README describes the smoke test's chapters or story count, include the stories of the smoke card.

## Acceptance criteria
- [ ] Every Feature support row that PDD023 touches has its true status and a note, checked against the landed code.
- [ ] The README states the poisoned-store rule, its two departures from Wasmtime, where a trap surfaces, the error-context gate and host types, and the record cap.
- [ ] The conformance summary matches the closing corpus run in all four states.
- [ ] The document is formatted with `markdown format`, and the `markdown` check and `lint` pass.

