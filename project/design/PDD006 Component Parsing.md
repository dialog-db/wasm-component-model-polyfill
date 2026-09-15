# Component Parsing

[PDD005] established the foundations: `Engine`, `Store<T>`, the single `Error`
type, and the implementation posture. This document begins the work the
foundation exists for. It specifies the parsing surface, the type-system data
shapes that describe a component's imports and exports, and the identifier model
that keys them. It stops short of linking, instantiation, the Canonical ABI, and
resources.

This PDD also defines the synchronous baseline, the first feature tier of the
polyfill. Later PDDs measure themselves against it.

## Goals

- A developer constructs a `Component` from bytes through the public API and
  reads its declared imports and exports (package name, interface identifier,
  value type shape) without reaching for an upstream type.
- `Component` lives at the crate root (`wcmp::Component`) and wraps the
  translation it holds. No upstream type appears in the public API.
- The identifier model (`PackageName`, `InterfaceIdentifier`, semver
  constraints) is exposed through polyfill-owned types.
- The type-system surface describes every type in the synchronous baseline in
  data form.
- A component is translated once, at construction. Later work reuses the
  translation.
- `wcmp::Error` grows with a parse variant.

## Non-goals

- Linking, instantiation, host functions, the Canonical ABI, and resources.
- Anything outside the synchronous baseline, including every concurrency
  feature.
- Component-level `start`, host binding generation, and value imports.
- Any re-export of a runtime layer or upstream component-layer type.

## The Synchronous Baseline

The synchronous baseline is a named set of Component Model features. It is the
first feature tier the polyfill commits to, on top of the foundations of
[PDD005]. The term is project-defined for two reasons. First, "wasip2" is hard
to pin down in retrospect, because the label shifted in meaning over the life of
the proposal. Second, the prior art of [PDD002] implements a subset of wasip2,
so it is not a reference either. The baseline is the polyfill's own fix on the
problem.

The synchronous baseline includes:

- The binary format and section layout, including nested components and aliases,
  excluding the type-encoding bytes and `canon` built-ins that are specific to
  concurrency.
- The primitive types (`bool`, `s8` to `s64`, `u8` to `u64`, `f32`, `f64`,
  `char`, `string`), the compound types (`record`, `variant`, `list<T>`,
  `option<T>`, `result<T, E>`, `tuple<…>`, `flags`, `enum`), and the handle
  types `own<T>` and `borrow<T>`. Structural type equality holds.
- The synchronous Canonical ABI: lift and lower for every type above, the
  parameter and result spill to memory, the three string encodings,
  `cabi_realloc`, synchronous `post-return`, and handle tables that follow the
  runtime-state rules.
- Synchronous host function registration (typed and untyped) and synchronous
  host resource registration with synchronous destructors, organized by package
  name and interface identifier.
- Composition: components that contain other components, with the adapter
  modules, instance flags, and transcoders that connect them.
- The `Engine`, `Store<T>`, `Component`, `Linker<T>`, `LinkerInstance`, and
  `Instance` types, exposed as the polyfill's own surface.

The synchronous baseline excludes the concurrency tier. The concurrency tier is
the rest of the [PDD003] matrix: the `async` bit on function types, asynchronous
lifts and lowers, asynchronous destructors, the scheduler, tasks and subtasks,
backpressure, cancellation, context locals, waitable sets, `thread.yield`, event
encoding, `stream<T>`, `future<T>`, and `error-context`. It also excludes
`map<K, V>`, fixed-length lists, component subtyping beyond structural equality,
host binding generation, value imports, and `start`. Each of those is tracked in
[PDD003].

The baseline is not a claim of conformance to an external version label. It is a
contract inside the polyfill, intended to hold while the concurrency tier is
built. A change to the boundary is a deliberate amendment to this document.

## The Component Surface

`Component` is the parsed-component value. `Component::new(&engine, bytes)`
constructs it. The function is asynchronous, as [PDD005] requires, because the
core modules inside the component are compiled at construction and the browser
compiles asynchronously. The byte slice is borrowed for the duration of the
call.

Construction runs the component translator of [PDD002] exactly once. The
translator validates the binary and produces the plan a runtime needs: the core
modules, the instantiation sequence, the imports and exports with their types,
the canonical ABI options of every lift and lower, and the adapter modules for
composition. `Component` owns that plan, together with the compiled core
modules, so that instantiation does not parse or compile again. The plan is a
polyfill-owned shape. The translator's types do not appear in the public API.

