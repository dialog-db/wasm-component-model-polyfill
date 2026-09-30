---
id: 10acdb
title: "Tidy the Wasmi lane: gc-less browsers, backend fallback, docs"
type: chore
blocked_by: [67f37c]
labels: [runtime-layer]
created: 2026-09-30T20:41:32Z
---

## What to build

Non-blocking findings from the independent review of 67f37c (Wasmi on every lane):

- **A browser without `gc`.** The host reader of the thread-start table needs function types, but `Func::ty` returns `None` in the browser backend (`wcmp-wasm-core` `externs/func.rs:79-85`). On a browser without `gc`, `thread.new-indirect` then fails with an internal error at call time instead of `Unsupported(gc)` at `Component::new`. Gate on the backend telling function types, or refuse at translation.
- **Backend fallback.** `rust/wcmp/tests/support/backend.rs:125` `backend_declaring` falls back to Wasmtime, which does not declare `host_suspension`; its doc's claim is wrong for that capability.
- **Docs.** `rust/wcmp/tests/corpus/README.md:189` says `memory64.wast` takes 3.6 GiB on Wasmi; the reviewer measured 4.0 GiB (65538 pages). `rust/wcmp/tests/zena/README.md` describes three subjects and needs the Wasmi subject (2/14, 12 stop at `parse` with `Unsupported gc`).
- **Missing test.** A unit test of the bench `backend()` parse and the report's `backend` field.

## Acceptance criteria
- [ ] A browser without `gc` refuses `thread.new-indirect` with `Unsupported(gc)` at `Component::new`, or it works.
- [ ] `backend_declaring` is right for every capability.
- [ ] The two READMEs are accurate, and the bench test exists.
- [ ] `tests all` and `lint` pass.

## Review notes

