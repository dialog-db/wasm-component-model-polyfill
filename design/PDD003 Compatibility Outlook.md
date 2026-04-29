# Compatibility Outlook

The Wasm Component Model is, today, a moving target without a single canonical
specification. Its semantics are scattered across a collection of design
documents in the [`WebAssembly/component-model`] repository, the working code
of the reference runtime ([Wasmtime]), the WIT files of the [WASI 0.3
(wasip3)][wasip3] worlds, and a long tail of blog posts and issue threads.
There is no formal specification for "wasip3 the Component Model"; there is
only the union of these sources at any given moment.

This document maps that union onto the polyfill. It establishes Wasm Core (in
the form available to evergreen browsers and to [Wasmtime]) as the *baseline*
the polyfill builds on, takes [`wasm_component_layer`] as the *current state*
the polyfill inherits from [PDD002], and treats [wasip3] as the *target* the
polyfill must reach. The bulk of the document is a structured, itemised
inventory of the gap between the second and the third — the discrete
features the polyfill must implement to host wasip3 components on top of a
plain Wasm Core engine.

## Goals

- Establish a shared, source-grounded picture of where the Component Model
  ends and where Wasm Core begins, so that the polyfill's scope is unambiguous.
- Enumerate the discrete Component Model features the polyfill must implement
  to reach wasip3, organised so that the list can serve as an incremental
  implementation checklist.
- Anchor each feature to a canonical reference URL — the design document,
  Wasmtime API page, or roadmap entry that defines its semantics — so that
  contributors can move from "what" to "how" without re-doing the research
  this document is built on.
- Record the implementation status of each feature in [Wasmtime] alongside the
  status in [`wasm_component_layer`], so that the polyfill can lean on the
  reference implementation when local work begins.
- Distinguish features that are *settled* in upstream sources from those that
  are *in flux*, so that contributors know where to expect churn.

## Non-goals

- This document does not specify per-WASI-world bindings (e.g. the
  `wasi:http/types` interface). Those are downstream of the polyfill: a WASI
  world is a *consumer* of the Component Model machinery, linked into a
  `Linker` like any other component import. Specific worlds are out of
  scope here.
- This document does not implement, polyfill, or otherwise compensate for
  Wasm Core proposals (GC, threads, exception handling, stack switching, …).
  Those are the host engine's responsibility. The inventory only notes which
  Core proposals are observably available, because their availability shapes
  implementation strategy for adjacent Component Model features.
- This document does not propose a sequencing for the implementation
  checklist beyond noting features that are foundational vs. those that depend
  on them. Engineering planning will choose the order.
- This document does not commit the polyfill to features that the upstream
  Component Model design has retreated from. Where such features remain on
  the project's roadmap, they are explicitly marked as deferred.

## Sources of Truth

The Component Model is documented in several places, none of which are
individually complete. The polyfill treats the following as canonical, in
descending order of authority:

1. The design documents in [`WebAssembly/component-model`], particularly
   [`design/mvp/Explainer.md`], [`design/mvp/Binary.md`],
   [`design/mvp/CanonicalABI.md`], [`design/mvp/Async.md`],
   [`design/mvp/Concurrency.md`], and [`design/mvp/Subtyping.md`]. The Python
   reference implementation in
   [`design/mvp/canonical-abi/definitions.py`] is executable and resolves
   ambiguity that the prose does not.
2. The [Wasmtime] reference implementation, particularly the
   `wasmtime::component` API and the `wasmtime_wasi::p3` bindings. Where the
   design documents are silent or contradictory, Wasmtime's behaviour is the
   tiebreaker.
3. The [WASI Roadmap] and the per-package WIT files at `wit-0.3.0-draft/`
   paths in each `WebAssembly/wasi-*` repository, which define the
   type-system features the polyfill must support to host wasip3 worlds.
4. Public writing from contributors and the Bytecode Alliance — the
   ["Looking Ahead to WASIp3"] post, ["Thinking about streams in WASI"], and
   the [`wasip3-prototyping`] repository — which provide rationale and worked
   examples that the normative sources omit.

The polyfill's reading of these sources is captured in the inventory below.
When upstream sources change, the inventory is the artifact that should be
revisited.

## The Wasm Core Baseline

The polyfill assumes a Wasm Core host that ships the Wasm 2.0 specification
and the post-2.0 proposals that have reached universal browser availability
as of this document's writing. That baseline is, concretely:

