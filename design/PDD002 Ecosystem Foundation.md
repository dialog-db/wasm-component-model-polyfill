# Ecosystem Foundation

The Wasm Component Model Polyfill is not a from-scratch effort. The bulk of the
hard, valuable work that makes a runtime-agnostic Component Model implementation
plausible has already been done elsewhere in the Rust Wasm ecosystem, and the
polyfill's job is to compose, extend, and modernize that work rather than to
replicate it. Two upstream crates form the foundation of the project:
[`wasm_runtime_layer`] supplies the runtime abstraction that lets the same code
target browsers and native hosts, and [`wasm_component_layer`] supplies a
working Component Model implementation that — while frozen at WASI Preview 2
(wasip2) — provides the structural and semantic bones from which a wasip3
polyfill can grow.

This document records that posture: which upstream artifacts the polyfill
depends on, how their responsibilities partition the problem, and what the
project owes back to the ecosystem that made it possible.

## Goals

- Establish [`wasm_runtime_layer`] as the polyfill's runtime substrate, so that
  the browser-versus-Wasmtime split described in [PDD000] is solved by an
  upstream crate rather than re-engineered in this project.
- Define the polyfill's relationship to upstream projects clearly enough
  that contributors know when to upstream a fix, when to extend locally, and
  when to diverge.
- Honour the licenses, attribution, and stylistic conventions of the upstream
  crates so that the polyfill remains a good citizen of the Rust Wasm ecosystem.

## Non-goals

- This document does not enumerate every transitive dependency of the upstream
  crates; only the foundational projects whose design directly shapes the
  polyfill are in scope.
- This document does not specify how the polyfill's source tree is organised or
  how upstream code is incorporated mechanically (vendored copy, fork, patched
  dependency, etc.). That is an engineering-planning concern.
- This document does not commit the project to upstreaming any particular
  change. It describes a posture, not a schedule.
- This document does not address packaging or publication of the polyfill
  itself; that is out of scope here.

## Runtime Abstraction

[`wasm_runtime_layer`] is a thin, backend-agnostic façade over WebAssembly
Core runtimes. It defines a `WasmEngine` trait that backends implement, and
exposes the familiar `Engine`, `Store`, `Module`, `Instance`, and `Val` types
on top of that trait. Backends already exist for [Wasmtime], [Wasmer], [Wasmi],
the browser's native `WebAssembly` JavaScript API, and Pyodide. The polyfill
depends on at least the Wasmtime backend (for native execution) and the
browser backend (for `wasm32-unknown-unknown` execution); other backends are
neither required nor excluded.

This crate is precisely the abstraction that [PDD000] presupposes when it
promises consumers "a single API and mental model when targeting both
platforms." Rather than building that abstraction inside the polyfill, the
project consumes it from upstream and inherits the maintenance, testing, and
ecosystem reach that come with a shared dependency. Any improvements the
polyfill needs at the runtime layer — additional backend behaviour, missing
trait methods, performance work — should be pursued upstream first and brought
into the polyfill only when upstreaming is impractical.

## Relationship to Wasmtime

[Wasmtime] is the polyfill's *reference runtime*: the canonical reading of
"what does the Component Model do?" comes from Wasmtime, and Wasmtime's
`wasmtime::component` API shapes the polyfill's familiar names. But the
polyfill is not a wrapper over `wasmtime::component`. Wasmtime participates
in the polyfill's stack only as a [`wasm_runtime_layer`] backend — i.e.
as a core-Wasm engine — on native targets. Component-level work (parsing,
the type system, lift/lower, instantiation, the canonical-ABI runtime
state, host-function and host-resource registration) is implemented by
the polyfill on top of `wasm_runtime_layer`'s generic core-Wasm
abstractions, so the same component-level code drives every supported
target.

This single-architecture stance is the polyfill's reading of
[`wasm_component_layer`]: the prior art proves that component-level semantics
can be built on top of `wasm_runtime_layer`'s `Engine`, `Store`, `Module`,
`Instance`, `Func`, `Memory`, `Global`, and `Table` without reaching for any
backend-specific component runtime, and that the same source then compiles
cross-target. The polyfill carries the same architecture forward, refining and
extending it where the prior art falls short.

The web target (`wasm32-unknown-unknown`) is therefore *not* a separate
re-implementation — it is the same component-level code, running over
the runtime-layer's browser backend instead of its Wasmtime backend.
The polyfill's public surface (`Engine`, `Component`, `Linker`,
`LinkerInstance`, `Instance`, host-function and host-resource
registration) is the same on every target.

Two corollaries follow from this posture, and they concern every following PDD
that introduces a component-layer type:

- **Don't reach into a backend-specific component runtime.** If the
  polyfill is depending on `wasmtime::component`, vendoring its source,
  or building parallel native-only logic that web has to retrace later,
  that is almost certainly the wrong shape. The runtime substrate is
  core-Wasm only on every target; component-level work happens above
  it.
- **Minimise the polyfill's public API surface.** Wasmtime's
  `wasmtime::component` API is large; the polyfill is not obliged to
  shadow all of it. Each PDD exposes only the types and methods load-
  bearing for that PDD's user stories, and resists the urge to mirror
  Wasmtime one-for-one. The polyfill is a polyfill, not a re-export.

## Component Model Foundation

[`wasm_component_layer`] is, to our knowledge, the only extant
runtime-agnostic implementation of the WebAssembly Component Model in Rust.
It builds on [`wasm_runtime_layer`] and provides the `Engine`, `Store`,
`Component`, `Linker`, and `Instance` types that a host needs in order to
load, link, instantiate, and call into a component. It supports parsing
component binaries, runtime construction of component interface types, guest
and host resources with destructors, and structural type equality as required
by the Component Model specification (citation needed).

