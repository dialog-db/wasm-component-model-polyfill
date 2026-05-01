# Library Foundations

The Wasm Component Model Polyfill cannot host any Component Model machinery
until it has a public library API to host it *on*. This document specifies
the polyfill's foundational surface — the engine and store types every
piece of component-layer work depends on — and establishes the posture
under which the polyfill is built. The featureful work is small on
purpose: just enough surface for a contributor to construct a working
runtime against the polyfill's own API.

## Goals

- A developer can construct an `Engine` and a `Store<T>` through the
  polyfill's public API on the native target and use them as the foundation
  for any Component Model work the polyfill grows into.
- The polyfill's foundational types wrap, rather than re-export,
  [`wasm_runtime_layer`]'s counterparts, so that the public API is the
  polyfill's to evolve as wasip3 work proceeds and is not pinned to an
  upstream surface that may change for unrelated reasons.
- The library's error story is established once, in this PDD, as a
  single enum that grows additively as the polyfill takes on parse,
  link, type, and ABI concerns. Subsystem-specific error hierarchies
  are excluded by construction.
- `Engine` and `Store<T>` are shaped so that the addition of
  component-parsing, linker, and instance types on top of them does not
  reshape the foundational surface. The path is described well enough
  that no later work needs to revisit `Engine` or `Store<T>`'s public
  shape.

## Non-goals

- Component parsing, linking, instantiation, host function definition,
  or resource definition. These are out of scope for this PDD; the
  broader plan for working through them is tracked on [PDD003]'s
  checklist.
- The web target. The implementation posture below establishes native as
  the leading edge of every PDD; web parity follows once a feature's
  semantics are settled on native.
- The async runtime substrate, futures, streams, error-context, and every
  other wasip3-only concern enumerated in [PDD003]'s checklist.
- Any attempt to expose, re-export, or otherwise surface the underlying
  [`wasm_runtime_layer`] types as part of the polyfill's public API. The
  polyfill's API is the polyfill's; the runtime layer is an implementation
  detail.

## The Foundational Surface

Two types make up this PDD.

`Engine` is the polyfill's compilation context. It is a thin newtype over
[`wasm_runtime_layer`]'s `Engine`, parameterised by whatever backend the
runtime layer is configured with (the wasmtime backend on native; the
browser backend on `wasm32-unknown-unknown` is in scope for the polyfill
overall but is out of scope for this PDD). The polyfill's `Engine` is
constructible without arguments — `Engine::new()` returns a polyfill
`Engine` carrying a default-configured backend engine. Engines are cheap
to clone, share state internally, and are the moral equivalent of
[Wasmtime]'s `wasmtime::component::Engine` — which is also the type a
component-model layer can hang component compilation off.

`Store<T>` is the polyfill's owner of guest state. It is a thin newtype
over [`wasm_runtime_layer`]'s `Store<T, …>`, again parameterised by the
runtime layer's backend. The `T` parameter is host data that travels with
the store and is reachable from any host function the polyfill exposes.
`Store` is constructed from an `Engine` and a host-data value:
`Store::new(&engine, host_data)`. The store exposes `data(&self)` and
`data_mut(&mut self)` accessors so that host code can read and mutate its
host data without leaving the polyfill's API. As with [Wasmtime]'s
`wasmtime::Store`, the polyfill's store is the unit of isolation between
independent component instances, and is the natural anchor for the
instance state (tables, memories, resource handle tables) that
component-model work attaches to it.

Neither type exposes the underlying [`wasm_runtime_layer`] handles
directly. Where access to the runtime layer is needed within the polyfill
crate, callers reach into the wrapper through crate-private accessors.
Downstream consumers of the polyfill never see a runtime-layer type.

The two types live in the public API at the crate root (`wcmp::Engine`,
`wcmp::Store`) and are re-exported from `lib.rs`. Their module placement
within `src/` is an implementation detail and not normative here.

## The Error Model

The polyfill exposes a single error enum at the crate root (`wcmp::Error`)
derived with [`thiserror`]. This PDD introduces the enum with whatever
variants the foundational surface needs (at minimum a backend
construction failure and a backend store-creation failure, propagated from
[`wasm_runtime_layer`]). The enum grows additively as the polyfill takes
on parse, link, type, and ABI concerns; the polyfill does not introduce
parallel error types per subsystem.

Public functions return `Result<T, Error>` (with the polyfill's `Error`
elided through a crate-level `Result<T>` type alias if it improves
readability). Internal error sources from [`wasm_runtime_layer`] are
captured as `#[source]` fields on the relevant variant so that the
underlying cause is preserved for debugging without leaking the runtime
layer's types into the public API.

## Shape Compatibility With Component-Model Work