| Capability                                                                                | Browsers                                    | Wasmtime            |
| ----------------------------------------------------------------------------------------- | ------------------------------------------- | ------------------- |
| Wasm 2.0 (multi-value, ref types, bulk memory, sign-ext, non-trapping float-to-int, SIMD) | Universal                                   | Yes                 |
| Garbage Collection                                                                        | Universal                                   | Yes                 |
| Threads + atomics                                                                         | Universal                                   | Yes                 |
| Tail calls                                                                                | Universal                                   | Yes                 |
| Exception handling (`exnref`)                                                             | Universal                                   | Yes                 |
| Multi-memory                                                                              | Universal                                   | Yes                 |
| Memory64                                                                                  | Universal                                   | Yes                 |
| Relaxed SIMD                                                                              | Universal                                   | Yes                 |
| Type reflection (JS API)                                                                  | Universal                                   | n/a                 |
| JS Promise Integration ([JSPI])                                                           | Chrome / Firefox stable; Safari behind flag | n/a                 |
| Stack switching (fibers, `cont`/`resume`)                                                 | Not shipped                                 | Experimental opt-in |

Two Core proposals materially affect Component Model implementation strategy.
[JSPI] is the polyfill's only sanctioned bridge for stackful async lift in a
browser host, and stack switching's absence in the browser is the reason the
polyfill cannot offer first-class stackful lifts on every platform. These
constraints are noted where they apply; the polyfill itself does not
implement Core proposals under any circumstance.

The polyfill also assumes the standard `WebAssembly.*` JavaScript API for
module loading, validation, instantiation, memory, tables, globals, and tags.
Resources, futures, and streams are represented inside the polyfill on top of
this surface; they are not exposed to host JavaScript directly.

## The Component Model Compatibility Matrix

The remainder of this section is the inventory itself, organised by
Component Model subsystem. Each row carries:

- the *current state* in [`wasm_component_layer`] (✅ implemented, ⚠️ partial
  or known-buggy, ❌ absent);
- the *wasip3 target*, expressed as the work the polyfill must do;
- the *Wasmtime status* (✅ implemented, ⚠️ experimental opt-in, ❌ removed
  or out-of-scope upstream); and
- a canonical reference.

Cross-reference [PDD002] for the polyfill's relationship to
[`wasm_component_layer`]: features marked ❌ in the current-state column are
the polyfill's responsibility to grow into wasip3, either by extending the
upstream crate or by carrying local divergence where upstreaming is
impractical.

### Component Binary Format

| Concern                                                                                                                                       | Current state                                                          | Polyfill target                    | Wasmtime                                   | Reference                        |
| --------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------- | ---------------------------------- | ------------------------------------------ | -------------------------------- |
| Component preamble & section parsing                                                                                                          | ✅ via wasip2-era `wit-parser`/`wit-component`/`wasmtime-environ` pins | Re-target to wasip3-aware versions | ✅                                         | [Binary – component definitions] |
| Type-encoding bytes `0x64` (`error-context`), `0x65` (`future`), `0x66` (`stream`)                                                            | ❌                                                                     | Implement                          | ⚠️ exp-opt-in                              | [Binary – type definitions]      |
| `canon` builtin opcodes for the wasip3 set (task / subtask / backpressure / context / yield / stream / future / error-context / waitable-set) | ❌                                                                     | Implement parsing and dispatch     | ⚠️ exp-opt-in (`-W component-model-async`) | [Binary – canonical definitions] |

Exact byte assignments for the `waitable-set.*` and `waitable.join` builtins
are still settling in spec drafts as of this writing; the polyfill should
expect minor churn there.

### Component Type System

