# Canonical ABI and Host Functions

[PDD006] introduced `Component` and the polyfill's type-system data
shapes; [PDD007] introduced `Linker<T>`, `LinkerInstance<'_, T>`, and
`Instance`, with instantiation and primitive-only export invocation.
This document picks up where those PDDs stopped: it introduces the
polyfill's host-side value family — `Val`, the value-bearing
counterpart to [PDD006]'s `ValueType` shapes — implements the
canonical ABI for every compound valtype in the synchronous baseline
*except* `own<T>` and `borrow<T>`, and adds typed and untyped
host-function registration on `LinkerInstance`.

This PDD inherits both boundaries [PDD006] established: the
synchronous baseline (defined in
[PDD006 §The Synchronous Baseline][pdd006-synchronous-baseline]) and
the parity-per-PDD discipline (expressed in
[PDD006 §Web Parity Per PDD][pdd006-web-parity-per-pdd]). Neither is
restated here.

## Goals

- A developer can register a host function — typed (statically-typed
  arguments and return) or untyped (`Val`-based) — against a
  `LinkerInstance<'_, T>`, and the linker checks at link time that the
  registration's declared type satisfies the import's declared type.
- A developer can call a component export whose signature uses any
  baseline valtype except `own<T>` and `borrow<T>`, and arguments and
  return values round-trip through the canonical ABI: lift, lower,
  `cabi_realloc` invocation for heap-allocating values, and sync
  `post-return` after the caller observes the return.
- The three corresponding tests in `tests/baseline_linking.rs` execute
  under both `test:native:*` and `test:web:*` without the `#[ignore]`
  attribute they currently carry. Concretely:
  `it_defines_an_untyped_host_function`,
  `it_defines_a_typed_host_function`, and
  `it_invokes_an_exported_component_function`.
- The un-stubbed tests run unconditionally on every supported target
  per [PDD006 §Web Parity Per PDD][pdd006-web-parity-per-pdd] — no
  `#[ignore]`, no target gate.
- `Val` lives in the polyfill's public API at the crate root
  (`wcmp::Val`) and wraps, rather than re-exports, any
  [`wasm_runtime_layer`] type or upstream component-layer type it
  happens to be built on top of.
- The single `wcmp::Error` enum grows additively with type-mismatch
  and ABI variants.
- This PDD leaves the canonical-ABI surface in a shape that does not
  have to be reshaped to accommodate handle-typed valtypes when those
  land.

## Non-goals

- `own<T>` and `borrow<T>` lift/lower; the handle table; host-
  resource registration with sync destructor — all are out of scope
  and tracked on [PDD003]'s checklist. The lift/lower machinery this
  PDD introduces is shaped to accept the handle valtypes additively
  but does not process them.
- Specialized list lift/lower fast paths (e.g. `list<u8>`,
  `list<u32>`) and string transcoders (UTF-8 ↔ UTF-16 ↔ Latin1+UTF-16)
  are permitted but not required by this PDD; observable behaviour
  is what the public API contracts on. Profiling-driven improvements
  to these paths are out of scope here.
- Async lift/lower (callback or stackful), per-task lift/lower context
  threading, and the generalised handle-table extension to `future<T>`
  and `stream<T>`. All are async-tier work.

## The Canonical ABI Surface

Calling an export — typed or untyped — exercises the canonical ABI.
This PDD covers, on every supported target:

- Lift and lower for every valtype in the synchronous baseline
  *except* `own<T>` and `borrow<T>`, in both argument and result
  position. This includes the primitives [PDD007] handled by direct
  passthrough and adds the compound and string types this PDD
  introduces.
- Invocation of the guest's `cabi_realloc` during lowering of
  heap-allocating values (e.g. `string`, `list<T>`, deeply-nested
  records), with the alignment and size discipline the
  [CanonicalABI] specifies.
- `post-return` after a sync lift, run after the caller observes the
  return value. Async lifts are out of scope per the synchronous
  baseline.

The lift/lower implementation operates on the runtime layer's
`Memory` through the crate-private accessors [PDD005] established; no
upstream type is exposed. Per
[PDD006 §Web Parity Per PDD][pdd006-web-parity-per-pdd], the
implementation lands on every supported target in the same change.

## The Host Value Surface

`Val` is the polyfill's host-side value. It is the value-bearing
counterpart to the `ValueType` data shapes [PDD006] introduced: every
shape in `ValueType` has a corresponding case in `Val` whose payload
carries the host-readable Rust representation of that valtype. `Val`
is the carrier for every untyped host function call (arguments are
`&[Val]`, return is `&mut [Val]`), the result of every untyped export
call, and the bridge between a typed host function's statically-typed
signature and the canonical ABI's runtime machinery.

