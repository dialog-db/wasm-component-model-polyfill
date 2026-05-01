# Linking and Instantiation

[PDD006] introduced `Component`, the polyfill's parsed-component
value, along with the type-system data shapes and identifier model a
component's imports and exports are described in. This document picks
up where that PDD stopped: it introduces the build-up surface and
the runtime surface — `Linker<T>`, `LinkerInstance<'_, T>`, and
`Instance` — and the identifier-resolution logic the linker hangs off
[PDD006]'s data types.

This PDD inherits both boundaries [PDD006] established: the
*synchronous baseline* (defined in
[PDD006 §The Synchronous Baseline][pdd006-synchronous-baseline]) and
the *parity-per-PDD discipline* (expressed in
[PDD006 §Web Parity Per PDD][pdd006-web-parity-per-pdd]). Neither is
restated here; this document references back to [PDD006]'s umbrella
sections.

## Goals

- A developer can construct a `Linker<T>` over an `Engine`,
  instantiate a `Component` into a `Store<T>` through the linker, and
  call an exported function whose signature uses only primitive
  valtypes — all through the polyfill's public API, on every supported
  target.
- The three corresponding tests in `tests/baseline_linking.rs` execute
  under both `test:native:*` and `test:web:*` without the `#[ignore]`
  attribute they currently carry. Concretely:
  `it_instantiates_a_component_through_a_linker`,
  `it_supports_multiple_independent_instances`, and
  `it_resolves_package_and_interface_identifiers_with_semver`.
- The un-stubbed tests run unconditionally on every supported target
  per [PDD006 §Web Parity Per PDD][pdd006-web-parity-per-pdd] — no
  `#[ignore]`, no target gate.
- `Linker<T>`, `LinkerInstance<'_, T>`, and `Instance` live in the
  polyfill's public API at the crate root (`wcmp::Linker`,
  `wcmp::LinkerInstance`, `wcmp::Instance`) and wrap, rather than
  re-export, any [`wasm_runtime_layer`] type or upstream
  component-layer type they happen to be built on top of.
- Identifier resolution — picking the registered `LinkerInstance`
  whose `PackageName` + `InterfaceIdentifier` + semver constraints
  satisfy a component's import — is performed in terms of [PDD006]'s
  identifier types only.
- The single `wcmp::Error` enum grows additively with link and
  instantiation variants.
- This PDD leaves the linking surface in a shape that does not have
  to be reshaped to accommodate host-item registration, canonical-ABI
  work, or resource-handle work when those land.

## Non-goals

- Host function registration (typed and untyped) and the canonical
  ABI for compound valtypes — both are out of scope and tracked on
  [PDD003]'s checklist. This PDD's `LinkerInstance` exists, but it
  carries no host items; components whose imports require host
  functions cannot be successfully linked under this PDD and fail
  cleanly with a `wcmp::Error::Link`.
- Host-resource registration and the handle table — both are out of
  scope and tracked on [PDD003]'s checklist.
- Lift and lower for compound valtypes (`record`, `variant`, `list`,
  `option`, `result`, `tuple`, `flags`, `enum`, `string`, `own<T>`,
  `borrow<T>`). Export invocation in this PDD covers only signatures
  whose arguments and return are *primitive valtypes* (`bool`,
  `s8`–`s64`, `u8`–`u64`, `f32`, `f64`, `char`) — types whose
  canonical-ABI lift/lower is direct passthrough through the
  underlying core function and does not touch component-instance
  memory. Compound-valtype work is out of scope and tracked on
  [PDD003]'s checklist.
- Sync `post-return` execution, which only becomes observable once
  compound valtypes are in scope.
- Anything outside the synchronous baseline as defined in [PDD006].

## The Linker Surface

`Linker<T>` is the polyfill's build-up of the host environment a
component links against. It is constructed from an engine via
`Linker::new(&engine)`; its `T` parameter matches the host-data
parameter of the `Store<T>` it will eventually be used with. The
linker organises host items by interface: a `LinkerInstance<'_, T>`
borrowed from a `Linker<T>` is the unit of "a single interface's
worth of host items," addressed by a `PackageName` and an
`InterfaceIdentifier` with optional semver constraints — using
[PDD006]'s identifier types directly.

