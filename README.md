# Wasm Component Model Polyfill

A Rust library that loads, links, instantiates, and calls [WebAssembly
Components][Component Model] on a platform that only implements [Wasm Core]. One
API serves native hosts and web browsers. The polyfill targets the Component
Model as released with [WASI 0.3], including its `async` functions and the task
built-ins that drive them, and its public API mirrors the names and shapes of
[Wasmtime]'s component API.

The project is in active development.
[Feature support](notes/feature-support.md) shows what works today. Try the
[live demo].

## Why a polyfill

Browsers implement Wasm Core and the `WebAssembly` JavaScript API, but no
browser implements the Component Model. Take a Rust web application that loads
plugins at run time. The plugins are Wasm Components, written in any language
and possibly composed from several components with [`wac`]. The polyfill reads
each component binary, drives the browser's engine to instantiate the core
modules inside it, and supplies the Canonical ABI and the built-ins that connect
them.

The same application also runs natively, as a desktop build or as a test suite.
There the polyfill runs on Wasmtime's core engine, and the host code does not
change between the two builds. [Architecture](notes/architecture.md) describes
the layers.

If you are familiar with Wasm but new to the [Component Model], this video gives
an eight-minute introduction:

[![Wasm Components Explainer](https://img.youtube.com/vi/h04vdcj03Ss/0.jpg)](https://www.youtube.com/watch?v=h04vdcj03Ss)

## Requirements

- Native: any target Wasmtime 50 or Wasmi 2.0 supports, with the runtime layer's
  Wasmtime or Wasmi backend as the core engine. The library is executor
  agnostic. The tests use `tokio`.
- Web: `wasm32-unknown-unknown` with `wasm-bindgen`, with the browser's
  `WebAssembly` API as the core engine. Guests can block in every current
  browser, through JSPI.
- Rust: stable, edition 2024, with the `wasm32-unknown-unknown` target
  installed. The `rust-toolchain.toml` pins the channel.

The consumer of the polyfill is Rust code. There is no JavaScript API.

## Quick start

A host depends on the polyfill and on one backend per target, as the crate
`rust/wcmp-downstream` does:

```toml
[dependencies]
wcmp = "0.1.0"

[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
wcmp-wasm-core-wasmtime = "0.1.0"

[target.'cfg(target_arch = "wasm32")'.dependencies]
wcmp-wasm-core-web = "0.1.0"
```

The host below loads a component, lends it one host function, and calls a typed
export. The same code compiles natively and for `wasm32-unknown-unknown` with
`wasm-bindgen`. Only the backend it names differs.

```rust
use wcmp::*;

async fn run(wasm: &[u8]) -> Result<String> {
    // The engine runs core Wasm on the backend the host names: Wasmtime
    // natively, or `wcmp_wasm_core_web::Web::new()` in a browser. It owns
    // the feature gates. The store owns guest state and the host data,
    // `()` here.
    let backend = wcmp_wasm_core_wasmtime::Wasmtime::new().expect("Wasmtime makes an engine");
    let engine = Engine::with_backend(backend)?;
    let mut store = Store::new(&engine, ())?;

    // Compiling is awaited: the browser compiles large modules only
    // asynchronously, and the same signature serves native.
    let component = Component::new(&engine, wasm).await?;

    // A typed host function. Its Rust signature derives the component
    // function type the linker checks against the import.
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap("log", |_call: HostCall<'_, ()>, (line,): (String,)| {
            println!("{line}");
            Ok(())
        });

    let instance = linker.instantiate(&mut store, &component).await?;

    // Walk the export tree to a function inside an exported interface,
    // and check its signature against a Rust tuple once, up front.
    let select_nth = instance
        .exports()
        .instance("test:guest/foo")
        .expect("the guest exports the interface")
        .func("select-nth")
        .expect("the interface exports the function")
        .typed::<(Vec<String>, u32), String>()?;

    let items = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    select_nth.call(&mut store, (items, 1)).await
}
```

The smoke test in `rust/wcmp-smoke` covers the rest: compositions, resources,
streams and futures, guests that block, cancellation, and traps. It runs
natively (`tests smoke native`) and as a browser page (`tests smoke web`). Open
the page to check whether the polyfill works in a given browser.

## Development

The repository is a Nix flake. Enter the shell with `nix develop`, which prints
the menu of commands. Every build and test goes through a menu command, never
through bare `cargo`:

```sh
nix develop
build debug          # build the polyfill crate for both targets
tests native debug   # unit and integration tests on the host
tests web debug      # the same tests in headless Chrome
tests all            # every lane, debug and release, native and web
lint                 # every check the flake declares
```

Nix sees only tracked files, so commit or snapshot before a `tests` or `lint`
command. [Development](notes/development.md) lists every command, and
[Repository layout](notes/repository-layout.md) maps the crates and the design
documents.

## Notes

- [Architecture](notes/architecture.md): translation, core execution, and the
  Component Model runtime.
- [Feature support](notes/feature-support.md): the Component Model feature by
  feature, with status and settings.
- [Host bindings with `bindgen!`](notes/bindgen.md): how to use
  `wcmp::bindgen!`, and how it differs from Wasmtime's `bindgen!`.
- [Differences from Wasmtime](notes/differences-from-wasmtime.md): what a host
  can notice when it moves from Wasmtime.
- [Suspend providers](notes/suspend-providers.md): how a guest blocks on each
  backend, and what happens without a provider.
- [Backends](notes/backends.md): the browser, Wasmi, and Wasmtime backends,
  their capabilities, and their test results.
- [Conformance](notes/conformance.md): pass rates on the Component Model and
  Wasmtime test corpora.
- [Memory](notes/memory.md): how each backend holds guest memory.
- [Performance](notes/performance.md): benchmark figures for the browser's call
  path and for floats.
- [Known limits](notes/known-limits.md): engine defects, web embedding limits,
  and backend limits.
- [Development](notes/development.md): every menu command.
- [Repository layout](notes/repository-layout.md): the crates, the board, and
  the design documents.

## Acknowledgements

This project's runtime layer crates were inspired by the [`wasm_runtime_layer`]
crates by Douglas Dwyer, which are licensed under either the Apache License,
Version 2.0 or the MIT license. We prototyped the polyfill on those crates. To
meet the project's goals, we then wrote our own runtime layer crates from
scratch. No code from `wasm_runtime_layer` remains in this repository.

## License

Copyright 2026 The Dialog DB Project.

This project is licensed under the [Mozilla Public License 2.0](LICENSE). Each
source file states the same at its top.

Two upstream test suites are vendored under `rust/wcmp/tests/corpus/` and keep
their own licenses: `cm/` comes from the Component Model and is under the Apache
License 2.0, and `wasmtime/` comes from Wasmtime and is under the Apache License
2.0 with LLVM exceptions. Each directory holds a copy of its license.

[Component Model]: https://github.com/WebAssembly/component-model
[Wasm Core]: https://webassembly.github.io/spec/core/
[WASI 0.3]: https://wasi.dev/releases/wasi-p3
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
[`wac`]: https://github.com/bytecodealliance/wac
[live demo]: http://dialog-db.github.io/wasm-component-model-polyfill/
[`wasm_runtime_layer`]: https://github.com/DouglasDwyer/wasm_runtime_layer
