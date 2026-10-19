# Architecture

The polyfill has three layers.

## Translation

[`wasmtime-environ`] parses and validates the component binary, resolves nested
components and aliases, and compiles a fused adapter module for every call
between two components. The same translator runs on every target.

## Core execution

The polyfill's own runtime layer, `wcmp-wasm-core`, runs the core modules on the
backend the host chooses. `wcmp-wasm-core-wasmtime` runs them on Wasmtime 49,
used only as a core Wasm engine, and `wcmp-wasm-core-web` runs them on the
browser's `WebAssembly` JavaScript API. The polyfill has no backend of its own:
the host hands one to `Engine::with_backend`. [Backends](backends.md) compares
the backends.

## The Component Model runtime

Everything above core Wasm is the polyfill's own Rust:

- the Canonical ABI: lift, lower, string transcoding, `post-return`, and
  `cabi_realloc`
- one handle table per component instance, for resources and waitables
- the adapter intrinsics
- a cooperative scheduler per `Store` that runs guest tasks, host `async`
  functions, and the concurrency built-ins

A [suspend provider](suspend-providers.md) gives each guest thread a stack of
its own, so a guest can block in the middle of a call.

## Outside the polyfill

The polyfill implements no Wasm Core proposal and no WASI world. The core engine
supplies the proposals. A WASI world is a consumer of the polyfill and links
into a `Linker` like any other import.

[`wasmtime-environ`]: https://docs.rs/wasmtime-environ