`Val` does not directly expose [`wasm_runtime_layer`] or any upstream
component-layer type. Its representation for compound valtypes (e.g.
`Val::Record`, `Val::List`) carries owned, polyfill-typed data. The
cases corresponding to `own<T>` and `borrow<T>` are present in the
enum so the shape is closed; their payload semantics are out of
scope for this PDD.

## Host Function Registration

`LinkerInstance<'_, T>` gains two registration modes:

- **Untyped registration** takes a closure over the polyfill's `Val`
  slices, mirroring [`wasm_component_layer`]'s `define_func` (prior
  art only — the polyfill's implementation is original). Lift and
  lower happen at call time against the import's declared signature.
- **Typed registration** takes a closure with statically-typed
  arguments and return, mirroring [Wasmtime]'s
  `LinkerInstance::func_wrap`. Argument and return types are checked
  at link time against the import's declared signature; mismatches
  surface as a `wcmp::Error::TypeMismatch` before the linker accepts
  the registration.

A typed registration's argument and return types must each correspond
to a baseline valtype this PDD handles — i.e. anything but `own<T>`
and `borrow<T>`. Typed registrations whose signature involves a
handle valtype are rejected at link time with a
`wcmp::Error::TypeMismatch`; untyped registrations targeting a
handle-valtyped import are similarly rejected at call time.

## Error Model Growth

`wcmp::Error` grows additively. The variants this PDD introduces:

- A *type mismatch* variant for failures unifying a host
  registration's declared type against the component's declared type
  for that import (typed registration), or for failures unifying an
  export's declared type against a typed export-call's declared
  signature.
- An *ABI* variant for failures during lift or lower of a specific
  valtype, carrying enough context to identify the value position
  (argument index or return) and the valtype involved.

Each variant carries a `#[source]` cause where one is available;
[PDD005]'s note about [`anyhow::Error`] in `#[source]` fields applies
unchanged.

## User Stories

**As a developer adopting the polyfill**, I want to register a typed
host function against an interface and call a component export whose
signature uses strings, lists, and records, so that the polyfill is
featureful enough to back a non-trivial host on its own.

> The developer builds a `wcmp::Linker<T>` over an engine, registers a
> typed host function against an interface the component imports,
> instantiates with `linker.instantiate(&mut store, &component)`,
> calls the export, and observes the round-trip — including
> `cabi_realloc` invocation and `post-return` — through the
> polyfill's API.

**As a contributor opening a downstream PDD that builds on this
work**, I want the canonical ABI already in tree and stable except
for handle valtypes, so that my PDD is purely an extension to the
lift/lower machinery and the registration modes on `LinkerInstance`.

> The contributor reads this document, sees the additive shape, and
> writes their additions against the existing `LinkerInstance` and
> lift/lower implementation without reshaping either.

**As a reviewer evaluating an in-progress PDD**, I want the scope
of each PDD to be readable.

> The reviewer reads the goals and non-goals, confirms that the PR
> un-stubs only the three named tests — green on both
> `test:native:*` and `test:web:*`, with no target gate — that no
> handle-valtype lift/lower or host-resource registration surface
> sneaks in, and that the public API additions are limited to `Val`
> and the new registration modes on `LinkerInstance`.

## References

- [PDD000] — product overview.
- [PDD001] — development environment.
- [PDD002] — ecosystem foundation.
- [PDD003] — compatibility outlook. This PDD covers the canonical-
  ABI rows of "Canonical ABI" and the host-function rows of "Linking,
  Instantiation, and Host Integration".
- [PDD004] — test macros.
- [PDD005] — library foundations.
- [PDD006] — component parsing.
- [PDD007] — linking and instantiation; the PDD this one extends.
- [`wasm_runtime_layer`] — the runtime substrate.
- [`wasm_component_layer`] — prior art only.
- [Wasmtime] — the reference runtime.
- [Explainer] — the canonical Component Model design document.
- [CanonicalABI] — the canonical ABI rules this PDD's lift/lower
  implementation realises for the synchronous baseline's valtypes.
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
[pdd006-synchronous-baseline]: ./PDD006%20Component%20Parsing.md#the-synchronous-baseline
[pdd006-web-parity-per-pdd]: ./PDD006%20Component%20Parsing.md#web-parity-per-pdd
[PDD007]: ./PDD007%20Linking%20and%20Instantiation.md
[`wasm_runtime_layer`]: https://github.com/DouglasDwyer/wasm_runtime_layer
[`wasm_component_layer`]: https://github.com/DouglasDwyer/wasm_component_layer
[`thiserror`]: https://docs.rs/thiserror
[`anyhow::Error`]: https://docs.rs/anyhow
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
[Explainer]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md
[CanonicalABI]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md
[Subtyping]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Subtyping.md
