---
id: 1babc0
title: Tidy the browser backend after its first review
type: chore
blocked_by: [f9f268]
labels: [runtime-layer]
created: 2026-09-29T07:32:54Z
---

## What to build

Non-blocking findings from the independent review of f9f268 in `rust/wcmp-wasm-core-web`:

- **i31 range.** `store.rs:538` accepts a Number below 2^31 as an i31. The JS API makes an i31 only from [-2^30, 2^30) (WebAssembly/spec `document/js-api/index.bs:1456`). A Number in [2^30, 2^31) is an internalized host value, but `any_ref_as_i31` reports it as an i31.
- **Non-nullable exnref.** A null passed to a non-nullable `exnref` parameter gets past `values::check`, which compares only kinds (`values.rs:84-90`). The carrier then traps at `ref_as_non_null` (`carrier.rs:411`), and the host gets `Trap(Other)` instead of `TypeMismatch`. Check nullability before the call.
- **Unknown concrete index.** `boundary.rs:549` declares `NoFunc` for a concrete type index it does not know; `Error::Compile` is right. The case is unreachable today.
- **Carrier limits.** Only exports with a known type get a carrier, and a function with a concrete reference type next to a `v128` or `exnref` is refused (`carrier.rs:335-352`). Lift the limit or say so in the crate docs.
- **Conventions.** `objects.rs` has 8 public types (lines 170-245) and `type_registry.rs` has 2, against the one-public-type-per-module rule.
- **Missing tests.** A browser test that equal types in two modules give equal handles (and a global links), while a different rec group gives a different handle and a link error; a test where one probe fails (for example `gc`) and the backend still declares the others; an i31 edge-of-range test.

## Acceptance criteria
- [ ] Each finding is fixed, or the crate docs say why not.
- [ ] The three missing tests exist and run in the web lanes.
- [ ] `tests all` and `lint` pass.

## Review notes

