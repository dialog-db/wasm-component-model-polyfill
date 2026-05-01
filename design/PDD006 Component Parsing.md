# Component Parsing

[PDD005] established the polyfill's library foundations — `Engine`,
`Store<T>`, the single `Error` enum, and the implementation posture
under which every PDD is built. This document begins the work the
foundation exists *for*: it specifies the polyfill's parsing surface,
the type-system data shapes a component's imports and exports are
described in, and the identifier model imports and exports are keyed
by. It deliberately stops short of linking, instantiation, the
canonical ABI, and resources — those are out of scope here, and the
polyfill-internal inventory of where they sit relative to the work
already done is tracked in [PDD003]'s checklist.

This PDD is shaped by two boundaries. The first is the *synchronous
baseline*, a project-defined featureful watermark introduced below.
The second is the platform boundary: every PDD lands web parity in
the same change as native, refining [PDD005]'s "native leads"
sequencing into the polyfill's standing posture for the PDDs that
follow. The synchronous baseline section and the parity-per-PDD
convention below are written as *umbrella material* — they are
project-wide conventions that this document establishes once and that
the polyfill's later work draws on without restating.

## Goals

- A developer can construct a `Component` from bytes through the
  polyfill's public API and introspect its declared imports and
  exports — package name, interface identifier, declared valtype shape
  — without reaching for any upstream type.
- The four corresponding tests in
  `tests/baseline_component_binary.rs` and `tests/baseline_linking.rs`
  execute under both `test:native:*` and `test:web:*` without the
  `#[ignore]` attribute they currently carry. Concretely:
  `it_parses_the_component_preamble`,
  `it_decodes_top_level_component_sections`,
  `it_rejects_a_malformed_component_binary`, and
  `it_loads_a_component_from_bytes`.
- The un-stubbed tests run unconditionally on every supported target.
  No `#[cfg_attr(target_arch = "wasm32", ignore = …)]` gate is
  applied (see [Web Parity Per PDD][pdd006-web-parity-per-pdd]
  below).
- `Component` lives in the polyfill's public API at the crate root
  (`wcmp::Component`) and wraps, rather than re-exports, any
  [`wasm_runtime_layer`] type or upstream component-layer type it
  happens to be built on top of.
- The polyfill's identifier model — `PackageName`,
  `InterfaceIdentifier`, semver constraints — is exposed through the
  polyfill's own types, so that downstream consumers never see a
  runtime-layer or upstream component-layer type even when introspecting
  a component's imports by interface name.
- The polyfill's public API for this PDD is no larger than the
  introspection contract above. `Component`, the type-system data
  shapes, and the identifier model land at the crate root; the
  internals that back them (the `wit-component` / `wit-parser` types
  the parser is delegated to) do not. The polyfill is a polyfill, not
  a re-export.
- The polyfill's type-system surface — enough valtype data shapes to
  describe every type listed in the synchronous baseline — is in tree
  in *data form*. Host-side values, lift/lower, and the canonical ABI
  proper are out of scope for this PDD.
- The single `wcmp::Error` enum introduced in [PDD005] grows
  additively with a parse variant.
- This PDD leaves the parsing surface in a shape that does not have
  to be reshaped to accommodate linking, instantiation, the canonical
  ABI, or resources when those land.

## Non-goals

- Linking, instantiation, host function registration, the canonical
  ABI, and host-resource registration. These are deferred; the
  polyfill's broader plan for working through them is tracked on
  [PDD003]'s checklist.
- Anything outside the synchronous baseline as defined below. That
  includes every async-tier concern enumerated in [PDD003]'s checklist
  and every wasip3-specific extension to the type system, ABI, and
  runtime substrate.
- Component-level `start` functions, host-binding code generation
  (a `wit-bindgen!` equivalent), and value imports / value exports —
  each tracked separately on [PDD003]'s checklist and out of scope
  for the entire synchronous-baseline group of work.
- Re-exporting any [`wasm_runtime_layer`] or upstream
  [`wasm_component_layer`] type as part of the polyfill's public API.

## The Synchronous Baseline

This document refers throughout to *the synchronous baseline*. The
term is project-defined for two reasons. First, "wasip2" is a
difficult quantity to nail down in retrospect; the version label has
shifted in meaning over the course of the proposal and is becoming
harder to pin the further the ecosystem moves past it. Second,
[`wasm_component_layer`] does not implement a complete wasip2 anyway
— its test corpus stops short of several wasip2-era features, and
what it does implement is itself the result of one project's reading
of a moving target. Anchoring the polyfill's near-term watermark to
either of those references would import a definitional ambiguity the
polyfill does not need.

