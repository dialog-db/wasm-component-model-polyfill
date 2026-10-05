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

| Backend                                                                                                         | Provider                           |
| --------------------------------------------------------------------------------------------------------------- | ---------------------------------- |
| Wasmtime on x86_64 Linux                                                                                        | Stack switching (`StackSwitching`) |
| The browser backend, in a browser with JSPI (every current browser)                                             | Host suspension (`HostSuspension`) |
| Wasmtime on other platforms, the browser backend without JSPI (such as Safari 26), or `suspend_provider(false)` | None (`None`)                      |

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

Directives passed, with pass percentage, as of 2026-09-27. `tests conformance`
prints the current numbers.

| Corpus           | Directives | Native, stack switching | Browser, JSPI | Native, no provider | Browser, no provider |
| ---------------- | ---------- | ----------------------- | ------------- | ------------------- | -------------------- |
| `cm`             | 1126       | 1096 (97.3)             | 1095 (97.2)   | 1096 (97.3)         | 1095 (97.2)          |
| `cm/async`       | 393        | 393 (100.0)             | 392 (99.7)    | 361 (91.9)          | 360 (91.6)           |
| `fixtures`       | 65         | 59 (90.8)               | 59 (90.8)     | 59 (90.8)           | 59 (90.8)            |
| `wasmtime`       | 469        | 441 (94.0)              | 434 (92.5)    | 441 (94.0)          | 434 (92.5)           |
| `wasmtime/async` | 387        | 387 (100.0)             | 385 (99.5)    | 363 (93.8)          | 361 (93.3)           |
| total            | 2440       | 2376 (97.4)             | 2365 (96.9)   | 2320 (95.1)         | 2309 (94.6)          |

- With a suspend provider, the `async` corpora pass every directive natively and
  all but three in the browser.
- The browser fails eleven more directives than native. In those, V8 fails the
  directive or gives a different trap message.
- Without a provider, the extra failures are blocks that only a stack switch can
  serve, plus later directives in the same file that then meet a poisoned store.
- The two WASI 0.3 HTTP fixtures stop at link, because the harness provides no
  `wasi:http/types` host.

## Targets and requirements

- **Native.** Any target Wasmtime 49 supports. The runtime layer's Wasmtime
  backend is the core engine. `tokio` supplies the executor in the tests, but
  the library itself is executor-agnostic. The stack-switching suspend provider
  runs on x86_64 Linux only; see
  [Blocking needs a suspend provider](#blocking-needs-a-suspend-provider).
- **Web.** `wasm32-unknown-unknown` with `wasm-bindgen`. The browser's
  `WebAssembly` API is the core engine. The JSPI suspend provider runs in every
  current browser. The test suites run in headless Chrome.
- **Rust.** Stable, edition 2024, with the `wasm32-unknown-unknown` target
  installed. The `rust-toolchain.toml` pins the channel.

The consumer of the polyfill is Rust code. There is no JavaScript API.

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
| `bench native` / `web`       | The benchmark suite on one target.                                                             |
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
