# Compatibility Outlook

The Component Model has no single canonical specification. Its semantics are
spread across the design documents in the [`WebAssembly/component-model`]
repository, the code of the reference runtime ([Wasmtime]), the WIT files of
the [WASI 0.3] worlds, and public writing by the contributors. The union of
those sources at a point in time is the target.

This document maps that union onto the polyfill. It takes Wasm Core, as
browsers and Wasmtime ship it, as the baseline. It takes the Component Model
as released with WASI 0.3 as the target. The body is an inventory of the
features between the two: the work the polyfill must do to host Component
Model 0.3 components on a plain Wasm Core engine.

## Goals

- Give a source-grounded picture of where the Component Model ends and Wasm
  Core begins, so that the scope of the polyfill is unambiguous.
- List the discrete Component Model features the polyfill must implement,
  organized as an incremental checklist.
- Anchor each feature to a canonical reference, so that a contributor can go
  from "what" to "how" without repeating the research.
- Record the status of each feature in Wasmtime, so that the polyfill can
  lean on the reference implementation.
- Distinguish features that are settled from features that are in flux, so
  that contributors know where to expect churn.

## Non-goals

- Per-world WASI bindings, for example `wasi:http/types`. A WASI world is a
  consumer of the polyfill and links into a `Linker` like any other import.
- Wasm Core proposals (garbage collection, threads, exception handling, stack
  switching). Those belong to the host engine. The inventory only notes which
  proposals are available, because availability shapes strategy.
- A sequence for the checklist. Engineering planning chooses the order.
- Features that the upstream design retreated from. Where such a feature
  stays on the roadmap, the inventory marks it as deferred.

## Sources of Truth

The polyfill treats these sources as canonical, in descending order of
authority:

1. The design documents in [`WebAssembly/component-model`]: the
   [Explainer], the [Binary format][Binary], the [Canonical ABI][CanonicalABI],
   and the [Concurrency explainer][Concurrency]. The Python reference in
   [`definitions.py`] is executable and resolves ambiguity that the prose
   leaves open. The Explainer marks each gated feature with an emoji. A
   feature marked 🔀 (concurrency), 🗺️ (`map`), or 🏷️ (annotations)
   shipped. A feature marked 📝 (`error-context`), 🪙 (value imports and
   `start`), 🚝 (more ABI options on async built-ins), or 🧵 (threads) is in
   development.
2. [Wasmtime], in particular `wasmtime::component` and `wasmtime-wasi`.
   Wasmtime 46 and later enable the WASI 0.3 features by default. Where the
   design documents are silent, Wasmtime's behavior decides.
3. The [WASI 0.3 release notes][WASI 0.3] and the [WASI roadmap], which
   define the release train and the WIT packages a 0.3 world imports.
4. The [Component Model 1.0 roadmap], which names the changes after 0.3.
5. The conformance corpora: the [Component Model test corpus] and the
   [Wasmtime component tests]. Both are `.wast` suites that a runtime can
   execute directly.

When an upstream source changes, this inventory is the document to revisit.

## The Wasm Core Baseline

The polyfill assumes a Wasm Core host that ships the Wasm 2.0 specification
and the post-2.0 proposals that every evergreen browser ships. That baseline
is:

| Capability                                                                                          | Browsers                                                            | Wasmtime       |
| --------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------- | -------------- |
| Wasm 2.0 (multi-value, reference types, bulk memory, sign extension, saturating float-to-int, SIMD) | All                                                                 | Yes            |
| Garbage collection                                                                                  | All                                                                 | Yes            |
| Threads and atomics                                                                                 | All                                                                 | Yes            |
| Tail calls                                                                                          | All                                                                 | Yes            |
| Exception handling (`exnref`)                                                                       | All                                                                 | Yes            |
| Multiple memories                                                                                   | All                                                                 | Yes            |
| Memory64                                                                                            | All                                                                 | Yes            |
| Relaxed SIMD                                                                                        | All                                                                 | Yes            |
| Type reflection (JavaScript API)                                                                    | All                                                                 | Not applicable |
| JavaScript Promise Integration ([JSPI])                                                             | Chrome and Edge shipped. Firefox behind a flag. Safari in progress. | Not applicable |
| Stack switching (`cont`, `resume`)                                                                  | Not shipped                                                         | Experimental   |

