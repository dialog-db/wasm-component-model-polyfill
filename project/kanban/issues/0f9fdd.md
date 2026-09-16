---
id: 0f9fdd
title: Vendor the async corpora and register host-echo-u32
type: chore
blocked_by: []
labels: [PDD018, concurrency, conformance]
created: 2026-09-16T05:54:16Z
---

## What to build
Vendor the `async` directory of the Component Model test corpus (`test/async/` at `e5ee0af9c617`) into `tests/corpus/cm/async/` and the `async` directory of the Wasmtime component tests (`tests/misc_testsuite/component-model/async/` at `cb091c33cece`) into `tests/corpus/wasmtime/async/`. Those are the commits `tests/corpus/README.md` already records. Regenerate the harness manifest with `fixtures` so each file gets a `corpus_test!`. Run the corpus and list every failing directive in `expected-failures.txt` as `deferred-feature`, one line per directive; a directive that already passes gets no line and counts as a pass. Register `host-echo-u32` in `link_spectest` in `tests/conformance.rs` as a synchronous host function: a synchronous host function called through an asynchronous lower returns `RETURNED` at once, which is what the spectest wants. Do not register `never-return`, `return-two-slowly`, `echo-slowly`, or `[method]resource1.never-return`; every directive whose component imports one of those gets a `deferred-feature` line whose reason names the missing host task registration. Make the progress summary show `cm/async` and `wasmtime/async` as rows of their own, separate from the synchronous rows of the same suite (`Summary::new` in `tests/conformance/report.rs` keys rows by the first path component today), and add the rows to the README's source table and baseline table. Add a line to `expected-failures.web.txt` only when the substrate differs, so both targets count the same. This vendoring lands before any card that can change what an async directive does, so later cards measure against a baseline instead of creating one.

## Acceptance criteria
- [ ] `tests/corpus/cm/async/` and `tests/corpus/wasmtime/async/` hold the upstream `async` directories at the commits the README records, and the README's source table says so.
- [ ] The progress summary shows one row for each `async` directory with its directive count and its deferred-feature count.
- [ ] `expected-failures.txt` has one `deferred-feature` line per failing async directive, and no line for a directive that passes.
- [ ] `host-echo-u32` is registered in `link_spectest`, and every directive that imports `never-return`, `return-two-slowly`, `echo-slowly`, or `[method]resource1.never-return` is listed as a deferred feature whose reason names the missing host task registration.
- [ ] The native run and the browser run report the same counts for the `async` rows, and the same pass and failure results for every async file.
- [ ] The harness still fails on an unlisted failure or a stale expectation, so a listed directive that starts to pass fails the run until its line is removed.


## Dispatch log
- 2026-09-16T06:28:44Z dispatched implementor `card-0f9fdd-23b7af81` (implement session, PDD018 thread, budget 4)
