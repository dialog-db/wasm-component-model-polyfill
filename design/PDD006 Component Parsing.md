# Component Parsing

[PDD005] established the polyfill's library foundations — `Engine`,
`Store<T>`, the single `Error` enum, and the implementation posture
under which every slice is built. This document begins the work the
foundation exists *for*: it specifies the polyfill's parsing surface,
the type-system data shapes a component's imports and exports are
described in, and the identifier model imports and exports are keyed
by. It deliberately stops short of linking, instantiation, the
canonical ABI, and resources — those are subsequent slices ([PDD007],
[PDD008], [PDD009]).

The slice is shaped by two boundaries. The first is the *synchronous
baseline*, a project-defined featureful watermark introduced below.
The second is the platform boundary: native leads, the web target
follows feature-by-feature in subsequent slices. Both come straight
from [PDD005]'s implementation posture and are preserved here without
modification. The synchronous baseline section and the native-leading
test gate convention below are written as *umbrella material* —
[PDD007], [PDD008], and [PDD009] reference back to them rather than
restating.

## Goals

- A developer can construct a `Component` from bytes through the
  polyfill's public API on the native target, and introspect its
  declared imports and exports — package name, interface identifier,
  declared valtype shape — without reaching for any upstream type.
- The four corresponding tests in
  `tests/baseline_component_binary.rs` and `tests/baseline_linking.rs`
  execute under `test:native:*` without the `#[ignore]` attribute they
  currently carry. Concretely:
  `it_parses_the_component_preamble`,
  `it_decodes_top_level_component_sections`,
  `it_rejects_a_malformed_component_binary`, and
  `it_loads_a_component_from_bytes`.
- Each of the un-stubbed tests is *target-gated* to be skipped on
  `wasm32-unknown-unknown` (see "The Native-Leading Test Gate" below).
  Web parity for any individual test is the explicit trigger for
  relaxing its gate; no test stays gated forever.
- `Component` lives in the polyfill's public API at the crate root
  (`wcmp::Component`) and wraps, rather than re-exports, any
  [`wasm_runtime_layer`] type or upstream component-layer type it
  happens to be built on top of.
- The polyfill's identifier model — `PackageName`,
  `InterfaceIdentifier`, semver constraints — is exposed through the
  polyfill's own types, so that downstream consumers never see a
  runtime-layer or upstream component-layer type even when introspecting
  a component's imports by interface name.
- The polyfill's type-system surface — enough valtype data shapes to
  describe every type listed in the synchronous baseline — is in tree
  in *data form*. Host-side values, lift/lower, and the canonical ABI
  proper are deferred to [PDD008].
- The single `wcmp::Error` enum introduced in [PDD005] grows
  additively with a parse variant.
- The path from this slice to the slices that follow is described well
  enough that the parsing surface does not have to be reshaped to
  accommodate them.

## Non-goals

- Linking, instantiation, host function registration, the canonical
  ABI, and host-resource registration. Each is the deliverable of a
  later slice in this group: [PDD007] (linking and instantiation),
  [PDD008] (canonical ABI and host functions), [PDD009] (resources).
- The web target. The same tests on `test:web:*` are explicitly out of
  scope for this slice; the target gate on each un-stubbed test is the
  mechanism by which that exclusion is made testable. Web parity
  arrives feature-by-feature in subsequent slices.
- Anything outside the synchronous baseline as defined below. That
  includes every async-tier concern enumerated in [PDD003]'s checklist
  and every wasip3-specific extension to the type system, ABI, and
  runtime substrate.
- Component-level `start` functions, host-binding code generation
  (a `wit-bindgen!` equivalent), and value imports / value exports —
  each tracked separately on [PDD003]'s checklist and out of scope for
  the entire synchronous-baseline group.
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
[PDD005]'s foundational types. It is the shape every slice in this
group of the polyfill's roadmap measures itself against; when this
document or [PDD007], [PDD008], or [PDD009] says "the synchronous
baseline," it means the list below.

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
  `LinkerInstance`, and `Instance` types this group of slices
  introduces, exposed as the polyfill's own surface.

The synchronous baseline explicitly excludes — these are the *async
tier* of the polyfill's roadmap, taken up in subsequent slices:

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
If a future slice needs to revise the boundary, it does so by amending
this section in a successor PDD rather than by silently widening or
narrowing the term in passing.

## The Component Surface

