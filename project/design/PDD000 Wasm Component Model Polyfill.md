# Wasm Component Model Polyfill

The Wasm Component Model Polyfill is a Rust library. It lets a Rust program
load, link, instantiate, and call [Wasm Components] on a platform that only
implements [Wasm Core]. The library exposes one API on every target it supports.
Today those targets are native hosts and web browsers.

The polyfill targets the Component Model as released with [WASI 0.3]. That
release added native concurrency to the Component Model: `async` functions,
`stream<T>`, `future<T>`, and the task and waitable built-ins that drive them.
The polyfill tracks the 0.3.x release train and keeps its design open to the
changes that the [Component Model 1.0 roadmap] announces.

## Goals

- A Rust host program can load, link, instantiate, and call components through
  one API on native targets and in web browsers.
- The API mirrors the names and shapes of [Wasmtime]'s component API, so that a
  Rust developer who knows Wasmtime feels at home.
- Components composed from other components link and run. Dynamic linking and
  code re-use are the reason the Component Model exists.
- Components that use the Component Model 0.3 concurrency features run on every
  supported target.

## Non-goals

- A JavaScript API. The consumer of the polyfill is Rust code, either native or
  compiled to `wasm32-unknown-unknown` with `wasm-bindgen`.
- Implementations of Wasm Core proposals. The polyfill uses the Wasm Core engine
  of the platform as it is.
- Implementations of WASI worlds such as `wasi:http` or `wasi:cli`. A WASI world
  is a consumer of the polyfill, not a part of it.

## Use Case

A web application written in Rust wants to load plugins at run time. The plugins
are Wasm Components, written in any language and possibly composed from several
components. Browsers implement Wasm Core and the `WebAssembly` JavaScript API,
but no browser implements the Component Model. The polyfill closes that gap. It
reads the component binary, drives the browser's core engine to instantiate the
core modules inside it, and implements the [Canonical ABI] and the concurrency
built-ins that connect them.

The same application also runs natively, for example as a desktop build or a
test suite. There the polyfill runs on top of a native core engine. The host
code does not change between the two builds.

## User Stories

A Rust developer builds a plugin system for a browser application. They compile
their host to `wasm32-unknown-unknown` and use the polyfill to load plugin
components at run time.

> The developer creates an engine and a store, loads a component from bytes,
> links it against host functions, and calls its exports. They use the same code
> in their native test suite. No platform-specific branch appears in their host
> code.

A Rust developer receives a component that was composed with `wac` from two
components written in different languages.

> The developer loads the composed component. The polyfill links the inner
> components to each other and to the host. The developer calls the outer
> exports without knowing how the component was composed.

A Rust developer targets a WASI 0.3 world whose functions are `async` and whose
I/O uses `stream<T>`.

> The developer calls an `async` export and awaits it. The polyfill drives the
> guest task, delivers stream data, and resolves the host's future when the
> guest returns.

## References

- [Wasm Components] and the [Canonical ABI] in the Component Model repository.
- [WASI 0.3], the release that the polyfill targets.
- The [Component Model 1.0 roadmap], which announces the changes after 0.3.
- [Wasmtime], the reference implementation of the Component Model.
- [Wasm Core], the specification that the platform engine implements.

[Wasm Core]: https://webassembly.github.io/spec/core/
[Wasm Components]: https://github.com/WebAssembly/component-model
[Canonical ABI]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md
[WASI 0.3]: https://wasi.dev/releases/wasi-p3
[Component Model 1.0 roadmap]:
  https://bytecodealliance.org/articles/the-road-to-component-model-1-0
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
