---
id: 1cc7ae
title: bindgen imported and exported resources
type: feature
blocked_by: [5290bd, acefba, 39c0c7, e3d75d, c79ad4]
labels: [PDD028, bindgen]
created: 2026-10-17T02:55:52Z
disposition: accepted
disposition_at: 2026-10-21T06:27:09Z
---

## What to build

PDD028, "Resources and `with`" for imported and exported resources without `with` (Wasmtime `lib.rs:3293-3315` at `v49.0.0-rc.1`).

- **Imported resources.** For each resource `r`, a trait `HostR` and a trait `HostRWithStore<T>`, dividing the constructor, methods, and static functions by the plain/`store`/async rule. `HostR` has `drop(&mut self, Resource<R>) -> wcmp::Result<()>`. The registration calls `LinkerInstance::resource` with the name, `ResourceType::host::<R>()`, and a destructor that calls `drop`.
- **`[drop]r`.** `drop` is synchronous in every interface, including one of async functions. A `[drop]r` rule with `store` moves `drop` to `HostRWithStore<T>`, where it receives an `Access`. A `[drop]r` rule with `async` fails the compile.
- **`eq` resources.** A resource that one interface `use`s from another is registered only in the interface that defines it, following Wasmtime's rule.
- **Exported resources.** `ResourceAny`, as in Wasmtime. The interface getter has `call_constructor` and one `call_*` per method and static function, following the export rules (including `async | store`).

## Acceptance criteria
- [x] A resource crosses through `Resource<T>`: a generated constructor of an imported resource pushes a host value into a `ResourceTable` and returns its handle. A generated method receives the guest's `borrow` of it, and the rep matches. When the guest drops the handle, `drop` runs once. On both targets.
- [x] An exported resource crosses through `ResourceAny`: the host calls the generated `call_constructor` and a generated `call_*` method on the handle, and the method returns the value the guest computes from the constructor's argument.
- [x] A `[drop]r` rule with `store` gives `drop` an `Access`, and a `[drop]r` rule with `async` fails the compile with a message that names the rule.
- [x] An `eq` resource used by two interfaces links once and keeps one identity.
- [x] `lint` passes, and `tests all` is green on both targets with the conformance summary unchanged.


## Delivery notes

Delivered on the host in `feat(bindgen): Generate exports, async calls, resources, with, and errors` (with 9723b5 in `fix(linker): Link a used resource with no registration of its own` before it).

- The `eq` criterion needed 9723b5 (pulled into the thread with the owner's OK and labelled PDD028): the generated `add_to_linker` registers a `use`d resource only where it is defined, as Wasmtime's does.
- A store-flag `drop` takes `Access<'_, T, Self>`, built from the destructor's `StoreContext`. Tests: `rust/wcmp-bindgen-tests/tests/resources.rs`; the exported resource is exported through a nested component, the shape `wit-component` writes, because a bundled instance does not name the resource in its functions' types.
- Generated readers: `FutureReader` and `StreamReader` now implement `Debug` (by their end), as Wasmtime's do, so a generated type that holds one derives `Debug`.