| Concern                                                                      | Current state | Polyfill target | Wasmtime      | Reference                             |
| ---------------------------------------------------------------------------- | ------------- | --------------- | ------------- | ------------------------------------- |
| Primitives, records, variants, lists, options, results, tuples, flags, enums | ✅            | Preserve        | ✅            | [Explainer – type definitions]        |
| Structural type equality                                                     | ✅            | Preserve        | ✅            | [Explainer]                           |
| Subtyping (variance, depth, width)                                           | ❌            | Implement       | ✅            | [Subtyping]                           |
| `own<T>` / `borrow<T>` resources, sync destructors                           | ✅            | Preserve        | ✅            | [Explainer – resources]               |
| Async resource destructors                                                   | ❌            | Implement       | ⚠️ exp-opt-in | [Async – async resource destructors]  |
| Cross-component resource handle transfer (transfer trampolines)              | ❌            | Implement       | ✅            | [CanonicalABI – `canon resource.new`] |
| `future<T>` as first-class valtype                                           | ❌            | Implement       | ⚠️ exp-opt-in | [Async – streams and futures]         |
| `stream<T>` as first-class valtype                                           | ❌            | Implement       | ⚠️ exp-opt-in | [Async – streams and futures]         |
| `error-context` as first-class valtype                                       | ❌            | Implement       | ⚠️ exp-opt-in | [Async – error-context]               |
| Function-type `async?` bit                                                   | ❌            | Implement       | ⚠️ exp-opt-in | [Explainer – component definitions]   |

### Canonical ABI

| Concern                                                  | Current state     | Polyfill target                                                                           | Wasmtime      | Reference                            |
| -------------------------------------------------------- | ----------------- | ----------------------------------------------------------------------------------------- | ------------- | ------------------------------------ |
| Lift/lower for all valtypes; `cabi_realloc` hook         | ✅                | Preserve                                                                                  | ✅            | [CanonicalABI]                       |
| Specialized list lift/lower fast paths                   | ✅                | Preserve                                                                                  | ✅            | [CanonicalABI – flat representation] |
| `post-return` (sync lifts only after wasip3)             | ⚠️ implicit       | Surface as a first-class option, restricted to sync lifts                                 | ✅            | [CanonicalABI – `canon lift`]        |
| String transcoders (UTF-8 ↔ UTF-16 ↔ Latin1+UTF-16)      | ❌                | Implement                                                                                 | ✅            | [CanonicalABI – storing]             |
| Async `canon lift` (callback mode)                       | ❌                | Implement                                                                                 | ⚠️ exp-opt-in | [CanonicalABI – `canon lift`]        |
| Async `canon lift` (stackful mode)                       | ❌                | Best-effort: [JSPI]-backed in browsers, native runtime where stack switching is available | ⚠️ exp-opt-in | [Async – stackful lift]              |
| Async `canon lower`                                      | ❌                | Implement                                                                                 | ⚠️ exp-opt-in | [CanonicalABI – `canon lower`]       |
| Per-task lift/lower context threading                    | ❌                | Implement                                                                                 | ⚠️ exp-opt-in | [Async – tasks]                      |
| Generalized handle table (futures + streams + resources) | ⚠️ resources only | Extend                                                                                    | ✅            | [CanonicalABI – runtime state]       |

### Async Runtime Substrate

| Concern                                                                                 | Current state | Polyfill target                                                                        | Wasmtime      | Reference                                 |
| --------------------------------------------------------------------------------------- | ------------- | -------------------------------------------------------------------------------------- | ------------- | ----------------------------------------- |
| Cooperative scheduler / event loop                                                      | ❌            | Implement: JS Promise integration in the browser; runtime-agnostic executor for native | ⚠️ exp-opt-in | [Wasmtime `Accessor`]                     |
| Task lifecycle (created → running → returned → dropped)                                 | ❌            | Implement                                                                              | ⚠️ exp-opt-in | [Async – task lifecycle]                  |
| `task.return`                                                                           | ❌            | Implement                                                                              | ⚠️ exp-opt-in | [CanonicalABI – `canon task.return`]      |
| Backpressure (`backpressure.set`, `backpressure.inc`, `backpressure.dec`)               | ❌            | Implement                                                                              | ⚠️ exp-opt-in | [CanonicalABI – `canon backpressure.set`] |
| Cancellation (`task.cancel`, `subtask.cancel`, `cancellable` waits/polls)               | ❌            | Implement                                                                              | ⚠️ exp-opt-in | [Async – cancellation]                    |
| Context-locals (`context.get`, `context.set`; i32 and i64 variants; fixed-length 2 i32) | ❌            | Implement                                                                              | ⚠️ exp-opt-in | [CanonicalABI – `canon context.get`]      |
| Structured concurrency (subtask/supertask edge)                                         | ❌            | Implement                                                                              | ⚠️ exp-opt-in | [Async – structured concurrency]          |
| `yield`                                                                                 | ❌            | Implement                                                                              | ⚠️ exp-opt-in | [CanonicalABI – `canon yield`]            |
| Waitable sets (`waitable-set.{new,add,remove,wait,poll,drop}`, `waitable.join`)         | ❌            | Implement                                                                              | ⚠️ exp-opt-in | [Async – waitable sets]                   |
| Event encoding for waits, polls, and callback status words                              | ❌            | Implement                                                                              | ⚠️ exp-opt-in | [Async – events and callbacks]            |

