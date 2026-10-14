# Wasm Component Model Polyfill

A Rust library that loads, links, instantiates, and calls [WebAssembly
Components][Component Model] on a platform that only implements [Wasm Core]. One
API serves native hosts and web browsers. The polyfill targets the Component
Model as released with [WASI 0.3], including its `async` functions and the task
built-ins that drive them, and its public API mirrors the names and shapes of
[Wasmtime]'s component API.

The project is in active development. The [feature support](#feature-support)
table shows what works today.

[Live demo here].

## How it works

If you are familiar with Wasm but new to the [Component Model], consider
watching this video for a quick (~8 minute) introduction:

[![Wasm Components Explainer](https://img.youtube.com/vi/h04vdcj03Ss/0.jpg)](https://www.youtube.com/watch?v=h04vdcj03Ss)

The polyfill has three layers:

- **Translation.** [`wasmtime-environ`] parses and validates the component
  binary, resolves nested components and aliases, and compiles a fused adapter
  module for every call between two components. The same translator runs on
  every target.
- **Core execution.** The polyfill's own runtime layer, `wcmp-wasm-core`, runs
  the core modules on the backend the host chooses. `wcmp-wasm-core-wasmtime`
  runs them on Wasmtime 49, used only as a core-Wasm engine, and
  `wcmp-wasm-core-web` runs them on the browser's `WebAssembly` JavaScript API.
  The polyfill has no backend of its own: the host hands one to
  `Engine::with_backend`.
- **The Component Model runtime.** Everything above core Wasm is the polyfill's
  own Rust: the Canonical ABI (lift, lower, string transcoding, `post-return`,
  `cabi_realloc`), one handle table per component instance for resources and
  waitables, the adapter intrinsics, and a cooperative scheduler per `Store`
  that runs guest tasks, host `async` functions, and the concurrency built-ins.
  A [suspend provider](#blocking-needs-a-suspend-provider) gives each guest
  thread a stack of its own, so a guest can block in the middle of a call.

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
`wasm-bindgen`, and only the backend it names differs.

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

The smoke test in `rust/wcmp-smoke` walks through the rest: compositions,
resources, streams and futures, guests that block, cancellation, and traps. It
runs natively (`tests smoke native`) and as a browser page (`tests smoke web`).
Open the page to check whether the polyfill works in a given browser.

## Feature support

The table lists the Component Model feature by feature, as the [Explainer], the
[Binary format][Binary], the [Canonical ABI][CanonicalABI], and the [Concurrency
explainer][Concurrency] define them. Every status applies to native hosts and
web browsers alike. A status means:

| Status | Meaning                                                    |
| ------ | ---------------------------------------------------------- |
| ✅     | Implemented and tested                                     |
| 🟡     | Works, with the limits that the notes give                 |
| 🔒     | Off by default. The notes name the `EngineConfig` setting. |
| ❌     | Not implemented yet                                        |
| ⛔     | Out of scope, or not possible in a polyfill                |

The notes use these terms:

- Lift and lower: the two directions of the Canonical ABI. A lift makes a core
  Wasm function into a component function. A lower gives core Wasm code a
  component function that it can call.
- Adapter: core Wasm code that the polyfill generates to carry a call from one
  component to another.
- Built-in: a function that the runtime gives to guest code, such as
  `task.return` or `stream.read`.
- Waitable: a handle that a guest can wait on. A subtask, a stream end, and a
  future end are waitables.
- Suspend provider: the mechanism that pauses a guest thread and resumes it
  later. [Blocking needs a suspend provider](#blocking-needs-a-suspend-provider)
  describes it.
- Nested turns: the fallback when no suspend provider is available. The blocked
  call runs the scheduler inside itself until the call can continue.
- `StackSwitchNeeded`: short for `SchedulerCause::StackSwitchNeeded`, the error
  for a call that can continue only if its stack is set aside.

| Feature                                                                                                                                | Status | Notes                                                                                                                                                                                                                                                                                                                  |
| -------------------------------------------------------------------------------------------------------------------------------------- | ------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Component binary format**                                                                                                            |        |                                                                                                                                                                                                                                                                                                                        |
| Component preamble, sections, imports, exports                                                                                         | ✅     | [`wasmtime-environ`] parses and validates them.                                                                                                                                                                                                                                                                        |
| Nested components and aliases                                                                                                          | ✅     |                                                                                                                                                                                                                                                                                                                        |
| Binary-format validation                                                                                                               | 🟡     | The polyfill loads 20 components that the spec tests require it to reject. Its validator does not yet check the maximum type size, or reject names that differ only in case or hyphens. It also reads a reserved byte as the old `cancellable` flag, as Wasmtime 49 does.                                              |
| [Binary format warts][Binary warts] that the spec plans to change in 1.0                                                               | ✅     | The polyfill reads the current encodings.                                                                                                                                                                                                                                                                              |
| `implements` annotation on plain-named instances                                                                                       | 🔒     | Turn on with `wasm_component_model_implements`.                                                                                                                                                                                                                                                                        |
| Component-level `start` function                                                                                                       | ❌     | The spec has not finished this feature.                                                                                                                                                                                                                                                                                |
| Value imports and exports                                                                                                              | ❌     | The spec has not finished this feature.                                                                                                                                                                                                                                                                                |
| **Type system**                                                                                                                        |        |                                                                                                                                                                                                                                                                                                                        |
| Primitives (`bool`, integers, floats, `char`, `string`)                                                                                | ✅     |                                                                                                                                                                                                                                                                                                                        |
| `record`, `variant`, `enum`, `flags`, `tuple`, `option`, `result`, `list<T>`                                                           | ✅     |                                                                                                                                                                                                                                                                                                                        |
| `map<K, V>`                                                                                                                            | ✅     | The host sees it as `Val::Map`.                                                                                                                                                                                                                                                                                        |
| Fixed-length `list<T, N>`                                                                                                              | ✅     | The host sees it as `Val::FixedLengthList`.                                                                                                                                                                                                                                                                            |
| `own<T>` and `borrow<T>`                                                                                                               | ✅     |                                                                                                                                                                                                                                                                                                                        |
| Resource types, imported and locally defined                                                                                           | ✅     |                                                                                                                                                                                                                                                                                                                        |
| Core module types in imports and exports                                                                                               | ✅     | A host gives a core `Module` to a component with `LinkerInstance::module`. A component can also export a core module.                                                                                                                                                                                                  |
| `async` function types                                                                                                                 | ✅     |                                                                                                                                                                                                                                                                                                                        |
| `stream<T>` and `future<T>` as value types                                                                                             | ✅     | The host sees them as `ValueType::Stream` and `ValueType::Future`, with their payload type.                                                                                                                                                                                                                            |
| `error-context` type                                                                                                                   | 🔒     | Turn on with `wasm_component_model_error_context`. When it is off, `Component::new` fails with `Error::Unsupported`. The host sees the value as `Val::ErrorContext` or `ErrorContext`, and can only pass it on.                                                                                                        |
| Structural type equality                                                                                                               | ✅     | Two type declarations with the same shape give the same `ValueType`.                                                                                                                                                                                                                                                   |
| Subtyping at the host boundary                                                                                                         | ❌     | A host function must have exactly the type of the import it fills. Subtyping between components works.                                                                                                                                                                                                                 |
| **Canonical ABI**                                                                                                                      |        |                                                                                                                                                                                                                                                                                                                        |
| Lift and lower for every supported value type                                                                                          | ✅     |                                                                                                                                                                                                                                                                                                                        |
| `cabi_realloc`, `memory`, parameter and result spill to memory                                                                         | ✅     |                                                                                                                                                                                                                                                                                                                        |
| String encodings `utf8`, `utf16`, `latin1+utf16`                                                                                       | ✅     |                                                                                                                                                                                                                                                                                                                        |
| String transcoding between components                                                                                                  | ✅     | Between every pair of the encodings above.                                                                                                                                                                                                                                                                             |
| `post-return`                                                                                                                          | ✅     |                                                                                                                                                                                                                                                                                                                        |
| 64-bit memories in canonical options                                                                                                   | ✅     | On by default. Turn off with `wasm_component_model_memory64`.                                                                                                                                                                                                                                                          |
| `canon lift async` with a `callback` (stackless)                                                                                       | ✅     |                                                                                                                                                                                                                                                                                                                        |
| `canon lift async` without a `callback` (stackful)                                                                                     | 🔒     | Turn on with `wasm_component_model_async_stackful`. The export runs as the main thread of its task and can block at any point.                                                                                                                                                                                         |
| `canon lower async`                                                                                                                    | ✅     |                                                                                                                                                                                                                                                                                                                        |
| Adapters between components, for every pair of lift and lower                                                                          | ✅     |                                                                                                                                                                                                                                                                                                                        |
| Instance flags (`may_leave`, `may_enter`, backpressure)                                                                                | ✅     |                                                                                                                                                                                                                                                                                                                        |
| One handle table per component instance                                                                                                | ✅     | Resources, waitables, waitable sets, and subtasks share the table.                                                                                                                                                                                                                                                     |
| Per-task lift and lower context                                                                                                        | ✅     |                                                                                                                                                                                                                                                                                                                        |
| Garbage-collected data model (the `gc` canonical option)                                                                               | ❌     | `Component::new` rejects the `gc` option. The `wasm_component_model_gc` setting has no effect yet.                                                                                                                                                                                                                     |
| Additional canonical options on the asynchronous built-ins                                                                             | 🔒     | Turn on with `wasm_component_model_more_async_builtins`.                                                                                                                                                                                                                                                               |
| Copy limit per call                                                                                                                    | ✅     | One call between components can copy at most 128 MiB. A list element counts as 32 bytes and a map entry as 64 bytes. Change the limit with `Store::set_hostcall_fuel`.                                                                                                                                                 |
| **Resources**                                                                                                                          |        |                                                                                                                                                                                                                                                                                                                        |
| `resource.new`, `resource.rep`, `resource.drop`                                                                                        | ✅     |                                                                                                                                                                                                                                                                                                                        |
| Synchronous destructors, host-defined and guest-defined                                                                                | ✅     |                                                                                                                                                                                                                                                                                                                        |
| Asynchronous destructors                                                                                                               | 🟡     | The polyfill accepts `resource.drop async`, but the destructor runs synchronously.                                                                                                                                                                                                                                     |
| Borrow lifetime tracking                                                                                                               | ✅     | A call fails if the component still holds a borrowed handle when the call ends.                                                                                                                                                                                                                                        |
| Handle transfer between components                                                                                                     | ✅     | The adapters move `own` handles and lend `borrow` handles.                                                                                                                                                                                                                                                             |
| One resource identity under several interfaces                                                                                         | ✅     | A host registers clones of one `HostResource` under each interface.                                                                                                                                                                                                                                                    |
| Host-minted handles (`Store::resource_new`, `resource_drop`)                                                                           | ✅     |                                                                                                                                                                                                                                                                                                                        |
| **Linking, instantiation, and the host API**                                                                                           |        |                                                                                                                                                                                                                                                                                                                        |
| `Engine`, `EngineConfig`, `Store<T>`                                                                                                   | ✅     | The store owns the host data `T` and one scheduler.                                                                                                                                                                                                                                                                    |
| `Component::new` from bytes, import and export introspection                                                                           | ✅     |                                                                                                                                                                                                                                                                                                                        |
| `Linker`, `LinkerInstance`, root and interface namespaces                                                                              | ✅     |                                                                                                                                                                                                                                                                                                                        |
| Semver-aware interface identifiers                                                                                                     | ✅     | The spec leaves version matching to the host. The polyfill uses Wasmtime's rules.                                                                                                                                                                                                                                      |
| Typed host functions (`func_wrap`) and untyped ones (`func_new`)                                                                       | ✅     |                                                                                                                                                                                                                                                                                                                        |
| Host `async` functions (`func_wrap_concurrent`, `func_new_concurrent`)                                                                 | ✅     | A guest can call them through a synchronous or an asynchronous lower.                                                                                                                                                                                                                                                  |
| Host resources (`resource`, `resource_with`)                                                                                           | ✅     |                                                                                                                                                                                                                                                                                                                        |
| Core module imports (`module`)                                                                                                         | ✅     |                                                                                                                                                                                                                                                                                                                        |
| Export navigation (`Instance::exports`, `ExportInstance`)                                                                              | ✅     | Finds functions inside an exported interface.                                                                                                                                                                                                                                                                          |
| Untyped calls (`Func::call` over `Val`) and typed (`TypedFunc::call`)                                                                  | ✅     |                                                                                                                                                                                                                                                                                                                        |
| Concurrent calls (`call_concurrent`, `Store::run_concurrent`)                                                                          | ✅     | An `Accessor` lets a future use the host data without a borrow of the store.                                                                                                                                                                                                                                           |
| Traps at the host boundary                                                                                                             | ✅     | A trap ends the call that polls the store and poisons the store.                                                                                                                                                                                                                                                       |
| Cap on the store's live records                                                                                                        | ✅     | A store holds at most 1,000,000 records. One more fails with "resource table has no free keys".                                                                                                                                                                                                                        |
| Composition with `wac`                                                                                                                 | ✅     |                                                                                                                                                                                                                                                                                                                        |
| Host binding generation (a `bindgen!` equivalent)                                                                                      | ❌     | Planned.                                                                                                                                                                                                                                                                                                               |
| **Concurrency: tasks, waitables, and threads**                                                                                         |        |                                                                                                                                                                                                                                                                                                                        |
| Cooperative scheduler per `Store`                                                                                                      | ✅     | The spec does not fix the order of turns. The polyfill uses Wasmtime's order, natively and in the browser.                                                                                                                                                                                                             |
| `task.return`                                                                                                                          | ✅     |                                                                                                                                                                                                                                                                                                                        |
| `task.cancel`                                                                                                                          | ✅     | The task resolves as cancelled. Its threads run until they end.                                                                                                                                                                                                                                                        |
| `backpressure.inc`, `backpressure.dec`                                                                                                 | ✅     |                                                                                                                                                                                                                                                                                                                        |
| `context.get`, `context.set`                                                                                                           | ✅     |                                                                                                                                                                                                                                                                                                                        |
| `waitable-set.new`, `waitable-set.wait`, `waitable-set.poll`, `waitable-set.drop`                                                      | ✅     |                                                                                                                                                                                                                                                                                                                        |
| `waitable.join`                                                                                                                        | ✅     |                                                                                                                                                                                                                                                                                                                        |
| `thread.yield`                                                                                                                         | ✅     |                                                                                                                                                                                                                                                                                                                        |
| `thread.index`, `thread.new-indirect`, `thread.resume-later`                                                                           | 🔒     | Turn on with `wasm_component_model_threading`. They work with or without a suspend provider.                                                                                                                                                                                                                           |
| `thread.suspend`, `thread.suspend-then-resume`, `thread.yield-then-resume`, `thread.suspend-then-promote`, `thread.yield-then-promote` | 🔒     | Turn on with `wasm_component_model_threading`. Without a suspend provider, a pause waits in nested turns. A switch to a thread lower on the same stack fails with `StackSwitchNeeded`.                                                                                                                                 |
| Event codes and callback status words                                                                                                  | ✅     |                                                                                                                                                                                                                                                                                                                        |
| Reentrance rules                                                                                                                       | ✅     | A call into an instance that is already active does not trap. It waits at the entry gate of the instance.                                                                                                                                                                                                              |
| Trap poisoning of an instance                                                                                                          | ✅     | A trap poisons the whole store, not only the instance. See [A trap poisons the store](#a-trap-poisons-the-store).                                                                                                                                                                                                      |
| Suspending a guest thread (stack switching, JSPI)                                                                                      | 🟡     | Needs a [suspend provider](#blocking-needs-a-suspend-provider): stack switching on x86_64 Linux, and JSPI in the browser. Elsewhere, a blocking built-in runs nested turns.                                                                                                                                            |
| Cancellation                                                                                                                           | ✅     | A caller cancels a guest or host callee with `subtask.cancel`. A guest callee confirms with `task.cancel`. The polyfill still honors the `cancellable` immediate, as Wasmtime 49 does. The spec removed it. Without a suspend provider, a synchronous cancel that needs a stack switch fails with `StackSwitchNeeded`. |
| **Subtasks and the asynchronous import**                                                                                               |        |                                                                                                                                                                                                                                                                                                                        |
| Subtask records and supertasks                                                                                                         | ✅     |                                                                                                                                                                                                                                                                                                                        |
| `subtask.drop`                                                                                                                         | ✅     |                                                                                                                                                                                                                                                                                                                        |
| `subtask.cancel`                                                                                                                       | ✅     | The polyfill cancels a host callee in the next scheduler turn by dropping its future, with no other signal. Lent handles come back when the subtask resolves.                                                                                                                                                          |
| Subtask events (started, returned, cancelled) through a waitable set                                                                   | ✅     |                                                                                                                                                                                                                                                                                                                        |
| Asynchronous lower of a host `async` function                                                                                          | ✅     | If the host future is still pending, the guest gets a subtask to wait on.                                                                                                                                                                                                                                              |
| Synchronous lower of a host `async` function                                                                                           | ✅     | With a suspend provider, the guest thread pauses until the future resolves. Without one, the call waits in nested turns, and fails with `StackSwitchNeeded` if the future stays pending.                                                                                                                               |
| **Streams, futures, and error contexts**                                                                                               |        |                                                                                                                                                                                                                                                                                                                        |
| `stream.new`, `stream.read`, `stream.write`                                                                                            | ✅     | Includes partial copies, zero-length copies, and the packed result value.                                                                                                                                                                                                                                              |
| `stream.cancel-read`, `stream.cancel-write`, `stream.drop-readable`, `stream.drop-writable`                                            | ✅     | A cancel reports how much of the copy completed.                                                                                                                                                                                                                                                                       |
| `future.new`, `future.read`, `future.write`                                                                                            | ✅     | Each end can be used one time only.                                                                                                                                                                                                                                                                                    |
| `future.cancel-read`, `future.cancel-write`, `future.drop-readable`, `future.drop-writable`                                            | ✅     |                                                                                                                                                                                                                                                                                                                        |
| Stream readiness, partial copies, and drop notification                                                                                | ✅     | When one end is dropped, the other end gets a notification, idle or waiting.                                                                                                                                                                                                                                           |
| A synchronous copy                                                                                                                     | ✅     | With a suspend provider, the guest thread pauses until the copy completes. Without one, it waits in nested turns, and can fail with a `SchedulerCause`.                                                                                                                                                                |
| Byte copy of a number payload, and the same-instance rule                                                                              | ✅     | A number payload, or no payload, copies as raw bytes. A read and a write in the same instance must use such a payload. This rule of the spec is temporary.                                                                                                                                                             |
| Transfer of a stream or future end between components                                                                                  | ✅     | Ends move through the adapters and through `task.return`.                                                                                                                                                                                                                                                              |
| Host-side stream and future types                                                                                                      | ✅     | A `StreamReader` or `FutureReader` takes data from a `StreamProducer` or `FutureProducer` and pipes it to a `StreamConsumer` or `FutureConsumer`.                                                                                                                                                                      |
| Untyped values (`Val::Stream`, `Val::Future`)                                                                                          | ✅     | The host can close them and convert them to typed values, but cannot read or write them untyped.                                                                                                                                                                                                                       |
| `error-context.new`, `error-context.debug-message`, `error-context.drop`                                                               | 🔒     | Turn on with `wasm_component_model_error_context`. The debug message stays exactly as the guest wrote it.                                                                                                                                                                                                              |
| **Limits of the core runtime**                                                                                                         |        |                                                                                                                                                                                                                                                                                                                        |
| Core modules that import or export exception tags                                                                                      | ✅     | Tags link across core modules and instances. The host cannot make a tag or throw an exception.                                                                                                                                                                                                                         |
| GC reference types in core modules (`i31ref`, typed function references, non-nullable refs)                                            | 🟡     | They run on a backend whose engine supports them, which includes Wasmtime and every current browser. A host cannot instantiate a core module itself if the module imports a type other than a number, `funcref`, or `externref`.                                                                                       |
| Wasm Core proposals                                                                                                                    | ⛔     | The core engine supplies them. The polyfill implements none.                                                                                                                                                                                                                                                           |

### Toward Component Model 1.0

The [Component Model 1.0 roadmap] announces changes to the Canonical ABI itself:
the lazy ABI, multivalue returns at the C ABI level, an `error-context` in every
`result`, and a GC ABI option. The polyfill does not implement them. The
`error-context` value type and its built-ins run today behind their gate, but a
`result` carries one only where its type says so. The lift and lower code sits
behind one interface, so a second ABI can be added beside the current one.

## Differences from Wasmtime

The polyfill mirrors Wasmtime's component API and runs Wasmtime's own component
tests. A host can notice these differences.

### Blocking needs a suspend provider

Wasmtime runs each guest on a fiber, so a guest can always block in the middle
of a call. The polyfill runs on the one stack that its target gives it, so it
needs a suspend provider to pause a guest and resume it later. When the engine
is built, it picks a provider from the features that its backend declares.
`Engine::suspend_provider()` returns the choice:

| Backend                                                                                                          | Provider                           |
| ---------------------------------------------------------------------------------------------------------------- | ---------------------------------- |
| Wasmtime where it serves stack switching and the switch probe passes: x86_64 Linux (tested) and macOS (untested) | Stack switching (`StackSwitching`) |
| The browser backend, in a browser with JSPI (every current browser)                                              | Host suspension (`HostSuspension`) |
| Wasmi, on any platform                                                                                           | Host suspension (`HostSuspension`) |
| Wasmtime on other platforms, the browser backend without JSPI (such as Safari 26), or `suspend_provider(false)`  | None (`None`)                      |

Without a provider, a blocked guest runs the store's other work in nested turns
until it can go on. That serves most blocks. A block that only a stack switch
can serve, such as a synchronous call to a host `async` function whose future is
still pending, fails with `SchedulerCause::StackSwitchNeeded` instead of
hanging. The [conformance](#conformance) table shows what that costs.

With host suspension in the browser, a few rare cases differ from native:

- A host function used as a guest thread's start function, and a guest that
  blocks inside a destructor, a `post-return` function, or a core start
  function, fail. No conformance test reaches either case.
- A guest thread can start inside a guest call that cannot suspend. If the new
  thread traps before it first pauses, the trap has no reason from the engine.
  The browser gives that reason only to a caller that awaits the call, and a
  host function cannot await. One conformance test reaches this case.

### A trap poisons the store

As in Wasmtime, one trap poisons the whole store. Every later call into guest
code fails with "cannot enter component instance", and a host recovers by
building a new store. Work that runs no guest code, such as reading host data or
dropping the store, still works. The polyfill is stricter than Wasmtime in two
ways:

- It refuses to instantiate into a poisoned store, because instantiation runs
  guest start functions.
- It drops all queued guest work and pending host futures at the moment of the
  trap. Wasmtime keeps them for a later `run_concurrent`.

## Conformance

The test suite runs two `.wast` corpora on both targets: the [Component Model
test corpus] and the [Wasmtime component tests]. Every directive the polyfill
does not pass is listed, with a reason, in
`rust/wcmp/tests/corpus/expected-failures.txt` and two overlays beside it, one
for the browser and one for running without a suspend provider. The harness
fails when a listed directive starts to pass or an unlisted one fails, so the
lists stay current.

Directives passed, with pass percentage, from the `tests all` run of 2026-10-13.
The Baseline section of `rust/wcmp/tests/corpus/README.md` gives the same
figures by failure category, and `tests conformance` prints the current numbers.

| Corpus           | Directives | Native, stack switching | Browser, JSPI | Native, no provider | Browser, no provider | Wasmi       |
| ---------------- | ---------- | ----------------------- | ------------- | ------------------- | -------------------- | ----------- |
| `cm`             | 1126       | 1106 (98.2)             | 1105 (98.1)   | 1106 (98.2)         | 1105 (98.1)          | 1092 (97.0) |
| `cm/async`       | 393        | 393 (100.0)             | 393 (100.0)   | 361 (91.9)          | 361 (91.9)           | 393 (100.0) |
| `fixtures`       | 71         | 65 (91.5)               | 65 (91.5)     | 65 (91.5)           | 65 (91.5)            | 65 (91.5)   |
| `wasmtime`       | 469        | 464 (98.9)              | 460 (98.1)    | 464 (98.9)          | 460 (98.1)           | 433 (92.3)  |
| `wasmtime/async` | 387        | 387 (100.0)             | 387 (100.0)   | 363 (93.8)          | 363 (93.8)           | 372 (96.1)  |
| total            | 2446       | 2415 (98.7)             | 2410 (98.5)   | 2359 (96.4)         | 2354 (96.2)          | 2355 (96.3) |

- With a suspend provider, the `async` corpora pass every directive on both
  targets.
- The browser fails five more directives than native. One is V8's wording of a
  validation error, three need memory past 4 GiB, and one is the trap without a
  reason that [Host suspension](#host-suspension) describes
  (`rust/wcmp/tests/corpus/expected-failures.web.txt`).
- Without a provider, the extra failures are blocks that only a stack switch can
  serve, plus later directives in the same file that then meet a poisoned store.
- On Wasmi, the extra failures are components that need a capability Wasmi
  lacks, and the directives that fail after them.
- The two WASI 0.3 HTTP fixtures stop at link, because the harness provides no
  `wasi:http/types` host.

## Targets and requirements

- **Native.** Any target Wasmtime 49 or Wasmi 2.0 supports. The runtime layer's
  Wasmtime backend or its Wasmi backend is the core engine, as the host chooses.
  `tokio` supplies the executor in the tests, but the library itself is
  executor-agnostic. The stack-switching suspend provider runs over Wasmtime,
  tested on x86_64 Linux only, and the host-suspension provider over Wasmi on
  any platform; see
  [Blocking needs a suspend provider](#blocking-needs-a-suspend-provider).
- **Web.** `wasm32-unknown-unknown` with `wasm-bindgen`. The browser's
  `WebAssembly` API is the core engine. The JSPI suspend provider runs in every
  current browser. The test suites run in headless Chrome.
- **Rust.** Stable, edition 2024, with the `wasm32-unknown-unknown` target
  installed. The `rust-toolchain.toml` pins the channel.

The consumer of the polyfill is Rust code. There is no JavaScript API.

## Backends compared

The polyfill runs core Wasm on one of three backends. The host makes one and
hands it to `Engine::with_backend`, as the [quick start](#quick-start) shows.
This section says what each backend can do, where it stops, and which record
holds each fact.

| Backend  | Crate                          | Runs                                                       | Engine                                  |
| -------- | ------------------------------ | ---------------------------------------------------------- | --------------------------------------- |
| Browser  | `rust/wcmp-wasm-core-web`      | In a browser, on `wasm32-unknown-unknown`                  | The browser's `WebAssembly` API         |
| Wasmi    | `rust/wcmp-wasm-core-wasmi`    | Natively. It also builds for `wasm32`, which no lane tests | Wasmi 2.0, an interpreter               |
| Wasmtime | `rust/wcmp-wasm-core-wasmtime` | Natively only                                              | Wasmtime 49, used as a core engine only |

The Wasmtime backend is the control: it runs beside the other two on every test
lane, so that a person can compare their results with Wasmtime's.

### Capabilities

Every backend runs Wasm 2.0. A capability is a Wasm feature above Wasm 2.0, or
host suspension, that a backend declares only where its engine implements it
faithfully. The names come from one fixed list
(`rust/wcmp-wasm-core/src/capability/capability.rs`). A component that needs a
capability the backend does not declare fails at `Component::new` with
`Error::Unsupported` and the name of the capability.

| Capability            | Browser, as a probe finds it     | Wasmi | Wasmtime                         |
| --------------------- | -------------------------------- | ----- | -------------------------------- |
| `multi_memory`        | Yes in Chromium                  | Yes   | Yes                              |
| `memory64`            | Yes in Chromium. No in WebKitGTK | Yes   | Yes                              |
| `tail_call`           | Yes in Chromium                  | Yes   | Yes                              |
| `exceptions`          | Yes in Chromium                  | No    | Yes                              |
| `function_references` | Yes in Chromium                  | No    | Yes                              |
| `gc`                  | Yes in Chromium                  | No    | Yes                              |
| `relaxed_simd`        | Yes in Chromium                  | Yes   | Yes                              |
| `threads`             | Yes in Chromium                  | No    | Yes                              |
| `stack_switching`     | No browser ships it              | No    | Where Wasmtime's compiler has it |
| `host_suspension`     | Where the browser has JSPI       | Yes   | No                               |

Sources: each backend's `src/backend.rs` and crate docs (`src/lib.rs`). The
Chromium column is the test `it_declares_what_chromium_implements` in
`rust/wcmp-wasm-core-web/tests/web.rs`. The `stack_switching` row for browsers
is from `rust/wcmp/src/concurrency/stack_switching_provider.rs`.

The browser backend decides each Wasm row at run time. When `Web::new` runs, it
gives one small module per feature to `WebAssembly.validate`, and it declares
each feature whose module the browser accepts
(`rust/wcmp-wasm-core-web/src/probes.rs`). It declares `host_suspension` where
the browser has both functions of JavaScript Promise Integration (JSPI),
`WebAssembly.Suspending` and `WebAssembly.promising`. A browser that lacks a
feature still loads the backend, which then declares less.

Two browser cases matter today:

- WebKitGTK 2.54.0 ships JavaScriptCore with memory64 off and with multi-memory
  and JSPI on. The polyfill refuses a component with a 64-bit memory there with
  `Unsupported(memory64)` (card `629412`, 2026-10-05).
- In a browser without `multi_memory`, as older Safari releases were
  (`rust/wcmp/tests/baseline_content_security_policy.rs`), the backend copies
  bytes in bulk with JavaScript `TypedArray.set`, which is the slower path. The
  translator then refuses a fused adapter that copies between the memories of
  two components (card `b5a406`).

### Host suspension

Host suspension lets a host function answer "not yet". The engine sets the guest
call aside, and the host resumes it later with the result. The polyfill uses it
to pause a guest thread. The section
[Blocking needs a suspend provider](#blocking-needs-a-suspend-provider) explains
how the engine picks a provider, and what happens without one.

|                              | Browser                                                    | Wasmi                                           | Wasmtime                                                                                                                        |
| ---------------------------- | ---------------------------------------------------------- | ----------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| Mechanism                    | JSPI: `WebAssembly.promising` and `WebAssembly.Suspending` | Wasmi's resumable calls (`call_resumable`)      | None. The polyfill uses stack switching instead, where the backend declares it                                                  |
| Suspend provider             | Host suspension                                            | Host suspension                                 | Stack switching where Wasmtime serves it and the switch probe passes: x86_64 Linux, tested, and macOS, untested. Otherwise none |
| A resumption                 | Runs later, on a microtask                                 | Runs at once, to the next suspension or the end | Not applicable                                                                                                                  |
| Suspended calls in one store | Any number, resumed in any order                           | Any number, resumed in any order                | Not applicable. `Func::call_async` holds the store, so one store has one suspended call                                         |

Sources: the crate docs of each backend, and
`rust/wcmp/src/suspend_provider_kind.rs` for the provider. The Wasmtime crate
docs name x86-64 Linux and macOS as the platforms where it declares
`stack_switching`. The polyfill's switch probe passes on x86_64 Linux.

Known limits:

- **Browser and Wasmi.** A call can suspend only where WebAssembly frames alone
  lie between the start of the resumable call and the host function. Where a
  host frame lies between them, "not yet" traps the guest with `TrapKind::Host`
  (each crate's docs).
- **Browser.** A host function cannot resume a call, because the browser runs
  the resumed stack only after the host function returns. The attempt fails with
  `Error::Backend` (crate docs). A guest thread that starts inside a host
  function and traps before it first pauses loses the reason for the trap: the
  browser reports the reason only to a caller that awaits the call, and a host
  function cannot await. This costs one conformance directive,
  `wasmtime/thread-transparency/reentrancy.wast:114`
  (`rust/wcmp/tests/corpus/expected-failures.web.txt`, card `656c71`).
- **Wasmi.** A start and a resumption both finish inside the poll that begins
  them: each future is ready on its first poll (crate docs). So a long guest
  computation holds the thread that polls until it suspends or ends.

### Memory

| Memory operation                        | Browser                                                                | Wasmi                                   | Wasmtime                                                           |
| --------------------------------------- | ---------------------------------------------------------------------- | --------------------------------------- | ------------------------------------------------------------------ |
| How guest memory is held                | The browser's engine decides                                           | A `Vec` that is zero-filled as it grows | Reserved address space, with only touched pages committed          |
| A scalar load or store                  | A call into a generated accessor module, from Wasm to Wasm             | Direct                                  | Direct                                                             |
| `with_bytes` (lend a range to the host) | Copies the range into a buffer of the polyfill once                    | Lends the memory's own bytes            | Lends the memory's own bytes, or copies a shared memory atomically |
| A bulk copy                             | One `memory.copy` with `multi_memory`. One `TypedArray.set` without it | One `memmove`                           | One `memmove`                                                      |
| A shared memory without `multi_memory`  | Eight bytes per accessor call, each byte atomic                        | Not supported                           | Not applicable                                                     |

Sources: each backend's crate docs, `rust/wcmp-wasm-core-web/src/accessor.rs`,
card `f7eb9b` for the eight-byte batching, and
`rust/wcmp/tests/corpus/README.md` for how Wasmi and Wasmtime hold memory.

What this costs:

- **Wasmi holds every page.** Measured alone in the debug build,
  `wasmtime/memory64.wast` peaks at 34 MiB on Wasmtime and 4112 MiB on Wasmi,
  and `wasmtime/big-strings.wast` at 34 MiB and 5134 MiB
  (`rust/wcmp/tests/corpus/README.md`). The Wasmi lane runs these two files one
  at a time.
- **The browser reaches only 4 GiB.** The polyfill is 32-bit code in the
  browser, and its Canonical ABI addresses guest memory with `usize`. So it
  cannot lift or lower data that lies past 4 GiB in a 64-bit guest memory. This
  costs three directives of `wasmtime/memory64.wast`
  (`rust/wcmp/tests/corpus/expected-failures.web.txt`). Without `multi_memory`,
  a bulk copy also works only within the first 4 GiB of a memory, and fails with
  `Error::Backend` past it (`rust/wcmp-wasm-core-web/src/store.rs`).

### Performance

The numbers below come from the benchmark suite (`rust/wcmp-bench`). Absolute
times depend on the machine. Compare a benchmark with itself before and after a
change, on one machine (`rust/wcmp-bench/README.md`). The baseline `afc71435f`
is the last commit before every browser call went through JSPI.

**The browser's host-suspension path.** When every browser export call became a
JSPI resumable call (card `7eb780`), each call cost more: from about 1 µs for
`u32-call` to about 42 µs for `list-u32/4096`. Card `721eac` recorded these
figures, with the benchmark names as the card gives them:

| Benchmark          | Before (µs) | After the change (µs) |
| ------------------ | ----------- | --------------------- |
| `u32-call`         | 3.333       | 4.381                 |
| `string/64`        | 4.158       | 6.800                 |
| `list-u8-typed/64` | 3.889       | 6.333                 |
| `list-u32/4096`    | 78.0        | 120.5                 |

Card `721eac` then made a call that never suspends end on its first poll, with
no JavaScript closure, no promise handler, and no microtask. Its reviewer
measured every string and list benchmark at 0.875 to 0.997 times the baseline,
for example `string/64` 4.32 → 3.78 µs. Card `f87de5` closed the remaining gaps.
Its reviewer's paired ratios against the baseline: `u32-call-typed` 0.89,
`composition-call` 0.90, `resource-handle` 0.91, `host-calls-in-flight/8` 0.97,
and `host-calls-in-flight/64` 0.99. Native ratios were 0.91 to 1.00. Card
`bf9a7f` records one older gap: `list-u8-roundtrip/65536` is about 1 to 2%
slower than the baseline.

A call that does suspend still pays for a promise and a microtask at each
resumption (crate docs of `rust/wcmp-wasm-core-web`).

**Floats in the browser.** The JavaScript API carries a float as a `Number`,
which can change the bits of a NaN. So the backend calls each export whose type
holds a float through a carrier: a small generated module that passes an `f64`
as the `i64` of its bits, which JavaScript receives as a `BigInt`
(`rust/wcmp-wasm-core-web/src/carrier.rs`). The `f64-call` benchmarks measure
the cost against the `u32` floor of the same shape. Two
`bench web target-sample-ms=50` runs of 2026-10-13 put `u32-call` at 2.04 and
2.06 µs and `f64-call` at 2.22 and 2.18 µs, and `u32-call-typed` at 1.99 and
1.96 µs and `f64-call-typed` at 2.13 and 2.12 µs. So a float call costs about
0.15 µs more in the browser, 6 to 9%. Natively the two are the same:
`bench native` put `u32-call` at 1.36 µs and `f64-call` at 1.35 µs (card
`276004`).

### Fidelity

| Suite                          | Browser                      | Wasmi                 | Wasmtime              |
| ------------------------------ | ---------------------------- | --------------------- | --------------------- |
| Conformance, directives passed | 2410 of 2446 (98.5%)         | 2355 of 2446 (96.3%)  | 2415 of 2446 (98.7%)  |
| Fidelity, script tests passed  | 264, with 83 listed failures | 264, with none listed | 267, with none listed |
| Zena scenarios passed          | 18 of 18                     | 2 of 18               | 18 of 18              |

- **Conformance** is the Component Model and Wasmtime `.wast` corpora under the
  [conformance](#conformance) harness, with the suspend provider on. The figures
  are the Baseline tables of `rust/wcmp/tests/corpus/README.md`, from the
  `tests all` run of 2026-10-13. On Wasmi, the 60 lines of
  `expected-failures.wasmi.txt` are components that need a capability Wasmi
  lacks, and the directives that fail after them.
- **Fidelity** is the WebAssembly specification test suite, run on one backend
  through the runtime layer (`rust/wcmp-wasm-core-fidelity`). The counts are
  from card `c91d3d` (2026-10-01). The Wasmtime file has three more tests, which
  test the runner itself. A script that needs a capability the backend does not
  declare passes without a run, so the same count does not mean the same
  coverage. Each backend lists its expected failures, each with a citation, in
  `tests/fidelity/expected-failures.txt` in its crate. The browser's 83 are V8
  defects and limits that the web embedding requires (see
  [known limits](#known-limits)).
- **Zena** runs programs that the Zena toolchain compiles, as components
  (`rust/wcmp/tests/zena/README.md`). The counts are the passes per subject in
  `rust/wcmp/tests/zena/record.txt`. The report of 2026-10-13 in the README
  gives the same counts. The Wasmtime column is the polyfill over the Wasmtime
  backend (`native` in the record). Wasmtime's own run (`wasmtime`) also passes
  18 of 18. On Wasmi, 16 scenarios stop at `Component::new` with
  `Unsupported(gc)`.

### Known limits

**Engine defects.**

- V8 words two traps, an integer overflow and an invalid conversion to integer,
  with one message. The browser backend reads that trap as `TrapKind::Other`,
  never as a wrong kind. This is 75 of the browser's fidelity failures
  (`rust/wcmp-wasm-core-web/tests/fidelity/expected-failures.txt`, cited to V8's
  `message-template.h`).
- V8 checks the element segment before the array in `array.init_elem`, where the
  specification checks the array first. Two fidelity failures (same file, cited
  to V8's `runtime-wasm.cc`).
- A Wasmi built optimized with debug assertions overflows the native stack on a
  long run (wasmi-labs/wasmi#2039). The Wasmi crate docs give the profile
  setting that avoids it.
- Wasmtime's embedding API cannot carry a continuation reference
  (bytecodealliance/wasmtime#10248). The Wasmtime backend refuses such a
  crossing with `Error::Backend` (crate docs).

**Limits that the web embedding requires.**

- The JavaScript API carries no `v128`, no `exnref`, and no continuation
  reference, and it can change the bits of a NaN. The backend carries `v128`,
  `exnref`, and float values of an export through a generated carrier. Any other
  crossing of `v128` or `exnref`, such as the value of a global or a table
  element of that type, fails with `Error::TypeMismatch` (crate docs).
- The JavaScript API limits a table to 10,000,000 elements and a 64-bit memory
  to 2^37-1 pages. Every browser refuses a module above them. Four fidelity
  failures, cited to the
  [JavaScript API](https://github.com/WebAssembly/spec/blob/608711107b7f1edb13efd57b7d79b49477462d36/document/js-api/index.bs#L2208-L2234).
- An atomic wait on the main thread of a page throws, as `Atomics.wait` does.
  Two fidelity failures, cited to the threads proposal and ECMA-262.
- Chromium refuses a synchronous compile of a module above 8 MB on the main
  thread. `Component::new` compiles asynchronously, so this affects only the
  synchronous compile (crate docs).
- A shared memory needs a page with cross-origin isolation. The test lanes serve
  every page with the headers for it, so no lane shows a page without them
  (crate docs, card `f7eb9b`).

**Missing capabilities.**

- **Wasmi** lacks `gc`, `exceptions`, `function_references`, `threads`, and
  `stack_switching`. For lack of `gc`, 16 of 18 Zena scenarios stop at
  `Component::new` on Wasmi (`rust/wcmp/tests/zena/record.txt`). Together, the
  missing capabilities cost Wasmi 23 conformance directives (16 for
  `exceptions`, 5 for `gc`, and one each for `threads` and
  `function_references`), and 37 more that fail after them
  (`rust/wcmp/tests/corpus/expected-failures.wasmi.txt`).
- **Browser.** No browser ships `stack_switching`. WebKitGTK 2.54.0 ships
  memory64 off (card `629412`). In a browser without `gc`, `thread.new-indirect`
  fails with an internal error when it is called, not with `Unsupported(gc)` at
  `Component::new` (card `10acdb`, open).
- **Wasmtime** does not declare `host_suspension`, and declares
  `stack_switching` only where its compiler has it (crate docs).

**Backend limits.**

- **Wasmi: 64 nested host calls.** Wasmi does not bound a chain of host
  functions that call back into a guest, because each call runs Wasmi again on
  the native stack. The backend allows at most 64 such calls inside one another.
  The 65th traps with `TrapKind::StackOverflow` before the host function runs
  (crate docs, `MAX_HOST_DEPTH` in `rust/wcmp-wasm-core-wasmi/src/store.rs`).
- **Wasmi: a 64-bit memory with the largest maximum.** Wasmi panics when it
  grows a 64-bit memory whose maximum is 2^48 pages (`wasmi_core` 2.0.0,
  `crates/core/src/memory/mod.rs:165`). The backend refuses to make such a
  memory, and refuses a host's growth of one. It cannot stop a guest's
  `memory.grow` on one, and on x86-64 that panic aborts the process. A host that
  runs untrusted memory64 modules on Wasmi checks their maxima itself (crate
  docs).
- **Wasmi: memory.** Every grown page is allocated and zero-filled (see
  [memory](#memory)).
- **Browser: 4 GiB.** See [memory](#memory).
- **Browser: host suspension.** A host function cannot resume a call, and a trap
  early in a thread started inside a host function loses its reason (see
  [host suspension](#host-suspension), and
  [Blocking needs a suspend provider](#blocking-needs-a-suspend-provider) for
  the other rare cases).
- **Wasmtime: no browser.** The Wasmtime backend does not build for the browser.
  On `wasm32` the crate is empty (crate docs).

## Development

The repository is a Nix flake. Enter the shell with `nix develop`. It prints a
menu, and `menu` prints it again. Every build and test goes through a menu
command, never through bare `cargo`. Outside the shell, prefix a command with
`nix develop -c`.

| Command                      | What it does                                                                                   |
| ---------------------------- | ---------------------------------------------------------------------------------------------- |
| `build debug` / `release`    | Build the polyfill crate for both targets.                                                     |
| `tests native debug`         | Unit and integration tests on the host.                                                        |
| `tests web debug`            | The same tests in headless Chrome.                                                             |
| `tests conformance`          | The conformance progress summary for both targets.                                             |
| `tests fidelity <backend>`   | The WebAssembly spec tests on one backend of the runtime layer: `wasmi`, `wasmtime`, or `web`. |
| `tests smoke native` / `web` | The end-to-end smoke test as a binary or as a served page.                                     |
| `tests all`                  | Every test archive, debug and release, native and web.                                         |
| `tests native wasmi`         | The polyfill's tests on the Wasmi backend, from the native debug build.                        |
| `tests zena` / `regenerate`  | The Zena scenarios on every subject, held to `rust/wcmp/tests/zena/record.txt`, or rewrite it. |
| `bench native` / `web`       | The benchmark suite on one target.                                                             |
| `bench wasmi`                | The benchmark suite over the Wasmi backend, with the binary `bench native` builds.             |
| `lint`                       | Every check the flake declares (`nix flake check`).                                            |
| `api list` / `update`        | Print or record the crate's public API snapshot.                                               |

Nix sees only tracked files. Snapshot or commit before a `tests` or `lint`
command, or the run measures a stale tree.

## Repository layout

| Path                    | Contents                                                                              |
| ----------------------- | ------------------------------------------------------------------------------------- |
| `rust/wcmp/`            | The library crate, its baseline tests, and the conformance harness and corpora.       |
| `rust/wcmp-macros/`     | Procedural macros: a cross-target `#[test]`, `#[bench]`, `wasm!`, `component!`.       |
| `rust/wcmp-smoke/`      | The end-to-end smoke test, one host program for both targets.                         |
| `rust/wcmp-bench/`      | The benchmark suite, one definition measured on both targets.                         |
| `rust/wcmp-wasm-core*/` | The runtime layer: its trait, one backend per engine, and the suites that prove them. |
| `project/design/`       | The Project Design Documents (PDDs), one per design decision.                         |
| `project/kanban/`       | The project board.                                                                    |

## Design documents

Every design decision is recorded as a PDD under `project/design/` before it is
built. Start with `PDD000`, the product overview, `PDD002`, the ecosystem
foundation, and `PDD003`, the compatibility outlook that maps the Component
Model onto the polyfill feature by feature. `PDD018` through `PDD021` design the
concurrency runtime, the callback export, subtasks and the asynchronous import,
and streams and futures. `PDD022` designs the suspend providers, the stackful
export, and the thread built-ins, and `PDD023` designs cancellation, error
contexts, and the poisoned store. Both are built.

## Acknowledgements

The runtime layer keeps the shape of the runtime-layer crates by Douglas Dwyer,
which the polyfill ran on, with local patches, until it wrote its own: one trait
over core WebAssembly, and one backend for each engine. That work is prior art,
licensed under either of the Apache License, Version 2.0 or the MIT license. No
code from it remains in this repository.

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
[`wasmtime-environ`]: https://docs.rs/wasmtime-environ
[`wac`]: https://github.com/bytecodealliance/wac
[Explainer]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md
[Binary]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Binary.md
[Binary warts]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Binary.md#binary-format-warts-to-fix-in-a-10-release
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
[Live demo here]: http://dialog-db.github.io/wasm-component-model-polyfill/