Two rows shape the strategy. [JSPI] is the only bridge for a stackful
asynchronous lift in a browser. Stack switching is absent in browsers, so
the polyfill cannot offer a stackful lift on every platform without JSPI.
The polyfill does not implement any Wasm Core proposal.

The polyfill also assumes the `WebAssembly` JavaScript API for module
compilation, instantiation, memories, tables, globals, and tags. On the web,
compilation and instantiation of a large module are asynchronous operations.
Resources, futures, streams, and tasks are represented inside the polyfill
and are not exposed to JavaScript.

## The Compatibility Matrix

The inventory is organized by subsystem. Each row carries the work the
polyfill must do, the status in Wasmtime (✅ implemented, ⚠️ gated behind a
feature flag, ❌ not implemented), and a reference.

### Component Binary Format

| Concern                                                                                                                         | Polyfill target                  | Wasmtime | Reference                        |
| ------------------------------------------------------------------------------------------------------------------------------- | -------------------------------- | -------- | -------------------------------- |
| Component preamble, sections, nested components, and aliases                                                                    | Implement through the translator | ✅       | [Binary – component definitions] |
| Type-encoding bytes `0x63` (`map`), `0x64` (`error-context`), `0x65` (`future`), `0x66` (`stream`)                              | Implement                        | ✅       | [Binary – type definitions]      |
| `canon` built-ins for tasks, waitable sets, streams, futures, `error-context`, context locals, backpressure, and `thread.yield` | Implement parsing and dispatch   | ✅       | [Binary – canonical definitions] |
| `canon thread.*` built-ins other than `thread.yield`                                                                            | Deferred (🧵 in development)     | ⚠️       | [Binary – canonical definitions] |
| Binary format warts scheduled for removal in 1.0                                                                                | Tolerate                         | ✅       | [Binary – warts]                 |

### Component Type System

| Concern                                                                      | Polyfill target | Wasmtime | Reference                        |
| ---------------------------------------------------------------------------- | --------------- | -------- | -------------------------------- |
| Primitives, records, variants, lists, options, results, tuples, flags, enums | Implement       | ✅       | [Explainer – type definitions]   |
| Structural type equality                                                     | Implement       | ✅       | [Explainer – type checking]      |
| Subtyping (variance, depth, width)                                           | Implement       | ✅       | [Explainer – type checking]      |
| `own<T>` and `borrow<T>` handles, synchronous destructors                    | Implement       | ✅       | [Explainer – handle types]       |
| Asynchronous resource destructors                                            | Implement       | ✅       | [Concurrency – blocking]         |
| Cross-component resource handle transfer                                     | Implement       | ✅       | [CanonicalABI – `canon lift`]    |
| `future<T>`, `stream<T>` as value types                                      | Implement       | ✅       | [Explainer – asynchronous types] |
| `error-context` as a value type                                              | Implement       | ⚠️       | [Explainer – error context]      |
| `map<K, V>`                                                                  | Implement       | ✅       | [Explainer – container types]    |
| Fixed-length `list<T, N>`                                                    | Implement       | ✅       | [Explainer – container types]    |
| The `async` bit on function types                                            | Implement       | ✅       | [Explainer – canonical ABI]      |
| `implements` and `external-id` annotations                                   | Tolerate        | ✅       | [Explainer – gated features]     |

### Canonical ABI