### Future, Stream, and Error-Context Lifecycles

| Concern                                                                        | Current state | Polyfill target    | Wasmtime      | Reference                                  |
| ------------------------------------------------------------------------------ | ------------- | ------------------ | ------------- | ------------------------------------------ |
| `future.{new,read,write,cancel-read,cancel-write,drop-readable,drop-writable}` | ❌            | Implement          | ⚠️ exp-opt-in | [CanonicalABI – `canon future.new`]        |
| `stream.{new,read,write,cancel-read,cancel-write,drop-readable,drop-writable}` | ❌            | Implement          | ⚠️ exp-opt-in | [CanonicalABI – `canon stream.new`]        |
| `error-context.{new,debug-message,drop}`                                       | ❌            | Implement          | ⚠️ exp-opt-in | [CanonicalABI – `canon error-context.new`] |
| End-drop and cancellation ordering edge cases (cf. CVE-2026-27195)             | ❌            | Add to test corpus | n/a           | [Async – streams and futures]              |

### Linking, Instantiation, and Host Integration

| Concern                                                                             | Current state                                       | Polyfill target                                                                                                                                             | Wasmtime            | Reference                                |
| ----------------------------------------------------------------------------------- | --------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------- | ---------------------------------------- |
| `Engine` / `Store` / `Module` / `Instance` (Core layer, via [`wasm_runtime_layer`]) | ✅ on 0.7                                           | Track upstream                                                                                                                                              | ✅                  | [`wasmtime::component::Linker`]          |
| `Component` / `Linker` / `LinkerInstance` / `Instance` (component layer)            | ✅ multi-instance, `define_func`, `define_resource` | Preserve and extend for async                                                                                                                               | ✅                  | [`wasmtime::component::LinkerInstance`]  |
| Identifier model (`PackageName`, `InterfaceIdentifier`, semver)                     | ✅                                                  | Preserve                                                                                                                                                    | ✅                  | [Explainer]                              |
| Host function definition (sync)                                                     | ✅ untyped and typed                                | Preserve                                                                                                                                                    | ✅                  | [`LinkerInstance::func_wrap`]            |
| Host function definition (async)                                                    | ❌                                                  | Implement                                                                                                                                                   | ⚠️ exp-opt-in       | [`LinkerInstance::func_wrap_concurrent`] |
| Host resource definition with sync destructors                                      | ✅                                                  | Preserve                                                                                                                                                    | ✅                  | [`wasmtime::component::ResourceType`]    |
| Host resource definition with async destructors                                     | ❌                                                  | Implement                                                                                                                                                   | ⚠️ exp-opt-in       | [`wasmtime::component::Accessor`]        |
| Host-binding code generation (`wit-bindgen!` equivalent)                            | ❌                                                  | Implement                                                                                                                                                   | ✅                  | [`wasmtime::component::bindgen!`]        |
| Component-level `start` function                                                    | ❌                                                  | Implement                                                                                                                                                   | ✅                  | [Explainer – start definitions]          |
| Value imports / value exports                                                       | ❌                                                  | **Deferred.** Tracked on the roadmap; implementation deferred until [Wasmtime] resumes work. The Component Model MVP currently treats this as out-of-scope. | ❌ removed from MVP | [Explainer – component definitions]      |
| WIT `@since` / `@unstable` feature gate handling                                    | ❌                                                  | Tolerate and gate appropriately                                                                                                                             | ✅                  | [WIT Feature Gates]                      |

## What This Document Does Not Commit To

A few items that surfaced during research are explicitly *not* part of the
checklist:

- WASI worlds at the per-function level (`wasi:cli`, `wasi:http`,
  `wasi:clocks`, `wasi:filesystem`, `wasi:sockets`, `wasi:random`). They are
  consumers of the polyfill, not part of it. The features they exercise —
  resources, async, `stream<T>`, `future<T>`, `error-context`, and the full
  set of value types — are all covered above. (`wasi:io`, present in
  wasip2, is retired in wasip3; its responsibilities are absorbed by the
  built-in `stream<T>` and `future<T>` types.)