The introspection accessors of `Component` derive from the translation's type
information. There is one source of truth for the type of an import or an
export. A malformed binary (corrupted preamble, truncated section, invalid type
reference) surfaces as a structured `wcmp::Error` that carries the byte offset
the translator reports.

The public surface stays small. Only the data shapes the user stories need are
introduced.

## The Type System Surface

The PDD introduces a `ValueType` family of data shapes that captures the
structural identity of every value type in the synchronous baseline. The shapes
are the result of `Component`'s import and export accessors. Host-side values,
lift and lower, and the Canonical ABI are out of scope.

Structural type equality holds at this level. Two identically shaped, separately
defined record types unify, as the [Explainer's type checking
rules][Explainer – type checking] require.

`own<T>` and `borrow<T>` appear as slots whose payload identifies the resource
they point at. Handle-table behavior is out of scope.

## The Identifier Model

The polyfill exposes `PackageName` and `InterfaceIdentifier`, with optional
semver constraints, as the keys of imports and exports. Their shapes mirror the
WIT package and interface names. This PDD introduces them as data, exposed
through `Component`'s accessors. Resolution logic is the linker's concern and is
out of scope.

## Error Model Growth

`wcmp::Error` grows with a parse variant. It covers a corrupted preamble, a
truncated section, an invalid reference, and a feature outside the synchronous
baseline. The translator's cause and byte offset are captured. [PDD005]'s note
about `anyhow::Error` in `#[source]` fields applies.

## User Stories

A developer adopting the polyfill wants to load a component and read its imports
and exports before the host-side work lands.

> The developer awaits `wcmp::Component::new(&engine, &bytes)`, walks the
> declared imports and exports, and matches against `PackageName` and
> `InterfaceIdentifier`. They do not reach for the runtime layer, Wasmtime, or
> the translator.

A contributor opening a PDD that needs the parsing surface wants it in place.

> The contributor writes their PDD's types against the existing `Component`
> without touching it. The error variants they need go into `wcmp::Error`.

A reviewer evaluating a PDD wants a readable scope.

> The reviewer makes sure that the PDD lands only the test cases, on both
> targets, and that no concurrency surface appears alongside the synchronous
> baseline.

A maintainer auditing the lineage of the component layer wants a clear answer to
"did this code originate in the prior art".

> The maintainer reads [PDD002] and finds an in-tree comment at every place a
> design choice came from the prior art.

## Test Cases

The preamble is recognized. A byte sequence with the component preamble
constructs a `Component`, and a byte sequence with the core module preamble is
rejected with the not-a-component error.

The top-level sections decode. A component with imports and exports yields their
names, package names, interface identifiers, and value type shapes through
polyfill-owned types.

A malformed binary is rejected. A truncated or corrupted component fails
construction with the parse error variant and a message that names the cause.

A component loads from bytes on both targets. The same bytes construct a
`Component` natively and in headless Chrome, and the declared imports and
exports match on both.

## References

- [PDD000], the product overview.
- [PDD001], the development environment.
- [PDD002], the ecosystem foundation, including the translator.
- [PDD003], the compatibility outlook. This PDD covers the parsing rows of the
  binary format and the data-shape rows of the type system.
- [PDD004], the test macros.
- [PDD005], the foundations and the implementation posture.
- [`wasmtime-environ`], the component translator.
- [Wasmtime], the reference implementation.
- [Explainer], the Component Model design document, including its [type checking
  rules][Explainer – type checking].
- [`thiserror`], the derive used by the error enum.

[PDD000]: ./PDD000%20Wasm%20Component%20Model%20Polyfill.md
[PDD001]: ./PDD001%20Development%20Environment.md
[PDD002]: ./PDD002%20Ecosystem%20Foundation.md
[PDD003]: ./PDD003%20Compatibility%20Outlook.md
[PDD004]: ./PDD004%20Test%20Macros.md
[PDD005]: ./PDD005%20Library%20Foundations.md
[`wasmtime-environ`]: https://docs.rs/wasmtime-environ
[`thiserror`]: https://docs.rs/thiserror
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
[Explainer]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md
[Explainer – type checking]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md#type-checking
