# Ecosystem Foundation

The polyfill is not a from-scratch effort. The Rust Wasm ecosystem already
provides a runtime abstraction that spans browsers and native hosts, a reference
implementation of the Component Model, and a target-agnostic component
translator. The polyfill composes those pieces and adds the part that nobody
provides: a Component Model runtime that a Rust host can use on every target
with one API.

This document records that posture. It names the upstream projects the polyfill
depends on, how they divide the problem, and what the polyfill owes back to
them.

## Goals

- Establish [`wasm_runtime_layer`] as the runtime layer of the polyfill, so that
  an upstream crate solves the browser-versus-native split.
- Establish [Wasmtime] as the reference implementation and as the source of the
  component translator the polyfill uses on every target.
- Define the relationship to upstream projects clearly enough that a contributor
  knows when to upstream a fix, when to carry a local change, and when to
  diverge.
- Honor the licenses and attribution of the upstream projects.

## Non-goals

- A list of every transitive dependency. Only the projects whose design shapes
  the polyfill are in scope.
- The mechanics of incorporating upstream code (patched dependency, fork, or
  vendored copy). Engineering planning decides that.
- Packaging or publication of the polyfill.

## Runtime Layer

[`wasm_runtime_layer`] is a thin facade over Wasm Core runtimes. It defines a
`WasmEngine` trait, and a backend (an implementation of that trait) exists for
[Wasmtime], [Wasmer], [Wasmi], and the browser's `WebAssembly` JavaScript API.
On top of the trait it exposes `Engine`, `Store`, `Module`, `Instance`, `Func`,
`Memory`, `Global`, `Table`, and `Val`.

The polyfill depends on the Wasmtime backend for native execution and on the
browser backend for `wasm32-unknown-unknown`. Other backends are neither
required nor excluded. The runtime layer sees only core Wasm. Every
component-level concern (translation, the type system, lift and lower,
instantiation, resource tables, tasks) is implemented in the polyfill on top of
the runtime layer's generic types. The same component-level code runs on every
target.

The polyfill wraps the runtime layer. It never re-exports a runtime layer type
from its public API.

### The Browser Backend

The browser backend of the runtime layer must surface a host function's `Err(_)`
result to the guest as a trap. A backend that returns `undefined` to the guest
instead hides every error the polyfill raises inside a host trampoline. The
polyfill files such a defect upstream with a reproduction, and carries a patched
copy of the backend until the fix lands. The patched copy keeps the shape of
upstream so that the patch can be dropped when upstream catches up.

The polyfill does not replace the runtime layer on the web. If a browser
capability that the polyfill needs cannot be expressed through the runtime
layer's trait (for example, an asynchronous compile or a [JavaScript Promise
Integration][JSPI] wrapper), the polyfill proposes the extension upstream first.

## Wasmtime

[Wasmtime] has two roles.

First, Wasmtime is the reference implementation. When the Component Model design
documents are silent or ambiguous, Wasmtime's behavior decides. The polyfill's
public API mirrors the names of `wasmtime::component`.

Second, Wasmtime's component translator, published as the [`wasmtime-environ`]
crate, is the polyfill's parser. The translator reads a component binary and
produces the flattened plan that a runtime needs: the core modules to compile,
the order to instantiate them, the imports and exports to wire, the canonical
ABI options of every lift and lower, and the adapter modules that connect
components to each other. The translator is pure Rust and builds for
`wasm32-unknown-unknown`. The polyfill runs it on every target, so there is one
parsing path.

Using the translator is not delegation to `wasmtime::component`. The polyfill
does not depend on Wasmtime's component runtime, does not vendor its source, and
does not build native-only logic that the web target has to retrace. Wasmtime
participates at run time only as a core Wasm backend of the runtime layer, and
only on native targets.

Two corollaries follow for every PDD that introduces a component-layer type:

- Do not reach into a backend-specific component runtime. The runtime layer is
  core Wasm only on every target. Component-level work happens above it.
- Keep the public API small. Wasmtime's component API is large. Each PDD exposes
  only the types and methods that its user stories need.

