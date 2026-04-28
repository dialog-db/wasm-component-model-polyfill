# Library Foundations

The Wasm Component Model Polyfill cannot host any Component Model machinery
until it has a public library API to host it *on*. This document specifies
the polyfill's foundational surface — the engine and store types every
subsequent slice depends on — and establishes the posture under which this
slice and every later one is built. The featureful work is small on
purpose: just enough surface for a contributor to construct a working
runtime against the polyfill's own API, leaving component parsing, linking,
and instantiation to the slices that follow.

The baseline test files already in tree ([PDD001], [PDD003], [PDD004])
serve as the progress map. This document peels two of those tests off the
`#[ignore]` list and stops there.

## Goals

- A developer can construct an `Engine` and a `Store<T>` through the
  polyfill's public API on the native target and use them as the foundation
  for any later Component Model work the polyfill grows into.
- The two corresponding baseline tests in `tests/baseline_linking.rs` —
  `it_constructs_an_engine` and `it_constructs_a_store` — execute under
  `test:native:*` without the `#[ignore]` attribute they currently carry.
- The polyfill's foundational types wrap, rather than re-export,
  [`wasm_runtime_layer`]'s counterparts, so that the public API is the
  polyfill's to evolve as wasip3 work proceeds and is not pinned to an
  upstream surface that may change for unrelated reasons.
- The library's error story is established once, in this slice, so that
  later slices add variants to a single enum rather than introducing
  parallel error hierarchies.
- The path from `Engine` + `Store` to the next slice (`Component`,
  `Linker`, `Instance`) is described well enough that the API surface does
  not have to be reshaped to accommodate it.

## Non-goals

- Component parsing, linking, instantiation, host function definition, or
  resource definition. These are subsequent slices and are out of scope
  here.
- The web target. The implementation posture below establishes native as
  the leading edge of every slice; web parity follows once a feature's
  semantics are settled on native.
- The async runtime substrate, futures, streams, error-context, and every
  other wasip3-only concern enumerated in [PDD003]'s checklist.
- Any attempt to expose, re-export, or otherwise surface the underlying
  [`wasm_runtime_layer`] types as part of the polyfill's public API. The
  polyfill's API is the polyfill's; the runtime layer is an implementation
  detail.

## The Foundational Surface

Two types make up this slice.

`Engine` is the polyfill's compilation context. It is a thin newtype over
[`wasm_runtime_layer`]'s `Engine`, parameterised by whatever backend the
runtime layer is configured with (the wasmtime backend on native; later
slices will add the browser backend on `wasm32-unknown-unknown`). The
polyfill's `Engine` is constructible without arguments — `Engine::new()`
returns a polyfill `Engine` carrying a default-configured backend engine.
Engines are cheap to clone, share state internally, and are the moral
equivalent of [Wasmtime]'s `wasmtime::component::Engine` — which is also
the type a future Component Model layer will hang component compilation
off.

`Store<T>` is the polyfill's owner of guest state. It is a thin newtype
over [`wasm_runtime_layer`]'s `Store<T, …>`, again parameterised by the
runtime layer's backend. The `T` parameter is host data that travels with
the store and is reachable from any host function the polyfill later lets
contributors define. `Store` is constructed from an `Engine` and a host-
data value: `Store::new(&engine, host_data)`. The store exposes
`data(&self)` and `data_mut(&mut self)` accessors so that host code can
read and mutate its host data without leaving the polyfill's API. As with
[Wasmtime]'s `wasmtime::Store`, the polyfill's store is the unit of
isolation between independent component instances; later slices will
attach instances and their tables, memories, and resource handle tables
to it.

Neither type exposes the underlying [`wasm_runtime_layer`] handles
directly. Where access to the runtime layer is needed by later slices
within the polyfill crate, those slices reach into the wrapper through
crate-private accessors. Downstream consumers of the polyfill never see a
runtime-layer type.

The two types live in the public API at the crate root (`wcmp::Engine`,
`wcmp::Store`) and are re-exported from `lib.rs`. Their module placement
within `src/` is an implementation detail and not normative here.

## The Error Model

The polyfill exposes a single error enum at the crate root (`wcmp::Error`)
derived with [`thiserror`]. This slice introduces the enum with whatever
variants the foundational surface needs (at minimum a backend
construction failure and a backend store-creation failure, propagated from
[`wasm_runtime_layer`]). Subsequent slices add variants — parse errors,
link errors, type errors, ABI errors — to this same enum. The polyfill
does not introduce parallel error types per subsystem.

Public functions return `Result<T, Error>` (with the polyfill's `Error`
elided through a crate-level `Result<T>` type alias if it improves
readability). Internal error sources from [`wasm_runtime_layer`] are
captured as `#[source]` fields on the relevant variant so that the
underlying cause is preserved for debugging without leaking the runtime
layer's types into the public API.