`Component` is the polyfill's parsed-component value, and the only
public component-layer type this slice introduces. It is constructed
from an engine and a byte slice via `Component::new(&engine, bytes)`,
mirroring [Wasmtime]'s `wasmtime::component::Component::new`. The byte
slice is borrowed only for the duration of the call; the parsed
representation is the polyfill's. A `Component` exposes its declared
imports and exports through accessors on the value itself, so that
test code (and downstream tooling) can introspect structural shape
without reaching for an upstream type. Malformed binaries — corrupted
preamble, truncated section, unknown section tag at a fatal position —
surface as a structured `wcmp::Error` variant rather than a panic.

The types `Component` exposes through its introspection accessors are
the polyfill's own — the type-system data shapes and identifier types
introduced below — so that a test or tool walking a component's
imports never sees an upstream type. Where access to the runtime
layer is needed within the polyfill crate to back the parser, it is
reached through crate-private accessors as established in [PDD005].

## The Type System Surface

The slice introduces enough of the polyfill's own type-system surface
to describe every type listed in the synchronous baseline. It does so
in *data form* only: a `ValueType` (or comparably-named) family of
shapes that captures the structural identity of every valtype, usable
as the result of `Component`'s import/export introspection accessors.
Host-side values (a `Val` family that carries data through host
function calls), lift/lower, and the canonical ABI proper are deferred
to [PDD008].

Structural type equality is preserved at this slice's level: two
identically-shaped, separately-defined record types unify, per
[Subtyping]. Subtyping proper (variance, depth, width) is async-tier
work and is out of scope here.

`own<T>` and `borrow<T>` appear in the type-system data shapes as
slots whose payload type identity is settled by [PDD009]; the
introspection surface this slice introduces is sufficient for parsing
and reporting, not for handle-table behaviour.

## The Identifier Model

The polyfill exposes its own identifier types — `PackageName` and
`InterfaceIdentifier`, with optional semver constraints attached — as
the keys imports and exports are addressed by. Their shapes mirror the
upstream component-layer model so the mental mapping is clear, but no
upstream type is exposed to consumers. [PDD007] uses these data types
as the addressing surface for its linker types and adds the
resolution logic (semver matching, selecting between candidate
registrations); this slice introduces them as data only, exposed
through `Component`'s import/export accessors.

## Error Model Growth

`wcmp::Error` grows additively. The variant this slice introduces:

- A *parse* variant for failures decoding a component binary
  (corrupted preamble, truncated section, unsupported encoding inside
  the synchronous baseline). The underlying parser cause is captured
  as `#[source]`.

[PDD007], [PDD008], and [PDD009] add the link, instantiation,
type-mismatch, and ABI variants their own scopes need.

[PDD005]'s note about [`anyhow::Error`] in `#[source]` fields applies
unchanged. Public functions continue to return the crate-level
`Result<T>` alias.

## The Native-Leading Test Gate

The `#[ignore]` attribute the baseline tests carry today says "this
test is a stub; the polyfill cannot pass it yet." That message stops
being accurate the moment a slice lands the underlying feature on
native — but the same test, run against the wasm32 target, *is* still
expected to fail because the web backend has not yet caught up.
`#[ignore]` cannot distinguish those two states.

This slice (and every slice that follows the same native-leading
discipline) replaces `#[ignore]` on a test it un-stubs with a target
gate that *conditionally* applies `#[ignore]` on the wasm32 target
only. The mechanism is a single per-test annotation:

```rust
#[cfg_attr(target_arch = "wasm32", ignore = "web parity pending")]
```

On native this attribute is absent; the test runs. On wasm32 it
expands to `#[ignore]` with a message naming the missing-feature
reason; the test is registered with the test binary but skipped at
runtime, where it shows up in `test:web:*` output as a clearly-
labelled deferred test rather than a hidden absence. The convention
trades a stricter "absent from the wasm32 test binary entirely"
reading for a softer one — *present but conditionally ignored* —
that requires no macro work and reads identically to a regular
`#[ignore]` for any contributor familiar with Cargo.

The convention, expressed this way, is that:

- A test gated this way is green on `test:native:*` and skipped with a
  "web parity pending" message on `test:web:*`. The wasm32 build does
  not regress, and the test output for the wasm32 build is honest
  about what is deferred and why.
- Removing the gate is the *acceptance criterion* for the slice that
  brings the corresponding feature to web. When a future web-parity
  slice claims to land "host function registration on the browser
  backend," that slice's PR is the one that drops the
  `#[cfg_attr(target_arch = "wasm32", ignore = …)]` from
  `it_defines_a_typed_host_function` (and any sibling tests it
  legitimately makes green on web).
