# Pre-wasip3 Public API

[PDD006] through [PDD009] together brought the polyfill's
synchronous baseline to feature-completeness on the wire: components
parse, link, instantiate, round-trip every synchronous-baseline
valtype through the canonical ABI, and host-owned resources observe
their sync destructors. What that work *does not* finish is the
shape of the polyfill's public API at the developer's eye level.
This PDD closes that gap before the async tier starts to pile new
surface area on top of an unfinished one.

Two surfaces are introduced. The first is *export navigation*: a
developer holding an `Instance` can walk the component's export tree,
reach an instance-typed export named by its `InterfaceIdentifier`,
and obtain a function handle from inside it. The second is the
*typed call surface*: a developer can convert an untyped function
handle into one whose Rust parameter tuple and return type are
checked against the export's component-level signature at the
moment of acquisition, and call it with native Rust values.

## Goals

- An `Instance` exposes an export navigator that reaches both root-
  level function exports and function exports nested inside an
  instance-typed export, addressed by the polyfill's identifier
  types.
- A function handle exposes a typed-conversion entry point that
  produces a handle whose Rust parameter tuple and return type are
  checked at acquisition against the export's declared signature.
- The polyfill's existing flat-name `Instance::get_func` accessor is
  preserved unchanged.
- The two corresponding tests in `tests/baseline_linking.rs` —
  `it_navigates_instance_typed_exports` and
  `it_supports_a_typed_export_call_surface` — run unconditionally on
  every supported target.

## Non-goals

- Async export invocation. The typed call surface introduced here is
  synchronous.
- A host-binding code generator (a `wit-bindgen!` equivalent).
- Convenience defaults that hide the engine, such as a
  `Linker::default()` constructor.
- Any extension of export-navigation reach beyond instance-typed and
  function-typed exports.

## The Export Navigator

`Instance` gains an `exports` accessor returning a polyfill-owned
navigator value. The navigator exposes two lookups: one keyed by
`InterfaceIdentifier` that returns a view onto a single instance-
typed export (e.g. the `(export "test:guest/foo" (instance …))`
form), and one keyed by function name that returns the polyfill's
`Func` for a function export. The function lookup is reachable both
at the root of the export tree and on the instance-export view —
nested function exports are visible only through the latter.

The navigator is a polyfill type; no upstream type appears at the
navigation boundary. The instance-name lookup uses the polyfill's
own `InterfaceIdentifier` so that traversal threads through the
identifier surface introduced earlier in the project.

The flat-name `Instance::get_func` accessor that has existed since
[PDD007] is preserved as the unchanged shorthand it has always been.
The navigator does not duplicate its semantics; it adds the
instance-typed traversal `get_func` cannot see.

## The Typed Call Surface

`Func` gains a typed-conversion entry point that consumes the
untyped handle and produces a typed handle. The typed handle's
parameter tuple type and return type are each constrained, by a
polyfill-defined trait, to correspond to a synchronous-baseline
valtype. The conversion checks at acquisition that the export's
declared component-level signature satisfies the requested Rust
types; mismatches surface as a `wcmp::Error::TypeMismatch`.

Calling the typed handle takes a tuple of native Rust values and
returns the native Rust return; the canonical-ABI round-trip — lift,
lower, `cabi_realloc`, `post-return` — happens behind the typed
surface, exactly as it does for the existing untyped call path,
and surfaces the same structured `wcmp::Error::Abi` failures.

The trait that constrains the typed handle's parameter and return
types is the polyfill's own. It mirrors the lift/lower/typed-
descriptor triple [Wasmtime]'s `wasmtime::component` exposes, but
is named in the polyfill's surface and implements only the
synchronous-baseline valtypes — `future<T>`, `stream<T>`, and
`error-context` are out of scope. `own<T>` and `borrow<T>` are
synchronous-baseline valtypes per [PDD009] and inhabit the typed
surface; the resource-type registration the typed handle is
checked against is the registration mode [PDD009] introduced on
`LinkerInstance`.

## Error Model Growth

`wcmp::Error::TypeMismatch` ([PDD008]) gains the lookup-time
signature-check failure modes the typed-conversion entry point
surfaces. The variant's payload grows additively to identify the
export the handle was acquired against, the declared component-
level signature, and the Rust parameter tuple and return type the
caller asked for.

[PDD005]'s note about [`anyhow::Error`] in `#[source]` fields applies
unchanged.

## User Stories

**As a developer adopting the polyfill**, I want to call a component
export with native Rust values and reach into a component whose
exports are organised under WIT-named interfaces, so that the
polyfill's developer-facing surface is featureful enough for a
non-trivial host without a code generator.

> The developer parses a component, instantiates it, navigates from
> the instance to a named export instance, looks up a function
> inside it, asks for a typed handle of
> `(Vec<String>, u32) -> String`, and calls it with native values.
> No upstream type appears in the host code; the polyfill's surface
> is the only API the developer reaches for.

**As a contributor opening the first async-tier PDD**, I want the
polyfill's synchronous developer-facing surface complete and stable
on every supported target, so that my PDD extends a finished
surface rather than completing one.

> The contributor reads this document, sees that the export
> navigator and the typed call surface are in tree, and writes the
> async-tier surface against them additively. The synchronous
> baseline is the floor; the async tier is the extension.

## References

- [PDD000] — product overview.
- [PDD001] — development environment.
- [PDD002] — ecosystem foundation.
- [PDD003] — compatibility outlook. This PDD closes the developer-
  facing-API rows of "Linking, Instantiation, and Host Integration"
  for the synchronous baseline.
- [PDD004] — test macros.
- [PDD005] — library foundations.
- [PDD006] — component parsing.
- [PDD007] — linking and instantiation; the PDD this one extends at
  the navigation surface.
- [PDD008] — canonical ABI and host functions; the PDD this one
  extends at the typed call surface and the type-mismatch error
  variant.
- [PDD009] — resources; supplies the handle valtypes the typed
  surface accepts.
- [`wasm_runtime_layer`] — the runtime substrate.
- [`wasm_component_layer`] — prior art only.
- [Wasmtime] — the reference runtime.
- [Explainer] — the canonical Component Model design document.
- [Subtyping] — structural-equality rules.
- [`thiserror`] — the derive used by the polyfill's error enum.
- [`anyhow::Error`] — the source-capture compromise [PDD005] notes.

[PDD000]: ./PDD000%20Wasm%20Component%20Model%20Polyfill.md
[PDD001]: ./PDD001%20Development%20Environment.md
[PDD002]: ./PDD002%20Ecosystem%20Foundation.md
[PDD003]: ./PDD003%20Compatibility%20Outlook.md
[PDD004]: ./PDD004%20Test%20Macros.md
[PDD005]: ./PDD005%20Library%20Foundations.md
[PDD006]: ./PDD006%20Component%20Parsing.md
[PDD007]: ./PDD007%20Linking%20and%20Instantiation.md
[PDD008]: ./PDD008%20Canonical%20ABI%20and%20Host%20Functions.md
[PDD009]: ./PDD009%20Resources.md
[`wasm_runtime_layer`]: https://github.com/DouglasDwyer/wasm_runtime_layer
[`wasm_component_layer`]: https://github.com/DouglasDwyer/wasm_component_layer
[`thiserror`]: https://docs.rs/thiserror
[`anyhow::Error`]: https://docs.rs/anyhow
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
[Explainer]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md
[Subtyping]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Subtyping.md