## Path to the Next Slice

The next slice introduces `Component`, `Linker`, and `Instance` — enough
surface to un-stub the remaining tests in `baseline_component_binary.rs`
and `baseline_linking.rs`. The shapes already implied by this slice (an
`Engine` from which compilation hangs; a `Store<T>` against which
instantiation runs; a single error enum into which parse/link/typecheck
variants accrue) cover everything the next slice needs. Specifically:

- `Component::new(&engine, bytes)` parses a component binary against an
  engine. The signature mirrors [Wasmtime]'s
  `wasmtime::component::Component::new` for familiarity.
- `Linker<T>::new(&engine)` constructs a linker over the same backend. Its
  generic parameter matches the host-data parameter of the store it is
  used with.
- `Linker::instantiate(&self, &mut Store<T>, &Component) -> Result<Instance>`
  produces an `Instance` whose lifetime is bound to the store.

None of those types are introduced in this slice; they are listed only to
demonstrate that the foundational surface does not need to be reshaped to
accept them.

## Implementation Posture

This sub-section binds the interpretation of [PDD002]'s "seed crystal"
language and governs how every subsequent slice is built.

The polyfill is a clean re-implementation on top of [`wasm_runtime_layer`].
[`wasm_component_layer`] is read as prior art and consulted as a
reference, but it is **not** taken on as a dependency, vendored into the
workspace, or copied verbatim into the polyfill's source. Its data
structures, traversal patterns, and ABI implementation choices inform
the polyfill's own design, but every line of source in the polyfill is
written for the polyfill.

Work proceeds in slices. Each slice un-stubs a small, related group of
baseline tests; lands behind a working menu command from [PDD001]; and
extends the public API additively rather than reshaping it. The native
target leads every slice. Web-target work for a given feature begins only
after that feature's semantics are settled on native — the polyfill is
not in the business of debugging two backends simultaneously while the
design is still in motion.

Because the polyfill's source is original work, it carries the polyfill's
own copyright and licensing throughout. Where a design choice is taken
from [`wasm_component_layer`] verbatim — a struct shape, a control-flow
pattern, an algorithm — the polyfill notes the prior-art origin in an
in-tree comment so that a future reader can cross-reference the upstream
implementation for context.

## User Stories

**As a developer adopting the polyfill**, I want to construct an engine
and a store in a handful of lines, so that I have a familiar, ergonomic
foundation to build the rest of my host on.

> The developer writes `let engine = wcmp::Engine::new()?;` and
> `let mut store = wcmp::Store::new(&engine, ())?;`, runs `test:native:debug`,
> and watches the two foundational baseline tests turn green. They do not
> reach for the underlying runtime layer; the polyfill's API is enough.

**As a contributor opening the next slice (`Component`, `Linker`,
`Instance`)**, I want the foundational types already in tree, so that my
slice is purely additive and I am not relitigating engine/store design
while I work on component parsing.

> The contributor reads this document, sees the path-to-next-slice
> section, and writes their `Component::new(&engine, bytes)` against the
> existing `Engine` without touching it. The error variants they need are
> added to the existing `wcmp::Error` enum.

**As a reviewer evaluating an in-progress slice**, I want to judge the
slice's scope against an articulated discipline, so that "is this PR
doing too much" has a documented answer rather than a per-PR
negotiation.

> The reviewer reads the implementation posture and confirms that the
> slice under review un-stubs a small, related group of baseline tests,
> lands native first, and extends the public API additively. Anything
> beyond that scope is asked to be split off.

**As a future maintainer auditing the polyfill's lineage**, I want a
clear, durable answer to "did this code originate in
`wasm_component_layer`", so that I can answer license and provenance
questions without archaeological work.

> The maintainer reads the implementation posture, sees that
> [`wasm_component_layer`] is prior art only, and finds in-tree comments
> at every place a design choice was deliberately taken from upstream.
> The audit is a few minutes of reading rather than a multi-day exercise.

## References

- [PDD000] — the polyfill's product overview.
- [PDD001] — the development environment, Nix shell, and menu commands
  this slice's tests are exercised through.
- [PDD002] — the polyfill's relationship to [`wasm_runtime_layer`] and
  [`wasm_component_layer`]; this document binds the "seed crystal"
  language to "prior art only, no dependency, no vendored source".
- [PDD003] — the compatibility outlook and implementation checklist this
  document begins to work through.
- [PDD004] — the test macros every baseline test in this slice is
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
[PDD003]: ./PDD003%20Compatibility%20Outlook.md
[PDD004]: ./PDD004%20Test%20Macros.md
[`wasm_runtime_layer`]: https://github.com/DouglasDwyer/wasm_runtime_layer
[`wasm_component_layer`]: https://github.com/DouglasDwyer/wasm_component_layer
[`thiserror`]: https://docs.rs/thiserror
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