The synchronous baseline is the polyfill's own fix on the problem: a
named, enumerated set of Component Model features that the polyfill
commits to implementing as its first featureful tier, on top of
[PDD005]'s foundational types. It is the shape every PDD in the
polyfill's first-tier roadmap measures itself against; this document
is the canonical place the term is defined, and any later use means
the list below.

The synchronous baseline includes:

- The Component Model's binary format and section layout, excluding
  type-encoding bytes and `canon` opcodes that are specific to the
  async tier.
- The component type system's primitives (`bool`, `s8`–`s64`,
  `u8`–`u64`, `f32`, `f64`, `char`, `string`), compound types
  (`record`, `variant`, `list<T>`, `option<T>`, `result<T, E>`,
  `tuple<…>`, `flags`, `enum`), and `own<T>` / `borrow<T>` resource
  handles. Structural type equality is preserved.
- The synchronous canonical ABI: lift and lower for every type above,
  invocation of the guest's `cabi_realloc` during lowering, sync
  `post-return`, and a handle table whose index allocation and reuse
  semantics follow the canonical ABI's runtime-state rules.
- Sync host function registration (typed and untyped) and sync host
  resource registration with sync destructors, organised by package
  name and interface identifier.
- The `Engine`, `Store<T>`, `Component`, `Linker<T>`,
  `LinkerInstance`, and `Instance` types the polyfill's first-tier
  work introduces, exposed as the polyfill's own surface.

The synchronous baseline explicitly excludes — these are the *async
tier* of the polyfill's roadmap, taken up after the synchronous
baseline is fully in place:

- The `async?` bit on function types; async `canon lift` (callback or
  stackful); async `canon lower`; async resource destructors; per-task
  lift/lower context threading.
- The cooperative scheduler, task lifecycle, `task.return`,
  backpressure, cancellation (`task.cancel`, `subtask.cancel`,
  `cancellable` waits/polls), context-locals, structured concurrency,
  `yield`, waitable sets, and event encoding for waits/polls/callbacks.
- The async-only valtypes: `future<T>`, `stream<T>`, and
  `error-context`, including their lifecycle ABIs.
- Component subtyping (variance, depth, width). Structural equality is
  in the synchronous baseline; subtyping proper is in the async tier.
- Cross-component resource handle transfer trampolines.
- Component-level `start`, host-binding code generation, value
  imports / value exports, and the WIT `@since` / `@unstable` feature
  gate handling — each tracked separately on [PDD003]'s checklist.

The synchronous baseline is not a claim of conformance to any external
version label. It is a polyfill-internal contract, intended to hold
stable while the async tier is being designed and built on top of it.
A revision to the boundary is a deliberate amendment to this document,
not an in-passing widening or narrowing of the term.

## The Component Surface

`Component` is the polyfill's parsed-component value, and the only
public component-layer type this PDD introduces. It is constructed
from an engine and a byte slice via `Component::new(&engine, bytes)`,
mirroring [Wasmtime]'s `wasmtime::component::Component::new`. The byte
slice is borrowed only for the duration of the call; the parsed
representation is the polyfill's. A `Component` exposes its declared
imports and exports through accessors on the value itself, so that
test code (and downstream tooling) can introspect structural shape
without reaching for an upstream type. Malformed binaries — corrupted
preamble, truncated section, unknown section tag at a fatal position —
surface as a structured `wcmp::Error` variant rather than a panic.

Parsing, validation, and the type-space resolution that follow from the binary
are delegated to [`wit-component`] — specifically `wit_component::decode`, which
returns a high-level [`wit-parser`] view (`Resolve` plus a world id) the
polyfill walks to produce its own data shapes. The polyfill does not pull
`wasmparser` directly or hand-roll a parser of its own; `wit-component` is the
parser. Because both `wit-component` and `wit-parser` are pure Rust and
target-agnostic, the same lowering drives `Component::new` on every supported
target — there is no platform-divergent parsing path in this PDD, and parity
falls out of the implementation rather than being chased after. This sits
naturally inside the cross-target stance
[PDD002 §Relationship to Wasmtime][pdd002-relationship-to-wasmtime]
establishes: component-level work is built on top of `wasm_runtime_layer`'s
generic abstractions on every target, and parsing is the simplest case of
that — a target-agnostic library in, the polyfill's own data shapes out, no
substrate divergence at all.

The types `Component` exposes through its introspection accessors are
the polyfill's own — the type-system data shapes and identifier types
introduced below — so that a test or tool walking a component's
imports never sees an upstream type. This PDD keeps that surface
deliberately small: only the data shapes the PDD's user stories
require are introduced. Both `wit-component`'s and Wasmtime's
component API surfaces are large, and the polyfill is not obliged to
mirror either; the smaller the polyfill's public surface, the less
the PDDs that follow have to constrain their own implementation
choices around it.

