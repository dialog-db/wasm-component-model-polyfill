---
id: acefba
title: bindgen world struct and exported calls, with Error::MissingExport
type: feature
blocked_by: [5290bd]
labels: [PDD028, bindgen]
created: 2026-10-17T02:55:52Z
---

## What to build

PDD028, "Exports" for plain functions and "The World". The world struct, the exported calls, and `Error::MissingExport` (Wasmtime `tests/expanded/smoke.rs:9-175` and `char_async.rs:360-384` at `v49.0.0-rc.1`).

- **World struct.** Named after the world in upper camel case, with `async fn instantiate_async(store, component, linker)`, `fn new(store, instance)`, `add_to_linker` (each imported interface's `add_to_linker` plus `add_to_linker_imports`), one `call_*` per world-level export, and one interface getter per exported interface (`fn example_app_run(&self) -> &exports::example::app::run::Guest`). No `instantiate`, `{World}Pre`, or `{World}Indices`. With `interfaces`, no world struct.
- **`new`.** Acquires each export as a `TypedFunc` and keeps it, so the type check runs once here and never in a `call_*`. A mismatch fails with `Error::TypeMismatch` and the PDD027 path. `new` takes the store for parity with Wasmtime, though the lookups do not use it.
- **`Error::MissingExport`.** Holds the export's name and the instance's name. `new` fails with it when the component lacks an interface or a function of the world.
- **Exported calls.** Each `call_*` is an `async fn` taking `&mut Store<T>`, whatever the flags. Parameters are borrowed (`&str`, `&[T]`) per Wasmtime's parameter mode (`lib.rs:3075-3079`). Each `call_*` has a `func_*` getter for its `TypedFunc`. The `exports` option parses with the same rule engine as `imports`; the `async` flag is accepted with no effect, `store` alone changes nothing, and `trappable`, `exact`, `tracing`, `verbose_tracing` are matched.

## Acceptance criteria
- [x] A generated export takes a record and returns a variant, and the value that crosses equals the value the test expects, on both targets.
- [x] `new` on a component whose export has a different record fails with `Error::TypeMismatch` and the path of the wrong field.
- [x] `new` on a component without one of the world's exports fails with `Error::MissingExport` that names it.
- [x] A test counts the runs of the type check around 1,000 calls of one `call_*` method, and the count does not change.
- [x] `exports: { default: async }` compiles and emits the same `call_*` as no flag.
- [x] The form with `interfaces` emits no world struct.
- [x] `lint` passes, and `tests all` is green on both targets with the conformance summary unchanged.


## Delivery notes

Delivered on the host in `feat(bindgen): Generate exports, async calls, resources, with, and errors` (with 9723b5 in `fix(linker): Link a used resource with no registration of its own` before it).

- **Design gap, borrowed parameters.** A typed call keeps its arguments until the task it starts runs (`CallValues: Send + 'static`), so `TypedFunc<(&str,), _>::call` cannot take a borrow. A generated `call_*` keeps Wasmtime's borrowed parameters (`&str`, `&[T]`, `&T` of a named type that holds a list) and copies each into an owned value before the call: one host copy per string or list. The `TypedFunc` it keeps is the owned one, and `func_*` returns a reference to it (`&TypedFunc<…>`), because `TypedFunc` is not `Clone`. A parameter that holds a handle, a stream, or a future is taken by value. Filed as 178248 (a typed call that takes borrowed arguments with no host copy).
- `Error::MissingExport { name, instance: Option<String> }`, `None` for an export of the component itself, with Wasmtime's messages. The generated generics are `_T`/`_D`, so a WIT type named `t` or `d` does not collide with them (the corpus has one).
- Tests: `rust/wcmp-bindgen-tests/tests/exports.rs`; the 1,000-call type-check count is `rust/wcmp/src/baseline/bindgen_world.rs`, which invokes the macro inside the polyfill with `wcmp_crate: crate` to read the crate's own count.
