---
id: afe0cf
title: Widen and test the browser host-function wrapper
type: chore
blocked_by: [5d0ebe]
labels: [runtime-layer]
created: 2026-09-29T10:01:19Z
---

## What to build

Non-blocking findings from the independent review of 5d0ebe in `rust/wcmp-wasm-core-web`:

- **Concrete reference types.** `wrapper.rs:338-343` refuses concrete ref types with `Error::Backend` because the wrapper cannot name the type. Under iso-recursive canonicalization the wrapper could redeclare the rec group from the `TypeRegistry`. The PDD's goal is the full type model at the boundary.
- **exnref growth.** `exnref` parameters grow the exceptions table on every crossing, and a failed `table.grow` (-1) silently becomes null (`wrapper.rs:581-593`).
- **Missing frame.** `Calls::result` falls back to `undefined` for a missing frame; for an `i64` or reference result that throws a catchable `TypeError` instead of trapping. Unreachable today; make the fallback trap.
- **Missing tests.** `anyref`, `i31ref` and `structref` parameters and results through the wrapper, including the `ref.cast` path; a null result for a non-null reference type (expect `Host(TypeMismatch)`); the refusal of concrete and continuation types; a guest's own trap under a host frame followed by a successful call.

## Acceptance criteria
- [ ] Concrete reference types cross the wrapper, or the crate docs say why not.
- [ ] A failed `table.grow` is a structured error, and the exception table does not grow without bound.
- [ ] A missing frame traps.
- [ ] The missing tests exist and run in the web lanes.
- [ ] `tests all` and `lint` pass.

## Review notes