| Concern                                                                                              | Polyfill target                                                       | Wasmtime | Reference                         |
| ---------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------- | -------- | --------------------------------- |
| Lift and lower for every value type, `cabi_realloc`, parameter and result spill to memory            | Implement                                                             | ✅       | [CanonicalABI – flattening]       |
| String encodings UTF-8, UTF-16, and Latin-1 with UTF-16 fallback, and transcoding between components | Implement                                                             | ✅       | [CanonicalABI – storing]          |
| `post-return` for synchronous lifts                                                                  | Implement                                                             | ✅       | [CanonicalABI – `canon lift`]     |
| Asynchronous `canon lift`, stackless (callback) form                                                 | Implement                                                             | ✅       | [Concurrency – stackless exports] |
| Asynchronous `canon lift`, stackful form                                                             | Best effort: JSPI in browsers, native stack switching where available | ✅       | [Concurrency – stackful exports]  |
| Asynchronous `canon lower`                                                                           | Implement                                                             | ✅       | [Concurrency – async import ABI]  |
| Per-task lift and lower context                                                                      | Implement                                                             | ✅       | [CanonicalABI – runtime state]    |
| One handle table per component instance for resources, waitables, streams, and futures               | Implement                                                             | ✅       | [CanonicalABI – runtime state]    |
| Component instance flags (`may_leave`, `may_enter`, backpressure)                                    | Implement                                                             | ✅       | [CanonicalABI – runtime state]    |

### Concurrency Runtime

| Concern                                                              | Polyfill target                                                                     | Wasmtime | Reference                              |
| -------------------------------------------------------------------- | ----------------------------------------------------------------------------------- | -------- | -------------------------------------- |
| Cooperative scheduler                                                | Implement: JavaScript promises in the browser, a runtime-agnostic executor natively | ✅       | [Concurrency – threads and tasks]      |
| Task lifecycle and `task.return`, `task.cancel`                      | Implement                                                                           | ✅       | [Concurrency – returning]              |
| Subtasks and supertasks, `subtask.cancel`, `subtask.drop`            | Implement                                                                           | ✅       | [Concurrency – subtasks]               |
| Backpressure (`backpressure.inc`, `backpressure.dec`)                | Implement                                                                           | ✅       | [Concurrency – backpressure]           |
| Cancellation                                                         | Implement                                                                           | ✅       | [Concurrency – cancellation]           |
| Context locals (`context.get`, `context.set`)                        | Implement                                                                           | ✅       | [CanonicalABI – canonical definitions] |
| Waitable sets (`waitable-set.{new,wait,poll,drop}`, `waitable.join`) | Implement                                                                           | ✅       | [Concurrency – waitable sets]          |
| `thread.yield`                                                       | Implement                                                                           | ✅       | [Concurrency – thread built-ins]       |
| Event encoding for waits, polls, and callback status words           | Implement                                                                           | ✅       | [Concurrency – async ABI]              |
| Reentrance rules                                                     | Implement                                                                           | ✅       | [Concurrency – reentrance]             |

### Streams, Futures, and Error Contexts

| Concern                                                                        | Polyfill target    | Wasmtime | Reference                              |
| ------------------------------------------------------------------------------ | ------------------ | -------- | -------------------------------------- |
| `stream.{new,read,write,cancel-read,cancel-write,drop-readable,drop-writable}` | Implement          | ✅       | [Concurrency – streams and futures]    |
| `future.{new,read,write,cancel-read,cancel-write,drop-readable,drop-writable}` | Implement          | ✅       | [Concurrency – streams and futures]    |
| Stream readiness rules                                                         | Implement          | ✅       | [Concurrency – stream readiness]       |
| `error-context.{new,debug-message,drop}`                                       | Implement          | ⚠️       | [CanonicalABI – canonical definitions] |
| Drop and cancellation ordering edge cases (see CVE-2026-27195)                 | Add to test corpus | ✅       | [Concurrency – cancellation]           |

### Linking, Instantiation, and Host Integration