- Wasm Core proposals not shipped in browsers, including stack switching.
  The polyfill works without them and uses [JSPI] as the available bridge
  where it must.
- Shared-everything threading hooks (`thread.spawn`, `thread.yield-to`).
  These are tracked in [`design/mvp/Async.md`] but explicitly post-0.3.
- A generic payloaded `error<P>` type ([component-model issue 389]). Tracked
  upstream as future work; not part of wasip3.

These are not gaps in the polyfill; they are bounds on the polyfill's scope.

## User Stories

**As a polyfill contributor approaching the project for the first time**, I
want a single document that tells me what is left to build, so that I can
pick up a discrete piece of work without first having to rebuild the
research tree the project sits on.

> The contributor reads this document and lands on the Implementation
> Checklist. They pick item 29 — `future` lifecycle ABI — follow the
> [CanonicalABI – `canon future.new`] reference, cross-check against
> [Wasmtime]'s implementation, and start work knowing the scope of their
> change.

**As a polyfill contributor evaluating a bug report**, I want to know
whether the misbehaving feature is supposed to work yet, so that I can tell
"missing feature" from "broken feature" before I start debugging.

> A user reports that calling an async export hangs. The contributor checks
> the matrix, sees that async `canon lift` is on the checklist but not yet
> implemented, and replies that the feature is in the roadmap rather than
> opening a defect against existing code.

**As a polyfill maintainer responding to upstream churn**, I want every
feature on the checklist to point back to the canonical source that defined
it, so that when the source changes I can see immediately whether the
polyfill's plan still matches.

> The Component Model design repository merges a change to the
> waitable-set opcode encoding. The maintainer follows the reference link
> from row 27 of the matrix, sees the diff, and updates the checklist note
> for that item without having to re-derive the surrounding context.

**As a developer adopting the polyfill in a real application**, I want a
clear statement of what wasip3 features are supported today, so that I can
plan my application around what works rather than discovering gaps at
runtime.

> The developer reviews the matrix's "Current state" column and the open
> items on the Implementation Checklist. They observe that synchronous
> components and resources work today, while async functions, streams, and
> futures are still in progress, and they design their initial integration
> around the synchronous surface accordingly.

## References

- [PDD000] — the polyfill's product overview and the reason wasip3 is the
  target.
- [PDD001] — the development environment and multi-target test substrate
  this document presupposes.
- [PDD002] — the polyfill's relationship to [`wasm_component_layer`] and
  [`wasm_runtime_layer`].
- [`WebAssembly/component-model`] — the canonical design documents for the
  Component Model, including [`design/mvp/Explainer.md`],
  [`design/mvp/Binary.md`], [`design/mvp/CanonicalABI.md`],
  [`design/mvp/Async.md`], [`design/mvp/Concurrency.md`],
  [`design/mvp/Subtyping.md`], and the executable reference at
  [`design/mvp/canonical-abi/definitions.py`].
- [WASI Roadmap] — the project's own description of the WASI 0.3.0
  (wasip3) release the polyfill targets.
- [Wasmtime] — the reference runtime and the implementation status
  tiebreaker.
- ["Looking Ahead to WASIp3"] — Fermyon's overview of the wasip3 changes.
- ["Thinking about streams in WASI"] — design rationale for the stream /
  future model that replaces `wasi:io` in wasip3.
- [`wasip3-prototyping`] — the upstream testbed for wasip3 host bindings.
- [JSPI] — the JS Promise Integration proposal; the bridge between the
  Component Model's async surface and a browser host that lacks stack
  switching.
- [WIT Feature Gates] — the upstream specification for `@since` and
  `@unstable` annotations the polyfill must tolerate.
- [component-model issue 389] — the open spec issue tracking generic
  payloaded `error<P>`, noted as out-of-scope here.

[PDD000]: ./PDD000%20Wasm%20Component%20Model%20Polyfill.md
[PDD001]: ./PDD001%20Development%20Environment.md
[PDD002]: ./PDD002%20Ecosystem%20Foundation.md