The foundational surface is shaped so that the component-model machinery
the polyfill grows into — a parsed `Component`, a `Linker`, an
`Instance` — can be added without reshaping `Engine` or `Store<T>`. The
shapes already implied by this PDD (an `Engine` from which compilation
hangs; a `Store<T>` against which instantiation runs; a single error
enum into which parse/link/typecheck variants accrue) anticipate the
needs of that work without committing to its details:

- A `Component::new(&engine, bytes)` constructor would parse a component
  binary against an engine. The signature would mirror [Wasmtime]'s
  `wasmtime::component::Component::new` for familiarity.
- A `Linker<T>::new(&engine)` constructor would produce a linker over
  the same backend. Its generic parameter would match the host-data
  parameter of the store it is used with.
- A `Linker::instantiate(&self, &mut Store<T>, &Component) -> Result<Instance>`
  method would produce an `Instance` whose lifetime is bound to the
  store.

None of those types are introduced in this PDD; they are listed only
to demonstrate that the foundational surface does not need to be
reshaped to accept them.

## Implementation Posture

This sub-section binds the interpretation of [PDD002]'s "seed crystal"
language and gestures at how future PDDs may be shaped.

The polyfill is a clean re-implementation on top of [`wasm_runtime_layer`].
[`wasm_component_layer`] is read as prior art and consulted as a
reference, but it is **not** taken on as a dependency, vendored into the
workspace, or copied verbatim into the polyfill's source. Its data
structures, traversal patterns, and ABI implementation choices inform
the polyfill's own design, but every line of source in the polyfill is
written for the polyfill.

## User Stories

**As a developer adopting the polyfill**, I want to construct an engine
and a store in a handful of lines, so that I have a familiar, ergonomic
foundation to build the rest of my host on.

> The developer writes `let engine = wcmp::Engine::new()?;` and
> `let mut store = wcmp::Store::new(&engine, ())?;`, runs `test:native:debug`,
> and watches the two foundational baseline tests turn green. They do not
> reach for the underlying runtime layer; the polyfill's API is enough.

**As a contributor opening a PDD that introduces component-layer
types (`Component`, `Linker`, `Instance`)**, I want the foundational
types already in tree, so that my PDD is purely additive and I am
not relitigating engine/store design while I work on component
parsing.

> The contributor reads this document, sees the
> shape-compatibility-with-component-model-work section, and writes
> their `Component::new(&engine, bytes)` against the existing `Engine`
> without touching it. The error variants they need are added to the
> existing `wcmp::Error` enum.

**As a reviewer evaluating an in-progress PDD**, I want to judge the
PDD's scope against an articulated discipline, so that "is this PR
doing too much" has a documented answer rather than a per-PR
negotiation.

> The reviewer reads the implementation posture and confirms that the
> PDD under review un-stubs a small, related group of baseline tests,
> lands native first, and extends the public API additively. Anything
> beyond that scope is asked to be split off.

## References

- [PDD000] — the polyfill's product overview.
- [PDD001] — the development environment, Nix shell, and menu commands
  this PDD's tests are exercised through.
- [PDD002] — the polyfill's relationship to [`wasm_runtime_layer`],
  [`wasm_component_layer`], and [Wasmtime]; this document binds the "seed
  crystal" language to "prior art only, no dependency, no vendored source", and
  inherits [PDD002 §Relationship to Wasmtime][pdd002-relationship-to-wasmtime]
  for Wasmtime.
- [PDD003] — the compatibility outlook and implementation checklist this
  document begins to work through.
- [PDD004] — the test macros every baseline test in this PDD is
  written against.
- [`wasm_runtime_layer`] — the runtime substrate the foundational types
  wrap.
- [`wasm_component_layer`] — prior art consulted during design; not a
  dependency, not vendored.
- [`thiserror`] — the derive used by the polyfill's error enum.
- [Wasmtime] — the reference runtime whose `wasmtime::component` API
  shapes the polyfill's familiar names.

[PDD000]: ./PDD000%20Wasm%20Component%20Model%20Polyfill.md
[PDD001]: ./PDD001%20Development%20Environment.md
[PDD002]: ./PDD002%20Ecosystem%20Foundation.md
[pdd002-relationship-to-wasmtime]: ./PDD002%20Ecosystem%20Foundation.md#relationship-to-wasmtime
[PDD003]: ./PDD003%20Compatibility%20Outlook.md
[PDD004]: ./PDD004%20Test%20Macros.md
[`wasm_runtime_layer`]: https://github.com/DouglasDwyer/wasm_runtime_layer
[`wasm_component_layer`]: https://github.com/DouglasDwyer/wasm_component_layer
[`thiserror`]: https://docs.rs/thiserror
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