| Concern                                                                           | Polyfill target              | Wasmtime | Reference                                |
| --------------------------------------------------------------------------------- | ---------------------------- | -------- | ---------------------------------------- |
| `Engine`, `Store`, `Module`, `Instance` at the core layer (via the runtime layer) | Track upstream               | ✅       | [`wasmtime::component::Linker`]          |
| `Component`, `Linker`, `LinkerInstance`, `Instance` at the component layer        | Implement                    | ✅       | [`wasmtime::component::LinkerInstance`]  |
| Identifier model (`PackageName`, `InterfaceIdentifier`, semver)                   | Implement                    | ✅       | [Explainer – import and export]          |
| Host function definition, synchronous                                             | Implement, typed and untyped | ✅       | [`LinkerInstance::func_wrap`]            |
| Host function definition, asynchronous                                            | Implement                    | ✅       | [`LinkerInstance::func_wrap_concurrent`] |
| Host resource definition, synchronous destructor                                  | Implement                    | ✅       | [`wasmtime::component::ResourceType`]    |
| Host resource definition, asynchronous destructor                                 | Implement                    | ✅       | [`wasmtime::component::Accessor`]        |
| Composition: adapter modules, instance flags, transcoders between components      | Implement                    | ✅       | [Linking]                                |
| Host binding code generation (a `bindgen!` equivalent)                            | Implement                    | ✅       | [`wasmtime::component::bindgen!`]        |
| Component-level `start` function                                                  | Deferred (🪙 in development) | ⚠️       | [Explainer – start definitions]          |
| Value imports and exports                                                         | Deferred (🪙 in development) | ⚠️       | [Explainer – value definitions]          |
| WIT feature gates (`@since`, `@unstable`)                                         | Tolerate                     | ✅       | [WIT]                                    |

## Toward Component Model 1.0

The [Component Model 1.0 roadmap] announces changes that touch the Canonical
ABI itself. The polyfill does not implement them yet, but its design must not
block them:

- The lazy ABI. A lifted function returns values through a callback rather
  than through eager stores into linear memory. It ships as an opt-in
  `canonopt` in a 0.3.x release and becomes the default at 1.0. An adapter
  tool converts eager components to lazy ones.
- Multivalue returns at the C ABI level.
- An `error-context` value in the error case of every `result`.
- A GC ABI option, where components pass values in Wasm GC objects instead of
  linear memory.
- Native implementations in at least two browser engines. When that happens,
  the polyfill becomes a real polyfill: a fallback for browsers that lag.

The polyfill keeps its lift and lower strategy behind one seam, so that a
second ABI can sit beside the eager one.

## Conformance Corpora

Two `.wast` suites exist. The [Component Model test corpus] is organized by
`async`, `binary`, `linking`, `resources`, `validation`, and `values`, with an
expected-failure list (`nyi.txt`). The [Wasmtime component tests] cover
adapters, aliasing, large strings, string transcoding, resources, nested
instantiation, and an `async` directory. Wasmtime's trap messages are the
reference for the polyfill's trap messages. The polyfill runs both corpora
and records its expected failures the same way.

## What This Document Does Not Commit To

These items are bounds on the scope, not gaps:

- WASI worlds at the function level. The features they use (resources,
  concurrency, `stream<T>`, `future<T>`, `error-context`, the value types)
  are all in the matrix. `wasi:io` no longer exists in WASI 0.3. Its role is
  absorbed by `stream<T>` and `future<T>`.
- Wasm Core proposals that browsers do not ship, including stack switching.
- The thread built-ins other than `thread.yield`. The Concurrency explainer
  marks them 🧵 and in development.
- Value imports, value exports, and `start`. The Explainer marks them 🪙 and
  in development.
- A generic payloaded error type. The Explainer tracks it as future work.

## User Stories

A contributor approaches the project for the first time and wants one
document that tells them what is left to build.

> The contributor reads the matrix, picks the `future` lifecycle row, follows
> its reference, compares it with Wasmtime, and starts work with a clear
> scope.

A contributor evaluates a bug report and wants to know whether the feature is
supposed to work yet.

> A user reports that an `async` export hangs. The contributor finds the
> asynchronous `canon lift` row and replies that the feature is on the
> roadmap rather than opening a defect.

A maintainer responds to upstream churn and wants each feature to point back
to its source.

> The Concurrency explainer changes the waitable-set event encoding. The
> maintainer follows the reference from the event encoding row, reads the
> diff, and updates the row.

A developer adopting the polyfill wants a clear statement of what is
supported.

> The developer reads the matrix and the polyfill's release notes, and plans
> their integration around the supported features.

## References

- [PDD000], the product overview.
- [PDD001], the development environment.
- [PDD002], the ecosystem foundation.
- [`WebAssembly/component-model`], including the [Explainer], [Binary],
  [CanonicalABI], [Concurrency], [Linking], [WIT], and [`definitions.py`].
- [WASI 0.3] and the [WASI roadmap].
- The [Component Model 1.0 roadmap].
- [Wasmtime] and the [Wasmtime component tests].
- The [Component Model test corpus].
- [JSPI], the bridge between the concurrency features and a browser host.