This PDD's `LinkerInstance` exposes the *addressing* surface only;
host-item registration modes are out of scope. A `LinkerInstance`
constructed under this PDD carries no host items; it exists so the
linker can be addressed by interface identifier and so identifier
resolution has a concrete candidate to match against. Components
whose imports require host items therefore cannot be successfully
linked under this PDD; the failure surfaces as a
`wcmp::Error::Link` that names the missing import.

Tests this PDD un-stubs operate on components whose imports either
resolve to other component-instance exports (a self-contained
component), are absent entirely, or are interfaces whose item set is
empty — sufficient for `it_resolves_…` to exercise identifier-
matching without needing host-item registration.

`Linker<T>::instantiate(&self, &mut Store<T>, &Component) ->
Result<Instance>` produces an `Instance` whose lifetime is bound to
the store. Multiple instances of the same component can coexist in a
single store and remain isolated; the store remains the unit of
isolation, as established in [PDD005].

## Identifier Resolution

[PDD006] introduced `PackageName`, `InterfaceIdentifier`, and the
semver-constraint type as data; this PDD introduces the *resolution*
logic that walks a component's declared imports, matches each against
the linker's registered `LinkerInstance`s, and selects the candidate
whose package name, interface identifier, and version satisfy the
import's constraint. Mismatches surface as `wcmp::Error::Link`
variants with enough context to identify the unresolved import
(missing match, ambiguous match, unsatisfiable semver constraint).

Resolution operates entirely on [PDD006]'s identifier types; no
upstream type appears in the resolution interface or in error
diagnostics.

### Semver Compatibility

Two versions are considered compatible when they fall in the same WIT
*compatibility range*, which is the WIT specification's reading of
semver and is narrower than cargo-style caret matching:

- For pre-`1.0.0` versions, the compatibility range is the *minor*
  segment: `0.2.0` and `0.2.7` are compatible; `0.2.0` and `0.3.0` are
  not. Patch differences within a minor are tolerated.
- For `>= 1.0.0` versions, the compatibility range is the *major*
  segment: `1.4.0` and `1.7.2` are compatible; `1.4.0` and `2.0.0` are
  not.
- Pre-release identifiers (`-alpha.1`, etc.) and build metadata
  (`+sha.abc123`) are compared as the semver crate compares them; two
  versions that differ only in build metadata are equal.
- An import without a version matches a registration without a version
  exactly. A versioned import does not match an unversioned
  registration, and vice versa; the polyfill does not silently widen
  one to the other.

When more than one registered candidate falls in an import's
compatibility range, resolution selects the *highest-versioned*
candidate; an exactly-equal candidate ties with itself and is chosen
deterministically. When no registered candidate falls in range, or
when registration is ambiguous in a way the rules above do not
disambiguate, resolution surfaces a `wcmp::Error::Link`.

## The Instance Surface

`Instance` is the polyfill's owner of a successfully linked,
instantiated component. It exposes an export-lookup accessor — given
a name and an expected type, returns a handle from which a function
export can be called. Calling a function export performs the
canonical-ABI round-trip; this PDD supports the *primitive-only*
subset of that round-trip. Lift/lower for compound valtypes is out
of scope and tracked on [PDD003]'s checklist.

`Instance` does not directly expose [`wasm_runtime_layer`] or any
upstream component-layer type. Where access to the runtime layer is
needed within the polyfill crate to drive the underlying core-Wasm
machinery, it is reached through crate-private accessors as
established in [PDD005].

## Error Model Growth

`wcmp::Error` grows additively. The variants this PDD introduces:

- A *link* variant for failures resolving an import against a linker
  (missing import, ambiguous match, unsatisfiable semver constraint,
  registration of an item the PDD does not support).
- An *instantiation* variant for failures the runtime layer reports
  when the instantiated component fails to start, and for the
  component shapes this PDD intentionally rejects (e.g. an export
  whose signature requires compound-valtype lift/lower the PDD
  defers).

Each variant carries a `#[source]` cause where one is available;
[PDD005]'s note about [`anyhow::Error`] in `#[source]` fields applies
unchanged.

