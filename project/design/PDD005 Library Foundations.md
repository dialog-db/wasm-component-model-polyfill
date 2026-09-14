# Library Foundations

The polyfill cannot host Component Model machinery until it has a public
library API to host it on. This document specifies the foundational surface:
the engine and store types that every component-layer type depends on, the
single error type, and the posture under which every later PDD is built.

## Goals

- A developer constructs an `Engine` and a `Store<T>` through the public API
  and uses them as the foundation for component-layer work.
- The foundational types wrap the runtime layer's counterparts. They do not
  re-export them, so the public API is the polyfill's to evolve.
- The library has one error type. It grows as the polyfill takes on parse,
  link, type, and ABI concerns. Subsystem-specific error hierarchies are
  excluded.
- Every entry point that compiles, instantiates, or calls guest code is
  asynchronous (the host awaits it) on every target.
- `Engine` and `Store<T>` keep their public shape when the component-layer
  types are added on top of them.

## Non-goals

- Component parsing, linking, instantiation, host functions, and resources.
- The concurrency runtime, futures, streams, and `error-context`.
- Any exposure of a runtime layer type in the public API.

## The Foundational Surface

Two types make up the surface.

`Engine` is the compilation context. It is a thin newtype over the runtime
layer's `Engine`, parameterized by the backend of the target: the Wasmtime
backend on native and the browser backend on `wasm32-unknown-unknown`.
`Engine::new()` returns an engine with a default configuration. Engines are
cheap to clone and share state internally. `Engine` is the equivalent of
[Wasmtime]'s `wasmtime::Engine`.

`Store<T>` owns guest state. It is a thin newtype over the runtime layer's
`Store<T, …>`. The `T` parameter is host data that travels with the store and
is reachable from every host function. `Store::new(&engine, host_data)`
constructs a store. `data(&self)` and `data_mut(&mut self)` give host code
access to the host data. As in Wasmtime, the store is the unit of isolation
between component instances. Every per-instance table that the Canonical ABI
requires (resource handles, waitables, tasks) is anchored to the store.

Neither type exposes the runtime layer. Inside the crate, workspace-internal
accessors reach the wrapped handles. A downstream consumer never sees a
runtime layer type.

Both types live at the crate root (`wcmp::Engine`, `wcmp::Store`). Their
module placement is an implementation detail.

## Asynchronous Entry Points

Every public entry point that compiles, instantiates, or calls guest code is
an `async fn`. This includes the constructors and methods that later PDDs add
for components, linkers, instances, and function handles. Two facts require
it.

First, the browser's `WebAssembly` API compiles and instantiates large
modules asynchronously. A synchronous compile on the main thread is rejected
above a size limit in Chrome. A polyfill that blocks cannot load a component
of realistic size.

Second, the Component Model's concurrency features require the host to await
guest progress. An `async` export yields to the host's event loop. A host
function that awaits a promise suspends the guest through [JSPI]. Both need
the outer call to be a future.

On native, the same entry points return futures. A synchronous component
resolves without suspending. Host code awaits on both targets and does not
branch on the platform.

## The Error Model

The polyfill exposes one error enum at the crate root (`wcmp::Error`),
derived with [`thiserror`]. This PDD introduces the variants the foundational
surface needs, at minimum a backend engine failure and a backend store
failure. The enum grows as later PDDs add variants. The polyfill does not add
a parallel error type per subsystem.

Public functions return `Result<T, Error>`, through a crate-level `Result<T>`
alias. A cause from the runtime layer is captured as a `#[source]` field, so
that the origin of an error is preserved without leaking the runtime layer's
types. The runtime layer surfaces its failures as `anyhow::Error`. That type
appears in `#[source]` fields only, and consumers treat it as opaque.

A polyfill entry point never panics on input that a user can produce. An
input the polyfill does not support yet returns a structured error that
names the unsupported feature.

## Shape Compatibility With Later Work

The surface is shaped so that later PDDs add `Component`, `Linker`, and
`Instance` without reshaping `Engine` or `Store<T>`:

- `Component::new(&engine, bytes)` parses a component binary against an
  engine. The signature mirrors `wasmtime::component::Component::new`.
- `Linker<T>::new(&engine)` produces a linker over the same backend. Its
  generic parameter matches the store's host data.
- `Linker::instantiate(&self, &mut Store<T>, &Component)` produces an
  `Instance` bound to the store.

This PDD does not introduce those types. They are listed to show that the
foundation accepts them.

## Implementation Posture

Three rules govern this PDD and every PDD after it.

Original work. The polyfill is a clean implementation on top of the runtime
layer. [`wasm_component_layer`] is prior art, as [PDD002] states. It is not a
dependency and is not vendored.

Web parity per PDD. Every PDD lands its web behavior in the same change as
its native behavior. A test that a PDD names runs on every supported target
with no `#[ignore]` and no target gate. The
`#[cfg_attr(target_arch = "wasm32", ignore = …)]` attribute exists and is a
valid transitional device within an in-flight branch. It never appears in an
accepted PDD's tests. If a feature's web semantics cannot land in the same
PDD as its native semantics, the PDD is too large.

Additive public API. Each PDD extends the public API. It does not reshape
the types an earlier PDD introduced, unless the PDD says that it revises an
earlier design.

## User Stories

A developer adopting the polyfill wants a familiar foundation in a few lines.

> The developer writes `let engine = wcmp::Engine::new()?;` and
> `let mut store = wcmp::Store::new(&engine, ())?;`. They do not reach for the
> runtime layer.

A contributor opening a PDD that introduces component-layer types wants the
foundation in place.

> The contributor writes `Component::new(&engine, bytes)` against the
> existing `Engine` without touching it. The error variants they need go into
> the existing `wcmp::Error` enum.

A reviewer evaluating a PDD wants a documented scope discipline.

> The reviewer makes sure that the PDD lands its tests on both targets,
> extends the public API additively, and stays within its goals.

## References

- [PDD000], the product overview.
- [PDD001], the development environment.
- [PDD002], the ecosystem foundation.
- [PDD003], the compatibility outlook and checklist.
- [PDD004], the test macros.
- [`wasm_runtime_layer`], the runtime layer.
- [`wasm_component_layer`], prior art.
- [`thiserror`], the derive used by the error enum.
- [Wasmtime], the reference implementation.
- [JSPI], the JavaScript Promise Integration proposal.

[PDD000]: ./PDD000%20Wasm%20Component%20Model%20Polyfill.md
[PDD001]: ./PDD001%20Development%20Environment.md
[PDD002]: ./PDD002%20Ecosystem%20Foundation.md
[PDD003]: ./PDD003%20Compatibility%20Outlook.md
[PDD004]: ./PDD004%20Test%20Macros.md
[`wasm_runtime_layer`]: https://github.com/DouglasDwyer/wasm_runtime_layer
[`wasm_component_layer`]: https://github.com/DouglasDwyer/wasm_component_layer
[`thiserror`]: https://docs.rs/thiserror
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
[JSPI]: https://github.com/WebAssembly/js-promise-integration
