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
- [ ] A generated export takes a record and returns a variant, and the value that crosses equals the value the test expects, on both targets.
- [ ] `new` on a component whose export has a different record fails with `Error::TypeMismatch` and the path of the wrong field.
- [ ] `new` on a component without one of the world's exports fails with `Error::MissingExport` that names it.
- [ ] A test counts the runs of the type check around 1,000 calls of one `call_*` method, and the count does not change.
- [ ] `exports: { default: async }` compiles and emits the same `call_*` as no flag.
- [ ] The form with `interfaces` emits no world struct.
- [ ] `lint` passes, and `tests all` is green on both targets with the conformance summary unchanged.