## The Type System Surface

This PDD introduces enough of the polyfill's own type-system surface
to describe every type listed in the synchronous baseline. It does so
in *data form* only: a `ValueType` (or comparably-named) family of
shapes that captures the structural identity of every valtype, usable
as the result of `Component`'s import/export introspection accessors.
Host-side values (a `Val` family that carries data through host
function calls), lift/lower, and the canonical ABI proper are out of
scope for this PDD.

Structural type equality is preserved at this PDD's level: two
identically-shaped, separately-defined record types unify, per
[Subtyping]. Subtyping proper (variance, depth, width) is async-tier
work and is out of scope here.

`own<T>` and `borrow<T>` appear in the type-system data shapes as
slots whose payload type identity is sufficient for parsing and
reporting — that is, the introspection surface this PDD introduces
recognises a handle when it sees one and reports the resource it
points at. Handle-table behaviour (allocation, ownership transfer,
destructor invocation) is out of scope and deferred.

## The Identifier Model

The polyfill exposes its own identifier types — `PackageName` and
`InterfaceIdentifier`, with optional semver constraints attached — as
the keys imports and exports are addressed by. Their shapes mirror
the upstream component-layer model so the mental mapping is clear,
but no upstream type is exposed to consumers. This PDD introduces
them as data only, exposed through `Component`'s import/export
accessors. Resolution logic — semver matching against a registered
catalogue, selecting between candidate registrations — is the
linker's concern and is out of scope here.

## Error Model Growth

`wcmp::Error` grows additively. The variant this PDD introduces:

- A *parse* variant for failures decoding a component binary
  (corrupted preamble, truncated section, unsupported encoding inside
  the synchronous baseline). The underlying parser cause is captured
  as `#[source]`.

[PDD005]'s note about [`anyhow::Error`] in `#[source]` fields applies
unchanged. Public functions continue to return the crate-level
`Result<T>` alias. The enum continues to grow additively as the
polyfill takes on link, instantiation, type-mismatch, and ABI
concerns.

## Web Parity Per PDD

Every PDD in the polyfill's first-tier roadmap lands web parity in
the same change as native. A test that this PDD (or any later PDD
following the same posture) un-stubs runs unconditionally on every
supported target — `test:native:*` and `test:web:*` — with no
`#[ignore]` attribute and no target gate. The polyfill is not in the
business of carrying a long-lived target-divergent test inventory; if
a feature's web semantics cannot land in the same PDD as its native
semantics, the PDD is too large.

The mechanism this convention rules out is the
`#[cfg_attr(target_arch = "wasm32", ignore = "…")]` attribute. It
exists in Cargo and `wasm-bindgen-test`, and is occasionally useful as
a transitional device *within* an in-flight branch — for example, to
keep the wasm32 build green while a follow-up commit on the same
branch lands the last piece of web parity — but it never appears in a
merged PDD. A reviewer who finds a target gate on a test in a PR's
diff should ask why the PDD cannot deliver parity in the same
change.

The convention, expressed this way, is that:

- A test un-stubbed by a PDD is green on both `test:native:*` and
  `test:web:*` from the moment the PDD lands. The wasm32 build does
  not regress, and the test output is the same shape on every target.
- A test never *acquires* a target gate as part of merged feature
  work; the only path from `#[ignore]` (stub) to "running on every
  supported target" passes through "running on every supported
  target." There is no merged interim where the test is gated.
- The implementation choices a PDD makes are constrained by parity: a PDD that
  needs platform-divergent code is responsible for both paths in the same
  change. The parsing PDD's choice of `wit-component` as the parser (see
  [The Component Surface][pdd006-component-surface] above) is one expression of
  this discipline — picking a target-agnostic library means the PDD carries no
  platform-divergent parsing code at all.

This convention is the artifact-level expression of the polyfill's
PDD discipline: parity is part of "feature complete" for a PDD,
not a follow-up. The convention is established once here and applies
to any later work that follows the same posture.

## User Stories

**As a developer adopting the polyfill**, I want to load a known-good
component and read its declared imports and exports in a few lines of
code, so that I can integrate the polyfill into my build pipeline
before the host-side work to instantiate or call into it has landed.

> The developer parses a known-good component with
> `wcmp::Component::new(&engine, &bytes)`, walks its declared imports
> and exports through accessors on the value, and matches against the
> polyfill's `PackageName` and `InterfaceIdentifier` types. They do
> not reach for `wasm_runtime_layer`, `wasmtime`, or any upstream
> type; the polyfill's API is enough for the parsing-only surface.