## Prior Art

[`wasm_component_layer`] is a Rust implementation of the Component Model on top
of the runtime layer. It predates the concurrency features of Component Model
0.3 and is inactive. The polyfill reads it as prior art. Its data structures,
traversal patterns, and ABI choices inform the polyfill's design. The polyfill
does not depend on it, vendor it, or copy it. Every line of the polyfill is
written for the polyfill. Where a design choice comes from the prior art, a
comment at that place in the source says so.

## Related Projects

Three projects run components in JavaScript environments. None of them serves a
Rust host, so none of them replaces the polyfill. Each is a useful reference:

- [jco] transpiles a component ahead of time into core modules and JavaScript
  glue.
- [jsco] is a TypeScript runtime that instantiates a component from bytes at run
  time.
- [polyengine] is a TypeScript runtime that uses the same translation strategy
  as the polyfill: Wasmtime's translator compiled to `wasm32` produces a plan,
  and a runtime executes it with the `WebAssembly` API. It implements the
  Component Model 0.3 concurrency features and runs the official conformance
  corpus in browsers.

## Relationship to Upstream

The polyfill is not a hostile fork. In descending order of preference, the
polyfill will:

1. Use upstream as published, when the upstream code meets the need.
2. Contribute upstream, when a change is small, generally useful, and likely to
   be accepted by an active maintainer.
3. Carry a local change, when upstreaming is impractical. A local change is
   shaped so that it can be upstreamed later, and a note in the tree explains
   why upstreaming was not pursued.

The runtime layer and the translator are active, so option 2 is the default for
them. The prior art is inactive, so it is a reference only.

The polyfill carries forward the copyright notices and licenses of any upstream
code it adopts, and credits the upstream projects in its top-level
documentation. The runtime layer and the prior art are dual-licensed under MIT
and Apache-2.0, which is compatible with the polyfill's license.

## User Stories

A maintainer of the polyfill wants the runtime-versus-browser split to be
somebody else's problem.

> The maintainer reaches for the runtime layer whenever the polyfill talks to a
> core Wasm engine. They do not write conditional code paths for the browser
> versus Wasmtime. When they find a gap in the abstraction, they open a pull
> request upstream before they patch locally.

A maintainer of the polyfill wants one parser that behaves the same on every
target.

> The maintainer runs Wasmtime's translator inside the polyfill. The plan it
> produces drives instantiation on native and on the web. A parsing bug is fixed
> once.

A developer adopting the polyfill wants familiar names.

> The developer meets `Engine`, `Store`, `Component`, `Linker`, and `Instance`.
> Their host code reads like Wasmtime host code, even in a browser.

A maintainer of an upstream project wants a constructive downstream.

> The polyfill's contributors open issues and pull requests upstream when a fix
> is general. A local divergence carries a note that explains why.

## References

- [PDD000], the product overview.
- [PDD001], the development environment.
- [`wasm_runtime_layer`], the runtime layer.
- [`wasmtime-environ`], the component translator.
- [`wasm_component_layer`], prior art.
- [Wasmtime], the reference implementation.
- [jco], [jsco], and [polyengine], related projects.
- [JSPI], the JavaScript Promise Integration proposal.

[PDD000]: ./PDD000%20Wasm%20Component%20Model%20Polyfill.md
[PDD001]: ./PDD001%20Development%20Environment.md
[`wasm_runtime_layer`]: https://github.com/DouglasDwyer/wasm_runtime_layer
[`wasmtime-environ`]: https://docs.rs/wasmtime-environ
[`wasm_component_layer`]: https://github.com/DouglasDwyer/wasm_component_layer
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
[Wasmer]: https://github.com/wasmerio/wasmer
[Wasmi]: https://github.com/wasmi-labs/wasmi
[jco]: https://github.com/bytecodealliance/jco
[jsco]: https://github.com/pavelsavara/jsco
[polyengine]: https://github.com/polymorph-components/polyengine
[JSPI]: https://github.com/WebAssembly/js-promise-integration
