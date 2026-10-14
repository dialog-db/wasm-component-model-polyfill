---
id: 8dcde9
title: Test every container branch of the error-context store check
type: chore
blocked_by: [1bbe43]
labels: [concurrency, backlog-burndown-002]
created: 2026-10-13T18:44:16Z
disposition: accepted
disposition_at: 2026-10-16T06:33:12Z
---

## What to build
From the 2026-10-13 review of 1bbe43. The wrong-store check for error contexts is complete, but most of its container branches have no test.

The tests use only a bare `error-context` and a `list<error-context>`. Checked on 2026-10-13: with these two breaks applied together, plus one unrelated break, `tests native debug` passed all 1733 tests:
- `ComponentValue::check_store` for `Option<T>` (`rust/wcmp/src/linker/component_value.rs`) made to return `Ok(())`.
- The `Val::Option(Some(inner))` arm removed from the walker in `rust/wcmp/src/abi/call_values.rs`.

The review expects the same for:
- the `HashMap` and `[T; N]` overrides in `component_value.rs`;
- the walker's `Map`, `Record`, `Variant` and `Result` arms, or narrowing its `List | FixedLengthList | Tuple` arm;
- the `Variant`, `Map`, `FixedLengthList`, `Result` and `Tuple` arms of `holds_error_context` in `rust/wcmp/src/abi/signature.rs`. Its unit test covers only `List(Option)`, `Record`, `Stream` and `list<u8>`.

A missed branch does not fail cleanly. The value lowers inside the task and poisons the store, which is the bug 1bbe43 closed.

The tests need no component. Call `abi::call_values::check_store` on each `Val` shape with an error context from a fresh `StoreId`. Call `ComponentValue::check_store` on `Option`, `HashMap` and `[T; N]` holding `ErrorContext::new(StoreId::fresh(), id)`. Extend the `holds_error_context` unit test to every arm.

Smaller gap: the test of a host function returning another store's error context uses `func_new` only. Add the `func_wrap` result path.

## Acceptance criteria
- [x] Every container branch of the untyped walker, of the typed `check_store` overrides and of `holds_error_context` has a test that fails when that branch is removed. Show this by mutation for at least the two branches above.
- [x] A typed host function (`func_wrap`) that returns another store's error context fails the guest's call with the wrong-store cause.
- [x] `lint` passes and `tests all` is green on both targets.

## Implementation notes

- 2026-10-13 (`test(concurrency): Cover each container branch of the store
  check`):
  - `abi::call_values::tests::it_finds_another_stores_error_context_in_every_container`
    covers bare, list, fixed-length list, tuple, map key, map value,
    record, variant, option, result ok and result err, both refused and
    passing.
  - `linker::component_value::tests::it_checks_the_store_of_an_error_context_in_every_typed_container`
    covers bare, `Option`, `Vec`, `[T; N]`, `HashMap` and a tuple.
  - `holds_error_context`'s test adds variant, map key and value,
    fixed-length list, option, result ok and err, tuple, and a variant with
    none.
  - `func_wrap` path:
    `it_fails_the_guests_call_when_a_typed_host_function_returns_another_stores_error_context`
    shares its assertions with the `func_new` test.
  Mutation, run together on 2026-10-13: `Option<T>::check_store` returning
  `Ok(())` failed the typed-container test, and dropping the walker's
  `Val::Option(Some(inner))` arm failed the walker test.

- 2026-10-13 gate on the series tip: `tests all` green (native debug and release 1741/1741, no-provider 169, Wasmi 1458, web debug and release 1728/1728, fidelity 264/267/264, Zena Browser 18/18, Native 18/18, Wasmi 2/18, Wasmtime 18/18, demo 68/68), no `unexpected:` or `stale expectation:` line, `tests regenerate --dry-run` current, and the full `lint` green.

## Review notes

- 2026-10-13, review round 1 (context-free reviewer): nit. The commit's
  subject was 80 characters, over the rule's ~70. It is now `test(concurrency):
  Cover each container branch of the store check`.