[`WebAssembly/component-model`]: https://github.com/WebAssembly/component-model
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
[`wasm_component_layer`]: https://github.com/DouglasDwyer/wasm_component_layer
[`wasm_runtime_layer`]: https://github.com/DouglasDwyer/wasm_runtime_layer
[wasip3]: https://wasi.dev/roadmap
[WASI Roadmap]: https://wasi.dev/roadmap
["Looking Ahead to WASIp3"]: https://www.fermyon.com/blog/looking-ahead-to-wasip3
["Thinking about streams in WASI"]: https://blog.sunfishcode.online/preview3-streams
[`wasip3-prototyping`]: https://github.com/bytecodealliance/wasip3-prototyping
[JSPI]: https://github.com/WebAssembly/js-promise-integration
[component-model issue 389]: https://github.com/WebAssembly/component-model/issues/389

[`design/mvp/Explainer.md`]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md
[`design/mvp/Binary.md`]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Binary.md
[`design/mvp/CanonicalABI.md`]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md
[`design/mvp/Async.md`]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Async.md
[`design/mvp/Concurrency.md`]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md
[`design/mvp/Subtyping.md`]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Subtyping.md
[`design/mvp/canonical-abi/definitions.py`]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/canonical-abi/definitions.py

[Explainer]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md
[Explainer – type definitions]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md#type-definitions
[Explainer – resources]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md#resources
[Explainer – component definitions]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md#component-definitions
[Explainer – start definitions]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md#start-definitions

[Binary – component definitions]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Binary.md#component-definitions
[Binary – type definitions]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Binary.md#type-definitions
[Binary – canonical definitions]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Binary.md#canonical-definitions

[CanonicalABI]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md
[CanonicalABI – flat representation]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#flat-representation
[CanonicalABI – storing]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#storing
[CanonicalABI – runtime state]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#runtime-state
[CanonicalABI – `canon lift`]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#canon-lift
[CanonicalABI – `canon lower`]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#canon-lower
[CanonicalABI – `canon resource.new`]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#canon-resourcenew
[CanonicalABI – `canon task.return`]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#canon-taskreturn
[CanonicalABI – `canon backpressure.set`]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#canon-backpressureset
[CanonicalABI – `canon context.get`]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#canon-contextget
[CanonicalABI – `canon yield`]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#canon-yield
[CanonicalABI – `canon future.new`]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#canon-futurenew
[CanonicalABI – `canon stream.new`]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#canon-streamnew
[CanonicalABI – `canon error-context.new`]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#canon-error-contextnew

[Async – streams and futures]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Async.md#streams-and-futures
[Async – error-context]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Async.md#error-context
[Async – async resource destructors]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Async.md#async-resource-destructors
[Async – stackful lift]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Async.md#stackful-lift
[Async – tasks]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Async.md#tasks
[Async – task lifecycle]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Async.md#task-lifecycle
[Async – cancellation]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Async.md#cancellation
[Async – structured concurrency]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Async.md#structured-concurrency
[Async – waitable sets]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Async.md#waitable-sets
[Async – events and callbacks]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Async.md#events-and-callbacks

[Subtyping]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Subtyping.md
[WIT Feature Gates]: https://github.com/WebAssembly/component-model/blob/main/design/wit/Feature-Gates.md

[`wasmtime::component::Linker`]: https://docs.wasmtime.dev/api/wasmtime/component/struct.Linker.html
[`wasmtime::component::LinkerInstance`]: https://docs.wasmtime.dev/api/wasmtime/component/struct.LinkerInstance.html
[`LinkerInstance::func_wrap`]: https://docs.wasmtime.dev/api/wasmtime/component/struct.LinkerInstance.html#method.func_wrap
[`LinkerInstance::func_wrap_concurrent`]: https://docs.wasmtime.dev/api/wasmtime/component/struct.LinkerInstance.html#method.func_wrap_concurrent
[`wasmtime::component::ResourceType`]: https://docs.wasmtime.dev/api/wasmtime/component/struct.ResourceType.html
[`wasmtime::component::Accessor`]: https://docs.wasmtime.dev/api/wasmtime/component/struct.Accessor.html
[Wasmtime `Accessor`]: https://docs.wasmtime.dev/api/wasmtime/component/struct.Accessor.html
[`wasmtime::component::bindgen!`]: https://docs.wasmtime.dev/api/wasmtime/component/macro.bindgen.html
