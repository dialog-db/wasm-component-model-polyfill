# Wasm Component Model Polyfill

A Rust library that loads, links, instantiates, and calls [WebAssembly
Components][Component Model] on a platform that only implements [Wasm Core]. One
API serves native hosts and web browsers. The polyfill targets the Component
Model as released with [WASI 0.3], including its `async` functions and the task
built-ins that drive them, and its public API mirrors the names and shapes of
[Wasmtime]'s component API.

The project is under active development. The [feature support](#feature-support)
tables below say what runs today, what is gated, and what is still to come.

## How it works

The polyfill composes three upstream pieces and adds the part that none of them
provides:

- **Translation.** [`wasmtime-environ`] parses and validates the component
  binary, resolves nested components and aliases, and compiles a fused adapter
  module for every call between two components. The same translator runs on
  every target.
- **Core execution.** [`wasm_runtime_layer`] runs the core modules. Natively the
  backend is Wasmtime 49, used only as a core-Wasm engine. In the browser the
  backend is the `WebAssembly` JavaScript API through `js_wasm_runtime_layer`.
  Both backends are vendored under `rust/vendor/` with small patches, described
  in the `PATCHES.md` beside each.
- **The Component Model runtime.** Everything above core Wasm is the polyfill's
  own Rust: the Canonical ABI (lift, lower, string transcoding, `post-return`,
  `cabi_realloc`), one handle table per component instance for resources and
  waitables, the adapter intrinsics, and a cooperative scheduler per `Store`
  that runs guest tasks, host `async` functions, and the concurrency built-ins.

The polyfill implements no Wasm Core proposal and no WASI world. A WASI world is
a consumer of the polyfill and links into a `Linker` like any other import.

## The use case

A Rust web application wants to load plugins at run time. The plugins are Wasm
Components, written in any language and possibly composed with [`wac`] from
several components. Browsers implement Wasm Core and the `WebAssembly`
JavaScript API, but no browser implements the Component Model. The polyfill
closes that gap. It reads the component binary, drives the browser's engine to
instantiate the core modules inside it, and supplies the Canonical ABI and the
built-ins that connect them.

The same application also runs natively, as a desktop build or as a test suite.
There the polyfill runs on top of Wasmtime's core engine. The host code does not
change between the two builds.

## Quick start

The host below loads a component, lends it one host function, and calls a typed
export. The same code compiles natively and for `wasm32-unknown-unknown` with
`wasm-bindgen`.

```rust
use wasm_component_model_polyfill::*;

async fn run(wasm: &[u8]) -> Result<String> {
    // The engine owns the feature gates. The store owns guest state
    // and the host data, `()` here.
    let engine = Engine::new()?;
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

The end-to-end smoke test under `rust/wcmp-smoke` tells the rest of the story
chapter by chapter: a `wac` composition, host resources, a lent core module,
maps and fixed-length lists, a wit-bindgen world, a 64-bit memory, export
introspection, a gated feature, and awaiting outside the store. It runs as a
native binary and as a browser page from one source.

## Feature support

The tables list the Component Model feature by feature, as the [Explainer], the
[Binary format][Binary], the [Canonical ABI][CanonicalABI], and the [Concurrency
explainer][Concurrency] define them. A status means:

| Status | Meaning                                                                                                        |
| ------ | -------------------------------------------------------------------------------------------------------------- |
| ✅     | Implemented on both targets and exercised by the test suites.                                                  |
| 🟡     | Implemented with a limit the notes state.                                                                      |
| 🔒     | Off by default, as in Wasmtime. A host opts in through an `EngineConfig` setter. The notes say what then runs. |
| ❌     | Not implemented. The polyfill refuses the feature with `Error::Unsupported`.                                   |
| ⛔     | Out of scope: a limit of the runtime layer or a stated non-goal.                                               |

### Component binary format

| Feature                                          | Status | Notes                                                                                                                   |
| ------------------------------------------------ | ------ | ----------------------------------------------------------------------------------------------------------------------- |
| Component preamble, sections, imports, exports   | ✅     | Parsed and validated by the translator on every target.                                                                 |
| Nested components and aliases                    | ✅     |                                                                                                                         |
| Binary-format validation                         | 🟡     | Twenty corpus directives that the specification rejects are accepted today. They are recorded as `validation` failures. |
| Binary format warts scheduled for removal in 1.0 | ✅     | Tolerated, as Wasmtime tolerates them.                                                                                  |
| `implements` annotation on plain-named instances | 🔒     | `wasm_component_model_implements`. Accepted when on.                                                                    |
| Component-level `start` function                 | ❌     | Not accepted. The Explainer marks it in development (🪙).                                                               |
| Value imports and exports                        | ❌     | Not accepted. Same status as `start`.                                                                                   |

### Type system

| Feature                                                                      | Status | Notes                                                                                                                                     |
| ---------------------------------------------------------------------------- | ------ | ----------------------------------------------------------------------------------------------------------------------------------------- |
| Primitives (`bool`, integers, floats, `char`, `string`)                      | ✅     |                                                                                                                                           |
| `record`, `variant`, `enum`, `flags`, `tuple`, `option`, `result`, `list<T>` | ✅     |                                                                                                                                           |
| `map<K, V>`                                                                  | ✅     | On by default. `Val::Map`.                                                                                                                |
| Fixed-length `list<T, N>`                                                    | ✅     | On by default. `Val::FixedLengthList`.                                                                                                    |
| `own<T>` and `borrow<T>`                                                     | ✅     |                                                                                                                                           |
| Resource types, imported and locally defined                                 | ✅     |                                                                                                                                           |
| Core module types in imports and exports                                     | ✅     | A host lends a `Module` through `LinkerInstance::module`. A component can export one.                                                     |
| `async` function types                                                       | ✅     |                                                                                                                                           |
| `stream<T>` and `future<T>` as value types                                   | ❌     | A component that declares one is refused. The design is written and the implementation is next.                                           |
| `error-context` type                                                         | ❌     | The gate `wasm_component_model_error_context` exists. The built-ins behind it are refused.                                                |
| Structural type equality                                                     | ✅     | Two declarations of one shape are one `ValueType`.                                                                                        |
| Subtyping at the host boundary                                               | ❌     | The linker matches a host registration to an import structurally. Inside a component, the translator's validation applies the spec rules. |

### Canonical ABI

| Feature                                                            | Status | Notes                                                                                                                             |
| ------------------------------------------------------------------ | ------ | --------------------------------------------------------------------------------------------------------------------------------- |
| Lift and lower for every supported value type                      | ✅     |                                                                                                                                   |
| `cabi_realloc`, `memory`, parameter and result spill to memory     | ✅     |                                                                                                                                   |
| String encodings `utf8`, `utf16`, `latin1+utf16`                   | ✅     |                                                                                                                                   |
| String transcoding between components                              | ✅     | Every transcode operation the fused adapter compiler emits.                                                                       |
| `post-return`                                                      | ✅     |                                                                                                                                   |
| 64-bit memories in canonical options                               | ✅     | On by default (`wasm_component_model_memory64`).                                                                                  |
| `canon lift async` with a `callback` (stackless)                   | ✅     |                                                                                                                                   |
| `canon lift async` without a `callback` (stackful)                 | ❌     | Refused even when `wasm_component_model_async_stackful` is on. Neither target can suspend a guest thread in the middle of a call. |
| `canon lower async`                                                | ✅     | Status word, subtask handle in the caller's table, subtask events.                                                                |
| Fused adapters between components, every pairing of lift and lower | ✅     |                                                                                                                                   |
| Instance flags (`may_leave`, `may_enter`, backpressure)            | ✅     |                                                                                                                                   |
| One handle table per component instance                            | ✅     | Resources, waitables, waitable sets, and subtasks share it.                                                                       |
| Per-task lift and lower context                                    | ✅     |                                                                                                                                   |
| Garbage-collected data model (the `gc` canonical option)           | ❌     | The gate `wasm_component_model_gc` exists. The option is refused at translation.                                                  |
| Additional canonical options on the asynchronous built-ins         | 🔒     | `wasm_component_model_more_async_builtins`. The Explainer marks them in development (🚝).                                         |
| Trap messages                                                      | ✅     | Wasmtime's wording. No trap-message expectation is outstanding in the corpora.                                                    |

### Resources

| Feature                                                      | Status | Notes                                                                               |
| ------------------------------------------------------------ | ------ | ----------------------------------------------------------------------------------- |
| `resource.new`, `resource.rep`, `resource.drop`              | ✅     |                                                                                     |
| Synchronous destructors, host-defined and guest-defined      | ✅     |                                                                                     |
| Asynchronous destructors                                     | 🟡     | A `resource.drop async` is accepted. The destructor runs synchronously on the drop. |
| Borrow lifetime tracking                                     | ✅     | A lend must be returned before the call ends, or the call fails.                    |
| Handle transfer between components                           | ✅     | Own moves and borrow lends through the adapter intrinsics.                          |
| One resource identity under several interfaces               | ✅     | A `HostResource` cloned into more than one registration.                            |
| Host-minted handles (`Store::resource_new`, `resource_drop`) | ✅     |                                                                                     |

### Linking, instantiation, and the host API

| Feature                                                                | Status | Notes                                                                           |
| ---------------------------------------------------------------------- | ------ | ------------------------------------------------------------------------------- |
| `Engine`, `EngineConfig`, `Store<T>`                                   | ✅     | The store owns the host data `T` and one scheduler.                             |
| `Component::new` from bytes, import and export introspection           | ✅     |                                                                                 |
| `Linker`, `LinkerInstance`, root and interface namespaces              | ✅     |                                                                                 |
| Semver-aware interface identifiers                                     | ✅     | Wasmtime's compatibility-track rules.                                           |
| Typed host functions (`func_wrap`) and untyped ones (`func_new`)       | ✅     |                                                                                 |
| Host `async` functions (`func_wrap_concurrent`, `func_new_concurrent`) | ✅     | Reachable from a synchronous and from an asynchronous lower.                    |
| Host resources (`resource`, `resource_with`)                           | ✅     |                                                                                 |
| Core module imports (`module`)                                         | ✅     |                                                                                 |
| Export navigation (`Instance::exports`, `ExportInstance`)              | ✅     | Reaches functions nested inside an exported interface.                          |
| Untyped calls (`Func::call` over `Val`) and typed (`TypedFunc::call`)  | ✅     |                                                                                 |
| Concurrent calls (`call_concurrent`, `Store::run_concurrent`)          | ✅     | An `Accessor` reaches the store's host data from a future that borrows nothing. |
| Composition with `wac`                                                 | ✅     |                                                                                 |
| Host binding generation (a `bindgen!` equivalent)                      | ❌     | A design card is filed.                                                         |

### Concurrency: tasks, waitables, and threads

| Feature                                                                           | Status | Notes                                                                                                                                    |
| --------------------------------------------------------------------------------- | ------ | ---------------------------------------------------------------------------------------------------------------------------------------- |
| Cooperative scheduler per `Store`                                                 | ✅     | Guest code runs only inside a turn. The order matches Wasmtime on both targets.                                                          |
| `task.return`                                                                     | ✅     |                                                                                                                                          |
| `task.cancel`                                                                     | ❌     |                                                                                                                                          |
| `backpressure.inc`, `backpressure.dec`                                            | ✅     |                                                                                                                                          |
| `context.get`, `context.set`                                                      | ✅     |                                                                                                                                          |
| `waitable-set.new`, `waitable-set.wait`, `waitable-set.poll`, `waitable-set.drop` | ✅     |                                                                                                                                          |
| `waitable.join`                                                                   | ✅     |                                                                                                                                          |
| `thread.yield`                                                                    | ✅     |                                                                                                                                          |
| The other `thread.*` built-ins (`thread.index`, `thread.suspend`, spawning)       | ❌     | Refused even when `wasm_component_model_threading` is on. The Explainer marks them in development (🧵).                                  |
| Event codes and callback status words                                             | ✅     |                                                                                                                                          |
| Reentrance rules                                                                  | ✅     | No call traps for reentrance. The instance's entry gate is the only serialization.                                                       |
| Trap poisoning of an instance                                                     | ❌     | The rules that decide which trap poisons an instance are not implemented.                                                                |
| Suspending a guest thread (stack switching, JSPI)                                 | ❌     | A blocking built-in runs a nested scheduler turn instead. A wait that only a caller on the stack can release fails with a clear message. |
| Cancellation                                                                      | ❌     | `task.cancel`, `subtask.cancel`, and the cancellation events. A design card is filed.                                                    |

### Subtasks and the asynchronous import

| Feature                                                   | Status | Notes                                                  |
| --------------------------------------------------------- | ------ | ------------------------------------------------------ |
| Subtask records and supertasks                            | ✅     |                                                        |
| `subtask.drop`                                            | ✅     |                                                        |
| `subtask.cancel`                                          | ❌     |                                                        |
| Subtask events (started, returned) through a waitable set | ✅     |                                                        |
| Asynchronous lower of a host `async` function             | ✅     | A pending future becomes a subtask the guest waits on. |
| Synchronous lower of a host `async` function              | ✅     | Blocks the guest thread until the future resolves.     |

### Streams, futures, and error contexts

| Feature                                                                                     | Status | Notes                                                        |
| ------------------------------------------------------------------------------------------- | ------ | ------------------------------------------------------------ |
| `stream.new`, `stream.read`, `stream.write`                                                 | ❌     | The design is written and in review. Implementation is next. |
| `stream.cancel-read`, `stream.cancel-write`, `stream.drop-readable`, `stream.drop-writable` | ❌     |                                                              |
| `future.new`, `future.read`, `future.write`                                                 | ❌     |                                                              |
| `future.cancel-read`, `future.cancel-write`, `future.drop-readable`, `future.drop-writable` | ❌     |                                                              |
| Stream readiness and partial copies                                                         | ❌     |                                                              |
| Transfer of a stream or future end between components                                       | ❌     |                                                              |
| Host-side stream and future types                                                           | ❌     |                                                              |
| `error-context.new`, `error-context.debug-message`, `error-context.drop`                    | ❌     |                                                              |

### Substrate limits

| Feature                                                                                     | Status | Notes                                                                                                   |
| ------------------------------------------------------------------------------------------- | ------ | ------------------------------------------------------------------------------------------------------- |
| Core modules that import or export exception tags                                           | ⛔     | The runtime layer has no tag type.                                                                      |
| GC reference types in core modules (`i31ref`, typed function references, non-nullable refs) | ⛔     | The runtime layer has no such value types.                                                              |
| Wasm Core proposals                                                                         | ⛔     | The host engine's business. The polyfill implements none.                                               |
| A host function called again while it is on the stack                                       | 🟡     | Runs natively. The browser backend refuses the second call and the polyfill reports a structured cause. |

### Toward Component Model 1.0

The [Component Model 1.0 roadmap] announces changes to the Canonical ABI itself:
the lazy ABI, multivalue returns at the C ABI level, an `error-context` in every
`result`, and a GC ABI option. The polyfill does not implement them. Its lift
and lower strategy sits behind one seam so that a second ABI can sit beside the
eager one.

## Conformance

The test suite vendors two `.wast` corpora and runs every file on both targets:
the [Component Model test corpus] and the [Wasmtime component tests]. Every
directive the polyfill does not pass is recorded, with a reason, in
`rust/wasm-component-model-polyfill/tests/corpus/expected-failures.txt`. The
harness fails when a listed directive starts to pass, so the list stays current.

The native progress summary as of 2026-09-22 (`tests conformance` prints the
current one):

| Corpus           | Directives | Passed | Pass % |
| ---------------- | ---------- | ------ | ------ |
| `cm`             | 1126       | 1038   | 92.2   |
| `cm/async`       | 393        | 83     | 21.1   |
| `fixtures`       | 48         | 45     | 93.8   |
| `wasmtime`       | 469        | 431    | 91.9   |
| `wasmtime/async` | 387        | 125    | 32.3   |
| total            | 2423       | 1722   | 71.1   |

The `async` rows hold the total down. Most of what they still exercise is a
stream, a future, cancellation, an `error-context`, the stackful lift, or a
thread built-in, and every later directive in a file whose component is refused
fails as bookkeeping. The browser differs from native by nine directives, all of
them limits of the browser's engine.

## Targets and requirements

- **Native.** Any target Wasmtime 49 supports. The runtime layer's Wasmtime
  backend is the core engine. `tokio` supplies the executor in the tests, but
  the library itself is executor-agnostic.
- **Web.** `wasm32-unknown-unknown` with `wasm-bindgen`. The browser's
  `WebAssembly` API is the core engine. The test suites run in headless Chrome.
- **Rust.** Stable, edition 2024, with the `wasm32-unknown-unknown` target
  installed. The `rust-toolchain.toml` pins the channel.

The consumer of the polyfill is Rust code. There is no JavaScript API.

## Development

The repository is a Nix flake. Enter the shell with `nix develop`. It prints a
menu, and `menu` prints it again. Every build and test goes through a menu
command, never through bare `cargo`. Outside the shell, prefix a command with
`nix develop -c`.

| Command                      | What it does                                               |
| ---------------------------- | ---------------------------------------------------------- |
| `build debug` / `release`    | Build the polyfill crate for both targets.                 |
| `tests native debug`         | Unit and integration tests on the host.                    |
| `tests web debug`            | The same tests in headless Chrome.                         |
| `tests conformance`          | The conformance progress summary for both targets.         |
| `tests smoke native` / `web` | The end-to-end smoke test as a binary or as a served page. |
| `tests all`                  | Every test archive, debug and release, native and web.     |
| `bench native` / `web`       | The benchmark suite on one target.                         |
| `lint`                       | Every check the flake declares (`nix flake check`).        |
| `api list` / `update`        | Print or record the crate's public API snapshot.           |

Nix sees only tracked files. Snapshot or commit before a `tests` or `lint`
command, or the run measures a stale tree.

## Repository layout

| Path                                  | Contents                                                                        |
| ------------------------------------- | ------------------------------------------------------------------------------- |
| `rust/wasm-component-model-polyfill/` | The library crate, its baseline tests, and the conformance harness and corpora. |
| `rust/wcmp-macros/`                   | Procedural macros: a cross-target `#[test]`, `#[bench]`, `wasm!`, `component!`. |
| `rust/wcmp-smoke/`                    | The end-to-end smoke test, one host program for both targets.                   |
| `rust/wcmp-bench/`                    | The benchmark suite, one definition measured on both targets.                   |
| `rust/vendor/`                        | The two patched runtime-layer backends.                                         |
| `project/design/`                     | The Project Design Documents (PDDs), one per design decision.                   |
| `project/kanban/`                     | The project board.                                                              |

## Design documents

Every design decision is recorded as a PDD under `project/design/` before it is
built. Start with `PDD000`, the product overview, `PDD002`, the ecosystem
foundation, and `PDD003`, the compatibility outlook that maps the Component
Model onto the polyfill feature by feature. `PDD018` through `PDD021` design the
concurrency runtime, the callback export, subtasks and the asynchronous import,
and streams and futures.

## License

Licensed under either of the Apache License, Version 2.0 or the MIT license, at
your option.

[Component Model]: https://github.com/WebAssembly/component-model
[Wasm Core]: https://webassembly.github.io/spec/core/
[WASI 0.3]: https://wasi.dev/releases/wasi-p3
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
[`wasmtime-environ`]: https://docs.rs/wasmtime-environ
[`wasm_runtime_layer`]: https://github.com/DouglasDwyer/wasm_runtime_layer
[`wac`]: https://github.com/bytecodealliance/wac
[Explainer]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md
[Binary]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Binary.md
[CanonicalABI]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md
[Concurrency]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md
[Component Model 1.0 roadmap]:
  https://bytecodealliance.org/articles/the-road-to-component-model-1-0
[Component Model test corpus]:
  https://github.com/WebAssembly/component-model/tree/main/test
[Wasmtime component tests]:
  https://github.com/bytecodealliance/wasmtime/tree/main/tests/misc_testsuite/component-model