The crate targets a subset of wasip2 and predates the wasip3 proposal. As of the
writing of this document, it is effectively unmaintained: the upstream
repository has not been advanced to track wasip3, and several wasip2-era
limitations (string transcoders, host binding macros, subtyping, broader test
coverage) remain unresolved. None of this diminishes the value of the work; it
simply means that the polyfill cannot reach its goals by depending on
`wasm_component_layer` unmodified.

The polyfill's strategy is therefore to treat `wasm_component_layer` as a seed
crystal and a prior art reference. Its data structures, traversal patterns, and
ABI implementations are the starting point from which a wasip3-capable polyfill
is grown. Where the existing design carries directly forward to wasip3, the
polyfill preserves it; where wasip3 diverges from wasip2, the polyfill extends,
replaces, or rewrites the relevant pieces. Where existing wasip2 functionality
is incomplete (the gaps the upstream README already notes), the polyfill is free
to finish the job. The specific shape of the wasip3 deltas — and the polyfill's
plan for absorbing them — is out of scope here.

## Relationship to Upstream

The polyfill is not a hostile fork. The project's preferred posture, in
descending order of preference, is:

1. **Use upstream as published**, with no changes, when the upstream code
   already meets the polyfill's needs.
2. **Contribute upstream**, when a needed change is small, generally useful,
   and likely to be accepted by an active maintainer or successor.
3. **Carry a local change**, when upstreaming is impractical (the upstream is
   inactive, the change is polyfill-specific, or the change is too large to
   land outside the polyfill's own iteration cycle), while keeping the local
   change shaped so it could be upstreamed later if circumstances change.

Because `wasm_component_layer` is currently inactive, option (3) is the
expected default for Component Model work. Because `wasm_runtime_layer` is
healthier, option (2) is the expected default for runtime-layer work.

The polyfill carries forward upstream copyright notices and licenses
unchanged in any code it adopts, and credits the upstream projects in the
project's top-level documentation. Both upstream crates are dual-licensed
under MIT and Apache-2.0, which is compatible with the polyfill's own license.

## User Stories

**As a maintainer of the polyfill**, I want the runtime-versus-browser split
to be somebody else's problem, so that my own engineering effort can be spent
on the parts of the Component Model that nobody has yet polyfilled for wasip3.

> The maintainer reaches for `wasm_runtime_layer`'s `Engine` and `Store`
> abstractions whenever the polyfill needs to talk to a Wasm Core runtime.
> They do not write conditional code paths for the browser versus Wasmtime;
> the abstraction does that for them. When they do find a gap in the
> abstraction, they open a pull request against the upstream crate before
> they consider patching it locally.

**As a maintainer of the polyfill**, I want a known-good Component Model
implementation to learn from and grow into wasip3, so that I am not
re-deriving the canonical ABI, the resource lifecycle, and the type system
from scratch.

> The maintainer reads `wasm_component_layer`'s implementation as the primary
> reference for how a host loads, links, and instantiates a component on top
> of `wasm_runtime_layer`. Where wasip3 preserves wasip2 semantics, they
> preserve the upstream implementation; where wasip3 changes semantics or
> introduces new concepts, they make targeted, well-attributed changes.

**As a developer adopting the polyfill**, I want to load and instantiate
components in my web application using familiar Rust Wasm idioms, so that I
do not need to learn a polyfill-specific API on top of the abstractions the
ecosystem has already standardised on.

> The developer encounters `Engine`, `Store`, `Component`, `Linker`, and
> `Instance` types whose names, shapes, and responsibilities mirror those of
> `wasm_component_layer` and, by extension, Wasmtime's component API. Their
> code reads like host code anywhere else in the Rust Wasm ecosystem, even
> though it happens to be running in a browser.

**As a maintainer of `wasm_runtime_layer` or a future maintainer of
`wasm_component_layer`**, I want the polyfill to be a constructive downstream
that contributes back when it can, so that my project benefits from the
polyfill's existence rather than being eclipsed by it.

> The polyfill's contributors open issues and pull requests against the
> upstream repositories whenever a fix is general-purpose. When the polyfill
> must carry a local divergence, the divergence is documented in-tree with a
> note explaining why upstreaming was not pursued, so that a future
> maintainer can pick up the thread.

## References

- [PDD000] — the Wasm Component Model Polyfill product overview, which
  motivates the cross-runtime stance this document satisfies via upstream
  crates.
- [PDD001] — the development environment, which provides the toolchain in
  which the upstream crates and any local extensions are built and tested.
- [`wasm_runtime_layer`] — the runtime abstraction the polyfill builds on.
- [`wasm_component_layer`] — the wasip2 Component Model implementation whose
  bones inform the polyfill's wasip3 work.
- [Wasm Component Model] — the specification the polyfill ultimately
  implements.
- [WASI Roadmap] — the project's own description of the WASI 0.3.0 (wasip3)
  release the polyfill targets.

[PDD000]: ./PDD000%20Wasm%20Component%20Model%20Polyfill.md
[PDD001]: ./PDD001%20Development%20Environment.md
[`wasm_runtime_layer`]: https://github.com/DouglasDwyer/wasm_runtime_layer
[`wasm_component_layer`]: https://github.com/DouglasDwyer/wasm_component_layer
[Wasm Component Model]: https://github.com/WebAssembly/component-model
[WASI Roadmap]: https://wasi.dev/roadmap
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
[Wasmer]: https://github.com/wasmerio/wasmer
[Wasmi]: https://github.com/wasmi-labs/wasmi
