---
id: fc6b5d
title: Typed value forms for Result, tuples, boxed and shared strings and lists, and the any streams
type: feature
blocked_by: [3e9e67]
labels: [PDD027, typed-values]
created: 2026-10-16T07:41:59Z
---

## What to build

PDD027, "Value Types". Give the typed value traits the Rust forms that the PDD's table lists and the polyfill does not cover yet, following Wasmtime's impls in `crates/wasmtime/src/runtime/component/func/typed.rs` at `v49.0.0-rc.1`:

- `Result<T, E>` for `result<T, E>`. The unit type `()` matches a case with no payload, as Wasmtime's `IS_RUST_UNIT_TYPE` does (`typed.rs:2518-2526`).
- Tuples of 0 to 17 values as values: a tuple inside a record, a list, or a result crosses as a WIT `tuple`.
- `Box<str>` and `Arc<str>` for `string`, and `str` as lower only.
- `Box<[T]>` and `Arc<[T]>` for `list<T>`, and `[T]` as lower only.
- `StreamAny` and `FutureAny` for `stream<T>` and `future<T>`.
- `&T`, `Box<T>`, and `Arc<T>` forwarding to `T`, as lower only.

Not in scope: `bytes::Bytes` and `Rc`, which the PDD excludes.

## Acceptance criteria
- [x] Every value type of the table crosses on both targets. A test crosses each Rust form in the PDD's value types table in each direction that the table allows.
- [x] A fixed-length list with the wrong length fails the type check.
- [x] A `Result` with `()` matches a case with no payload, and a `Result` whose payload type differs fails the type check with a path that names the `ok` or `error` payload.
- [x] `lint` passes, and `tests all` is green on both targets with the conformance summary unchanged.

## Notes

- 2026-10-16: `Result<T, E>`, `str`, `[T]`, `&T`, `Box<T>`, `Arc<T>`, the boxed and shared strings and lists, and the `StreamAny`/`FutureAny` readers live in `rust/wcmp/src/typed/impls/{results,forms,handles}.rs`. `tests/baseline_typed_forms.rs` crosses each one (the scalars, options, tuples, maps, and readers come from 3e9e67's tests). A unit `Result` payload lifts against a case with no payload, and a mismatch names `ResultOk` or `ResultErr`.
- 2026-10-16: the public API listing for these impls was recorded late and folded into this commit.
- 2026-10-16: `tests all` is green on the composed tip (zulsyvvw): native 1768, web 1755, wasmi 1475, fidelity 264/267/264, demo 69/69, Zena Browser 18/18. The conformance progress record is unchanged.