## Implementation Posture

This PDD is governed by
[PDD005 §Implementation Posture][pdd005-implementation-posture] as refined by
[PDD006 §Implementation Posture][pdd006-implementation-posture]. The same three
points apply: original work; web parity per PDD; purely additive at the public
API.

A reviewer checking scope against the PDD discipline should find
the surface this PDD introduces at the crate root (`Linker<T>`,
`LinkerInstance<'_, T>`, `Instance`), the identifier-resolution logic
hanging off [PDD006]'s data types, and the link and instantiation
variants extending the existing error enum — and nothing else.

## User Stories

**As a developer adopting the polyfill**, I want to instantiate a
known-good component and call one of its exported primitive-returning
functions in a few lines of code, so that I can prove the polyfill's
runtime path end-to-end before host-function work lands.

> The developer parses a component with
> `wcmp::Component::new(&engine, &bytes)`, builds a `wcmp::Linker<T>`
> over the same engine, instantiates with
> `linker.instantiate(&mut store, &component)`, fetches a
> primitive-returning export, and calls it. They do not reach for
> `wasm_runtime_layer`, `wasmtime`, or any upstream type.

**As a contributor opening a downstream PDD that builds on this
work**, I want the linking and instantiation surface already in tree
and stable, with `Linker<T>`, `LinkerInstance<'_, T>`, and `Instance`
exposed through the polyfill's own types, so that my PDD extends
them additively without reshaping the public API.

> The contributor reads this document, sees the additive shape, and
> writes their additions against the existing `LinkerInstance` and
> `Instance` without touching them.

**As a reviewer evaluating an in-progress PDD**, I want the scope
of each PDD to be readable, so that "is this PR doing too much" has
a documented answer.

> The reviewer reads the goals and non-goals, confirms that the PR
> un-stubs only the three named tests — green on both
> `test:native:*` and `test:web:*`, with no target gate — and adds
> only link and instantiation variants to the error enum, and rejects
> any creep beyond that.

## References

- [PDD000] — product overview.
- [PDD001] — development environment.
- [PDD002] — ecosystem foundation.
- [PDD003] — compatibility outlook. This PDD covers the
  instantiation rows of "Linking, Instantiation, and Host
  Integration".
- [PDD004] — test macros.
- [PDD005] — library foundations.
- [PDD006] — component parsing; defines the synchronous baseline,
  the parity-per-PDD convention, and the identifier model this
  PDD builds on.
- [`wasm_runtime_layer`] — the runtime substrate.
- [`wasm_component_layer`] — prior art only.
- [Wasmtime] — the reference runtime.
- [Explainer] — the canonical Component Model design document.
- [`thiserror`] — the derive used by the polyfill's error enum.
- [`anyhow::Error`] — the source-capture compromise [PDD005] notes.

[PDD000]: ./PDD000%20Wasm%20Component%20Model%20Polyfill.md
[PDD001]: ./PDD001%20Development%20Environment.md
[PDD002]: ./PDD002%20Ecosystem%20Foundation.md
[PDD003]: ./PDD003%20Compatibility%20Outlook.md
[PDD004]: ./PDD004%20Test%20Macros.md
[PDD005]: ./PDD005%20Library%20Foundations.md
[pdd005-implementation-posture]: ./PDD005%20Library%20Foundations.md#implementation-posture
[PDD006]: ./PDD006%20Component%20Parsing.md
[pdd006-synchronous-baseline]: ./PDD006%20Component%20Parsing.md#the-synchronous-baseline
[pdd006-web-parity-per-pdd]: ./PDD006%20Component%20Parsing.md#web-parity-per-pdd
[pdd006-implementation-posture]: ./PDD006%20Component%20Parsing.md#implementation-posture
[`wasm_runtime_layer`]: https://github.com/DouglasDwyer/wasm_runtime_layer
[`wasm_component_layer`]: https://github.com/DouglasDwyer/wasm_component_layer
[`thiserror`]: https://docs.rs/thiserror
[`anyhow::Error`]: https://docs.rs/anyhow
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
[Explainer]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md