[PDD000]: ./PDD000%20Wasm%20Component%20Model%20Polyfill.md
[PDD001]: ./PDD001%20Development%20Environment.md
[PDD002]: ./PDD002%20Ecosystem%20Foundation.md

[`WebAssembly/component-model`]: https://github.com/WebAssembly/component-model
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
[WASI 0.3]: https://wasi.dev/releases/wasi-p3
[WASI roadmap]: https://wasi.dev/roadmap
[Component Model 1.0 roadmap]: https://bytecodealliance.org/articles/the-road-to-component-model-1-0
[JSPI]: https://github.com/WebAssembly/js-promise-integration
[Component Model test corpus]: https://github.com/WebAssembly/component-model/tree/main/test
[Wasmtime component tests]: https://github.com/bytecodealliance/wasmtime/tree/main/tests/misc_testsuite/component-model

[Explainer]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md
[Explainer – gated features]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md#gated-features
[Explainer – type definitions]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md#type-definitions
[Explainer – type checking]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md#type-checking
[Explainer – handle types]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md#handle-types
[Explainer – asynchronous types]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md#asynchronous-value-types
[Explainer – error context]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md#error-context-type
[Explainer – container types]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md#container-types
[Explainer – canonical ABI]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md#canonical-abi
[Explainer – start definitions]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md#start-definitions
[Explainer – value definitions]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md#value-definitions
[Explainer – import and export]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md#import-and-export-definitions

[Binary]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Binary.md
[Binary – component definitions]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Binary.md#component-definitions
[Binary – type definitions]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Binary.md#type-definitions
[Binary – canonical definitions]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Binary.md#canonical-definitions
[Binary – warts]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Binary.md#binary-format-warts-to-fix-in-a-10-release

[CanonicalABI]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md
[CanonicalABI – runtime state]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#runtime-state
[CanonicalABI – flattening]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#flattening
[CanonicalABI – storing]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#storing
[CanonicalABI – `canon lift`]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#canon-lift
[CanonicalABI – canonical definitions]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#canonical-definitions
[`definitions.py`]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/canonical-abi/definitions.py

[Concurrency]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md
[Concurrency – threads and tasks]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md#threads-and-tasks
[Concurrency – subtasks]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md#subtasks-and-supertasks
[Concurrency – thread built-ins]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md#thread-built-ins
[Concurrency – blocking]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md#blocking
[Concurrency – waitable sets]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md#waitables-and-waitable-sets
[Concurrency – streams and futures]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md#streams-and-futures
[Concurrency – stream readiness]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md#stream-readiness
[Concurrency – backpressure]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md#backpressure
[Concurrency – returning]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md#returning
[Concurrency – cancellation]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md#cancellation
[Concurrency – reentrance]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md#reentrance
[Concurrency – async ABI]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md#async-abi
[Concurrency – async import ABI]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md#async-import-abi
[Concurrency – stackful exports]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md#stackful-async-exports
[Concurrency – stackless exports]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md#stackless-async-exports

[Linking]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Linking.md
[WIT]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md

[`wasmtime::component::Linker`]: https://docs.wasmtime.dev/api/wasmtime/component/struct.Linker.html
[`wasmtime::component::LinkerInstance`]: https://docs.wasmtime.dev/api/wasmtime/component/struct.LinkerInstance.html
[`LinkerInstance::func_wrap`]: https://docs.wasmtime.dev/api/wasmtime/component/struct.LinkerInstance.html#method.func_wrap
[`LinkerInstance::func_wrap_concurrent`]: https://docs.wasmtime.dev/api/wasmtime/component/struct.LinkerInstance.html#method.func_wrap_concurrent
[`wasmtime::component::ResourceType`]: https://docs.wasmtime.dev/api/wasmtime/component/struct.ResourceType.html
[`wasmtime::component::Accessor`]: https://docs.wasmtime.dev/api/wasmtime/component/struct.Accessor.html
[`wasmtime::component::bindgen!`]: https://docs.wasmtime.dev/api/wasmtime/component/macro.bindgen.html
