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


## Dispatch log
- 2026-09-27T15:49:19Z dispatched implementor `card-7c4deb-3d2315fb` from code tip `a0983522f` (all twelve other PDD023 cards landed; implement session, PDD023 thread, budget 3; docs-only gate per the dispatch instructions unless the diff can affect a test).
- 2026-09-27T16:17:54Z implementor reported **blocked** with the README work complete at `d0490b5f4` (3 docs commits; only `README.md` changed): its `lint` was red on a guest Nix store fault, not the diff — a host-built `tests-native-debug` path invalid in the guest DB ("opening lock file … No such file or directory") and a native deps derivation failing at build setup ("getting status of fd 18"). Cause on the host side: the orchestrator's final tip gate built the same native derivations in the shared store while the guest was building them. Every check that does not need the native deps passed in the guest (markdown, project-lint, rustfmt, doctests, public-api, bench-web, sandbox, sandbox-verb-coverage); the three web lanes passed live (1313/1313, 169/169). Content: task.cancel/subtask.cancel ✅ (guest and host callees), Cancellation ✅ naming the `cancellable` immediate, Trap poisoning ✅ with a new "Traps and the poisoned store" section (refused entries, the two departures, where a trap surfaces, the 1,000,000 record cap), `error-context` 🔒 behind `wasm_component_model_error_context` with `Val::ErrorContext`/`ErrorContext`, two new host-API rows; conformance table: native states from the 072663 run adjusted for fed20f's `stats.wast` (5) and `error-reporter.wast` (1) → 2440 directives, 2376 (97.4) native, 2365 (96.9) JSPI, 2320 (95.1) native no-provider, 2309 (94.6) browser no-provider; no-provider overlay paragraph rewritten from the live list (56 lines). Notes for the owner: `tests/corpus/README.md` still gives fixtures as 59 directives and ten files (predates fed20f); the `TableFull` doc in `error.rs` omits error contexts from the counted records though `record_count` counts them. Menu gap: no command prints the no-provider conformance table. Fetched; paused the implementor.
- 2026-09-27T16:19:21Z running `lint` (`nix flake check` on `git+file://…?rev=d0490b5f4`) on the host in place of the guest's; launched a fresh reviewer `review-7c4deb-9078d1e1` (docs-only: reads and `markdown`); delivered `sandbox-guest/card-7c4deb-3d2315fb` as `delivered/card-7c4deb-3d2315fb`.
- 2026-09-27T16:21:38Z host `lint` (`nix flake check` on `git+file://…?rev=d0490b5f4`): all checks passed. The guest store fault did not reflect the diff; gate items 1, 3 and 4 hold (docs-only, so item 2 is skipped). Moved to needs-review.