- A test never *acquires* a target gate as part of feature work; the
  gate is a transitional state on the way from `#[ignore]` (stub) to
  unconditional (running on every supported target). If a slice cannot
  remove a gate on a test it touches, it has not finished that test's
  feature on web, and the gate stays.

This convention is the artifact-level expression of [PDD005]'s
implementation posture: native leads every slice; web parity follows
once a feature's semantics are settled on native. [PDD007], [PDD008],
and [PDD009] inherit this convention without restating it.

## Implementation Posture

This slice is governed by [PDD005]'s implementation posture without
modification. Three points are worth restating because they bear on
the acceptance criteria:

The polyfill's component-layer code is original work.
[`wasm_component_layer`] is consulted as prior art — its data
structures, traversal patterns, and parse implementation choices are
valuable references — but it is not taken on as a dependency,
vendored, or copied verbatim. Where a design choice is taken from
upstream, the polyfill notes the prior-art origin in an in-tree
comment so a future reader can cross-reference upstream for context.

Native leads. The web target is intentionally not attempted in this
slice. The acceptance criteria above are met when the named tests pass
on `test:native:*` and are gated out of `test:web:*`; they are not
weakened by the absence of those tests on the wasm32 build, and they
are not strengthened by claiming partial behaviour on web.

The slice is purely additive at the public API. The foundational
types ([PDD005]'s `Engine`, `Store<T>`, `Error`, `Result<T>`) are not
reshaped; `Component`, the type-system data shapes, and the identifier
types join them at the crate root, and a parse variant extends the
existing error enum. A reviewer checking scope against [PDD005]'s
"Implementation Posture" section should find no exception to the
slicing discipline here.

## User Stories

**As a developer adopting the polyfill on a native host**, I want to
load a known-good component and read its declared imports and exports
in a few lines of code, so that I can integrate the polyfill into my
build pipeline before the host-side work to instantiate or call into
it has landed.

> The developer parses a known-good component with
> `wcmp::Component::new(&engine, &bytes)`, walks its declared imports
> and exports through accessors on the value, and matches against the
> polyfill's `PackageName` and `InterfaceIdentifier` types. They do
> not reach for `wasm_runtime_layer`, `wasmtime`, or any upstream
> type; the polyfill's API is enough for the parsing-only surface.

**As a contributor opening [PDD007]**, I want the parsing surface
already in tree and stable, with `Component`, the type-system data
shapes, and the identifier model exposed through the polyfill's own
types, so that my slice is purely additive and I am not relitigating
parser design while I work on linking and instantiation.

> The contributor reads this document, sees the path-to-next-slice
> section, and writes their `Linker<T>` and `Instance` against the
> existing `Component` without touching it. The error variants they
> need are added to the existing `wcmp::Error` enum.

**As a reviewer evaluating an in-progress slice**, I want the scope of
each slice to be as readable as [PDD005]'s was, so that "is this PR
doing too much" continues to have a documented answer.

> The reviewer reads the goals and non-goals, confirms that the PR
> un-stubs only the named tests on native, that no async-tier surface
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
  this slice's tests are exercised through.
- [PDD002] — the polyfill's relationship to [`wasm_runtime_layer`] and
  [`wasm_component_layer`]. This document inherits the "prior art
  only, no dependency, no vendored source" interpretation [PDD005]
  bound it to.
- [PDD003] — the compatibility outlook and implementation checklist.
  This slice covers the parsing rows of "Component Binary Format" and
  the data-shape rows of "Component Type System".
- [PDD004] — the test macros every baseline test in this slice is
  written against; the target gate this document introduces composes
  with the cross-target test attribute defined there.
- [PDD005] — the foundational `Engine`, `Store`, `Error`, and the
  implementation posture this slice extends without modification.
- [PDD007] — the next slice in this group: linking and instantiation.
- [PDD008] — canonical ABI and host functions.
- [PDD009] — resources.
- [`wasm_runtime_layer`] — the runtime substrate the parsing surface
  is built on top of.
- [`wasm_component_layer`] — prior art consulted during design; not a
  dependency, not vendored.
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
[PDD003]: ./PDD003%20Compatibility%20Outlook.md
[PDD004]: ./PDD004%20Test%20Macros.md
[PDD005]: ./PDD005%20Library%20Foundations.md
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