**As a contributor opening a downstream PDD that needs the parsing
surface**, I want `Component`, the type-system data shapes, and the
identifier model already in tree and stable, so that my PDD is
purely additive and I am not relitigating parser design while I work
on whatever component-layer concern follows.

> The contributor reads this document, sees that the parsing surface
> is established, and writes their own PDD's types against the
> existing `Component` without touching it. The error variants they
> need are added to the existing `wcmp::Error` enum.

**As a reviewer evaluating an in-progress PDD**, I want the scope of
each PDD to be as readable as [PDD005]'s was, so that "is this PR
doing too much" continues to have a documented answer.

> The reviewer reads the goals and non-goals, confirms that the PR
> un-stubs only the named tests — green on both `test:native:*` and
> `test:web:*`, with no target gate — that no async-tier surface
> sneaks in alongside the synchronous baseline, and that the public
> API additions match the named types. Anything beyond that scope is
> asked to be split off.

**As a future maintainer auditing the polyfill's component-layer
lineage**, I want a clear answer to "did this code originate in
[`wasm_component_layer`]", so that I can answer license and provenance
questions for the component-layer code as easily as for the
foundational code.

> The maintainer reads the implementation-posture restatement, sees
> that [`wasm_component_layer`] remains prior art only, and finds
> in-tree comments at every place a design choice was deliberately
> taken from upstream. The audit is a few minutes of reading rather
> than a multi-day exercise.

## References

- [PDD000] — the polyfill's product overview.
- [PDD001] — the development environment, Nix shell, and menu commands
  this PDD's tests are exercised through.
- [PDD002] — the polyfill's relationship to [`wasm_runtime_layer`],
  [`wasm_component_layer`], and [Wasmtime]; this document inherits
  PDD002's "prior art only, no dependency, no vendored source"
  reading for the runtime-agnostic upstream, and the cross-target
  posture
  [PDD002 §Relationship to Wasmtime][pdd002-relationship-to-wasmtime]
  establishes for component-level work — the parsing surface is the
  simplest case of that posture (see
  [The Component Surface][pdd006-component-surface] above).
- [PDD003] — the compatibility outlook and implementation checklist.
  This PDD covers the parsing rows of "Component Binary Format" and
  the data-shape rows of "Component Type System".
- [PDD004] — the test macros every baseline test in this PDD is
  written against.
- [PDD005] — the foundational `Engine`, `Store`, `Error`, and the
  implementation posture this PDD refines (parity per PDD replaces
  PDD005's native-leading sequencing for the PDDs that follow).
- [`wasm_runtime_layer`] — the runtime substrate the parsing surface
  is built on top of.
- [`wasm_component_layer`] — prior art consulted during design; not a
  dependency, not vendored.
- [`wit-component`] — the target-agnostic decoder this PDD's
  parsing path delegates to.
- [`wit-parser`] — the high-level WIT view `wit_component::decode`
  returns; the polyfill walks it to build its own data shapes.
- [Wasmtime] — the reference runtime whose `wasmtime::component` API
  shapes the polyfill's familiar names.
- [Explainer] — the canonical Component Model design document.
- [Subtyping] — the structural-equality rules the type system surface
  preserves; full subtyping (variance, depth, width) is async-tier
  work.
- [`thiserror`] — the derive used by the polyfill's error enum.

[PDD000]: ./PDD000%20Wasm%20Component%20Model%20Polyfill.md
[PDD001]: ./PDD001%20Development%20Environment.md
[PDD002]: ./PDD002%20Ecosystem%20Foundation.md
[pdd002-relationship-to-wasmtime]: ./PDD002%20Ecosystem%20Foundation.md#relationship-to-wasmtime
[PDD003]: ./PDD003%20Compatibility%20Outlook.md
[PDD004]: ./PDD004%20Test%20Macros.md
[PDD005]: ./PDD005%20Library%20Foundations.md
[pdd005-implementation-posture]: ./PDD005%20Library%20Foundations.md#implementation-posture
[pdd006-component-surface]: #the-component-surface
[pdd006-web-parity-per-pdd]: #web-parity-per-pdd
[`wasm_runtime_layer`]: https://github.com/DouglasDwyer/wasm_runtime_layer
[`wasm_component_layer`]: https://github.com/DouglasDwyer/wasm_component_layer
[`wit-component`]: https://docs.rs/wit-component
[`wit-parser`]: https://docs.rs/wit-parser
[`thiserror`]: https://docs.rs/thiserror
[`anyhow::Error`]: https://docs.rs/anyhow
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
[Explainer]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md
[Subtyping]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Subtyping.md
