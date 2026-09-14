# Linking and Instantiation

[PDD006] introduced `Component`, the type-system data shapes, and the identifier
model. This document introduces the build-up surface and the runtime surface:
`Linker<T>`, `LinkerInstance<'_, T>`, and `Instance`, with the identifier
resolution logic that connects a component's imports to the host items a linker
holds.

This PDD inherits the synchronous baseline of [PDD006] and the implementation
posture of [PDD005].

## Goals

- A developer constructs a `Linker<T>` over an `Engine`, instantiates a
  `Component` into a `Store<T>` through the linker, and calls an exported
  function whose signature uses only primitive value types, on every supported
  target.
- The tests `it_instantiates_a_component_through_a_linker`,
  `it_supports_multiple_independent_instances`, and
  `it_resolves_package_and_interface_identifiers_with_semver` pass on every
  supported target.
- `Linker<T>`, `LinkerInstance<'_, T>`, and `Instance` live at the crate root
  and wrap, rather than re-export, any runtime layer type they use.
- Identifier resolution operates on [PDD006]'s identifier types only.
- The linker resolves a component's imports once. Instantiation consumes the
  result of that resolution and does not resolve again.
- `wcmp::Error` grows with link and instantiation variants.
- The linking surface accepts host item registration, Canonical ABI work, and
  resource work later without reshaping.

## Non-goals

- Host function and host resource registration. This PDD's `LinkerInstance`
  carries no host items. A component whose imports require host items fails with
  `wcmp::Error::Link`.
- Lift and lower for compound value types. Export invocation in this PDD covers
  signatures whose arguments and result are primitive value types (`bool`, `s8`
  to `s64`, `u8` to `u64`, `f32`, `f64`, `char`), which pass through the core
  function without touching memory.
- Synchronous `post-return`, which only becomes observable with compound value
  types.
- Composition of components. A component that contains other components is in
  the synchronous baseline but out of scope here.
- Anything outside the synchronous baseline.

## The Linker Surface

`Linker<T>` is the build-up of the host environment a component links against.
`Linker::new(&engine)` constructs it. Its `T` parameter matches the host data of
the `Store<T>` it is used with. The linker organizes host items by interface. A
`LinkerInstance<'_, T>` borrowed from a `Linker<T>` is the unit of one
interface's worth of host items. It is addressed by a `PackageName` and an
`InterfaceIdentifier` with optional semver constraints.

This PDD's `LinkerInstance` exposes the addressing surface only. It exists so
that the linker can be addressed by interface identifier and so that resolution
has a concrete candidate to match. The tests of this PDD use components whose
imports resolve to other component-instance exports, are absent, or name
interfaces whose item set is empty.

`Linker<T>::instantiate(&self, &mut Store<T>, &Component)` is an `async fn`, as
[PDD005] requires. It returns an `Instance` whose lifetime is bound to the
store. Several instances of one component can coexist in one store and stay
isolated. The store remains the unit of isolation.

## Identifier Resolution

Resolution walks a component's declared imports, matches each against the
linker's registered `LinkerInstance`s, and selects the candidate whose package
name, interface identifier, and version satisfy the import. A mismatch surfaces
as a `wcmp::Error::Link` variant with enough context to name the unresolved
import: no match, an ambiguous match, or an unsatisfiable version.

Resolution produces one binding per import, in declaration order. The linker
hands that result to instantiation. Instantiation reads the bindings to wire
each lowered import to its host item. It does not repeat the lookup.

Resolution operates on [PDD006]'s identifier types only. No upstream type
appears in the resolution interface or in diagnostics.

### Semver Compatibility

Two versions are compatible when they fall in the same WIT compatibility range.
That range is narrower than cargo-style caret matching:

- Before `1.0.0`, the range is the minor segment. `0.2.0` and `0.2.7` are
  compatible. `0.2.0` and `0.3.0` are not.
- From `1.0.0`, the range is the major segment. `1.4.0` and `1.7.2` are
  compatible. `1.4.0` and `2.0.0` are not.
- Pre-release identifiers and build metadata compare as the `semver` crate
  compares them. Two versions that differ only in build metadata are equal.
- An unversioned import matches an unversioned registration exactly. A versioned
  import does not match an unversioned registration, and the reverse. The
  polyfill does not widen one to the other.

When more than one candidate falls in the range, resolution selects the highest
version. When no candidate falls in the range, resolution surfaces
`wcmp::Error::Link`.

## The Instance Surface

`Instance` owns a linked and instantiated component. It exposes an export
lookup. Given a name, the lookup returns a function handle. Calling the handle
is an `async fn` that performs the Canonical ABI round trip. This PDD supports
the primitive-only subset of that round trip.

An `Instance` remembers the `Store<T>` it was created in. A call that passes a
different store returns a structured `wcmp::Error`. It does not panic.

`Instance` does not expose the runtime layer. Inside the crate, workspace-
internal accessors reach the core Wasm machinery.

## Error Model Growth

`wcmp::Error` grows with two variants:

- A link variant for a failure to resolve an import: no match, an ambiguous
  match, an unsatisfiable version, or an import that requires a host item the
  PDD does not support.
- An instantiation variant for a failure the runtime layer reports when a core
  module fails to instantiate, for a component that requires a feature the PDD
  defers, and for a call against the wrong store.

Each variant carries a `#[source]` cause where one exists. [PDD005]'s note about
`anyhow::Error` applies.

## User Stories

A developer adopting the polyfill wants to instantiate a component and call a
primitive-returning export in a few lines.

> The developer awaits `Component::new`, builds a `Linker<T>`, awaits
> `linker.instantiate(&mut store, &component)`, fetches an export, and awaits
> the call. They do not reach for the runtime layer or Wasmtime.

A contributor opening a PDD that builds on this work wants the linking surface
in place.

> The contributor adds registration modes to the existing `LinkerInstance`
> without touching `Linker` or `Instance`.

A reviewer evaluating a PDD wants a readable scope.

> The reviewer makes sure that the PDD lands only the three named tests, on both
> targets, and adds only link and instantiation error variants.

## References

- [PDD000], the product overview.
- [PDD002], the ecosystem foundation.
- [PDD003], the compatibility outlook. This PDD covers the instantiation rows of
  "Linking, Instantiation, and Host Integration".
- [PDD005], the foundations and posture.
- [PDD006], component parsing, the synchronous baseline, and the identifier
  model.
- [Wasmtime], the reference implementation.
- [Explainer], the Component Model design document.
- [`semver`], the crate whose comparison rules the polyfill follows.

[PDD000]: ./PDD000%20Wasm%20Component%20Model%20Polyfill.md
[PDD002]: ./PDD002%20Ecosystem%20Foundation.md
[PDD003]: ./PDD003%20Compatibility%20Outlook.md
[PDD005]: ./PDD005%20Library%20Foundations.md
[PDD006]: ./PDD006%20Component%20Parsing.md
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
[Explainer]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md
[`semver`]: https://docs.rs/semver
