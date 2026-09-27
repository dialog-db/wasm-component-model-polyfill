# Wasm Component Model Polyfill

A Rust library that loads, links, instantiates, and calls [WebAssembly
Components][Component Model] on a platform that only implements [Wasm Core]. One
API serves native hosts and web browsers. The polyfill targets the Component
Model as released with [WASI 0.3], including its `async` functions and the task
built-ins that drive them, and its public API mirrors the names and shapes of
[Wasmtime]'s component API.

The project is under active development. The [feature support](#feature-support)
table below says what runs today, what is gated, and what is still to come.

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
  A [suspend provider](#suspend-providers) gives each guest thread a stack of
  its own, so a guest can block where it stands.

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
introspection, a gated feature, awaiting outside the store, streams and futures
between a host and components, and guests that suspend: synchronous code waiting
for an `async` host function, an export that blocks until its answers arrive,
guest threads that park and wake, and the cause each of those three fails with
when suspending is turned off. A chapter on failure and cancellation shows a
guest that cancels a slow host call when its deadline passes, a trap that loses
the store, an error context passed from one component to another through the
host, and a guest thread that stops when its caller cancels. It runs as a native
binary and as a browser page from one source.

## Feature support

The table lists the Component Model feature by feature, as the [Explainer], the
[Binary format][Binary], the [Canonical ABI][CanonicalABI], and the [Concurrency
explainer][Concurrency] define them. "Both targets" means native hosts and web
browsers. A status means:

| Status | Meaning                                                          |
| ------ | ---------------------------------------------------------------- |
| ✅     | Implemented and tested                                           |
| 🟡     | Works with caveats (see notes column)                            |
| 🔒     | Off by default, enabled via `EngineConfig` (see notes column)    |
| ❌     | Not yet implemented; polyfill rejects calls `Error::Unsupported` |
| ⛔     | Out of scope and/or not possible to polyfill                     |

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
  later. [Suspend providers](#suspend-providers) describes it.
- Nested turns: the fallback when no suspend provider is available. The blocked
  call runs the scheduler inside itself until the call can continue.
- `StackSwitchNeeded`: short for `SchedulerCause::StackSwitchNeeded`, the error
  for a call that can continue only if its stack is set aside.

| Feature                                                                                                                                | Status | Notes                                                                                                                                                                                                                                                                                                     |
| -------------------------------------------------------------------------------------------------------------------------------------- | ------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Component binary format**                                                                                                            |        |                                                                                                                                                                                                                                                                                                           |
| Component preamble, sections, imports, exports                                                                                         | ✅     | [`wasmtime-environ`] parses and validates them on every target.                                                                                                                                                                                                                                           |
| Nested components and aliases                                                                                                          | ✅     |                                                                                                                                                                                                                                                                                                           |
| Binary-format validation                                                                                                               | 🟡     | The polyfill accepts 20 test cases in the corpus that the spec rejects. The test harness records them as `validation` failures.                                                                                                                                                                           |
| Binary format warts scheduled for removal in 1.0                                                                                       | ✅     | Accepted, as Wasmtime accepts them.                                                                                                                                                                                                                                                                       |
| `implements` annotation on plain-named instances                                                                                       | 🔒     | Turn on with `wasm_component_model_implements`.                                                                                                                                                                                                                                                           |
| Component-level `start` function                                                                                                       | ❌     | Rejected. The spec marks it as still in development (🪙).                                                                                                                                                                                                                                                 |
| Value imports and exports                                                                                                              | ❌     | Rejected. The spec marks them as still in development (🪙).                                                                                                                                                                                                                                               |
| **Type system**                                                                                                                        |        |                                                                                                                                                                                                                                                                                                           |
| Primitives (`bool`, integers, floats, `char`, `string`)                                                                                | ✅     |                                                                                                                                                                                                                                                                                                           |
| `record`, `variant`, `enum`, `flags`, `tuple`, `option`, `result`, `list<T>`                                                           | ✅     |                                                                                                                                                                                                                                                                                                           |
| `map<K, V>`                                                                                                                            | ✅     | On by default. The host sees it as `Val::Map`.                                                                                                                                                                                                                                                            |
| Fixed-length `list<T, N>`                                                                                                              | ✅     | On by default. The host sees it as `Val::FixedLengthList`.                                                                                                                                                                                                                                                |
| `own<T>` and `borrow<T>`                                                                                                               | ✅     |                                                                                                                                                                                                                                                                                                           |
| Resource types, imported and locally defined                                                                                           | ✅     |                                                                                                                                                                                                                                                                                                           |
| Core module types in imports and exports                                                                                               | ✅     | A host gives a core `Module` to a component with `LinkerInstance::module`. A component can also export a core module.                                                                                                                                                                                     |
| `async` function types                                                                                                                 | ✅     |                                                                                                                                                                                                                                                                                                           |
| `stream<T>` and `future<T>` as value types                                                                                             | ✅     | The host sees them as `ValueType::Stream` and `ValueType::Future`. Each one carries its payload type.                                                                                                                                                                                                     |
| `error-context` type                                                                                                                   | ❌     | The setting `wasm_component_model_error_context` exists, but the polyfill rejects the built-ins that use this type.                                                                                                                                                                                       |
| Structural type equality                                                                                                               | ✅     | Two type declarations with the same shape give the same `ValueType`.                                                                                                                                                                                                                                      |
| Subtyping at the host boundary                                                                                                         | ❌     | A host function must have exactly the type of the import it fills. Inside a component, `wasmtime-environ` applies the subtyping rules of the spec.                                                                                                                                                        |
| **Canonical ABI**                                                                                                                      |        |                                                                                                                                                                                                                                                                                                           |
| Lift and lower for every supported value type                                                                                          | ✅     |                                                                                                                                                                                                                                                                                                           |
| `cabi_realloc`, `memory`, parameter and result spill to memory                                                                         | ✅     |                                                                                                                                                                                                                                                                                                           |
| String encodings `utf8`, `utf16`, `latin1+utf16`                                                                                       | ✅     |                                                                                                                                                                                                                                                                                                           |
| String transcoding between components                                                                                                  | ✅     | Covers every conversion that the adapters use.                                                                                                                                                                                                                                                            |
| `post-return`                                                                                                                          | ✅     |                                                                                                                                                                                                                                                                                                           |
| 64-bit memories in canonical options                                                                                                   | ✅     | On by default (`wasm_component_model_memory64`).                                                                                                                                                                                                                                                          |
| `canon lift async` with a `callback` (stackless)                                                                                       | ✅     |                                                                                                                                                                                                                                                                                                           |
| `canon lift async` without a `callback` (stackful)                                                                                     | 🔒     | Turn on with `wasm_component_model_async_stackful`. The spec marks it as still in development (🚟). The export then runs as the main thread of its task and can block at any point.                                                                                                                       |
| `canon lower async`                                                                                                                    | ✅     | Includes the status code that the call returns, the subtask handle in the caller's handle table, and subtask events.                                                                                                                                                                                      |
| Adapters between components, for every pair of lift and lower                                                                          | ✅     |                                                                                                                                                                                                                                                                                                           |
| Instance flags (`may_leave`, `may_enter`, backpressure)                                                                                | ✅     |                                                                                                                                                                                                                                                                                                           |
| One handle table per component instance                                                                                                | ✅     | Resources, waitables, waitable sets, and subtasks all use this one table.                                                                                                                                                                                                                                 |
| Per-task lift and lower context                                                                                                        | ✅     |                                                                                                                                                                                                                                                                                                           |
| Garbage-collected data model (the `gc` canonical option)                                                                               | ❌     | The setting `wasm_component_model_gc` exists, but `Component::new` rejects the `gc` option.                                                                                                                                                                                                               |
| Additional canonical options on the asynchronous built-ins                                                                             | 🔒     | Turn on with `wasm_component_model_more_async_builtins`. The spec marks them as still in development (🚝).                                                                                                                                                                                                |
| Copy budget (Wasmtime's hostcall fuel)                                                                                                 | ✅     | Each call across a component boundary can copy at most 128 MiB. A list costs 32 bytes per element and a map 64 bytes per entry. The error message is Wasmtime's. Change the limit with `Store::set_hostcall_fuel`.                                                                                        |
| Trap messages                                                                                                                          | ✅     | The same words as Wasmtime. Every trap message that the test corpora expect already matches.                                                                                                                                                                                                              |
| **Resources**                                                                                                                          |        |                                                                                                                                                                                                                                                                                                           |
| `resource.new`, `resource.rep`, `resource.drop`                                                                                        | ✅     |                                                                                                                                                                                                                                                                                                           |
| Synchronous destructors, host-defined and guest-defined                                                                                | ✅     |                                                                                                                                                                                                                                                                                                           |
| Asynchronous destructors                                                                                                               | 🟡     | The polyfill accepts `resource.drop async`, but the destructor runs synchronously during the drop.                                                                                                                                                                                                        |
| Borrow lifetime tracking                                                                                                               | ✅     | A component must give back every handle that it borrowed before the call ends. If it does not, the call fails.                                                                                                                                                                                            |
| Handle transfer between components                                                                                                     | ✅     | The adapters move `own` handles and lend `borrow` handles.                                                                                                                                                                                                                                                |
| One resource identity under several interfaces                                                                                         | ✅     | A host registers clones of one `HostResource` under each interface.                                                                                                                                                                                                                                       |
| Host-minted handles (`Store::resource_new`, `resource_drop`)                                                                           | ✅     |                                                                                                                                                                                                                                                                                                           |
| **Linking, instantiation, and the host API**                                                                                           |        |                                                                                                                                                                                                                                                                                                           |
| `Engine`, `EngineConfig`, `Store<T>`                                                                                                   | ✅     | The store owns the host data `T` and one scheduler.                                                                                                                                                                                                                                                       |
| `Component::new` from bytes, import and export introspection                                                                           | ✅     |                                                                                                                                                                                                                                                                                                           |
| `Linker`, `LinkerInstance`, root and interface namespaces                                                                              | ✅     |                                                                                                                                                                                                                                                                                                           |
| Semver-aware interface identifiers                                                                                                     | ✅     | Follows Wasmtime's rules for which versions are compatible.                                                                                                                                                                                                                                               |
| Typed host functions (`func_wrap`) and untyped ones (`func_new`)                                                                       | ✅     |                                                                                                                                                                                                                                                                                                           |
| Host `async` functions (`func_wrap_concurrent`, `func_new_concurrent`)                                                                 | ✅     | A guest can call them through a synchronous lower or an asynchronous lower.                                                                                                                                                                                                                               |
| Host resources (`resource`, `resource_with`)                                                                                           | ✅     |                                                                                                                                                                                                                                                                                                           |
| Core module imports (`module`)                                                                                                         | ✅     |                                                                                                                                                                                                                                                                                                           |
| Export navigation (`Instance::exports`, `ExportInstance`)                                                                              | ✅     | Finds functions inside an exported interface.                                                                                                                                                                                                                                                             |
| Untyped calls (`Func::call` over `Val`) and typed (`TypedFunc::call`)                                                                  | ✅     |                                                                                                                                                                                                                                                                                                           |
| Concurrent calls (`call_concurrent`, `Store::run_concurrent`)                                                                          | ✅     | An `Accessor` lets a future use the store's host data without holding a borrow of the store.                                                                                                                                                                                                              |
| Composition with `wac`                                                                                                                 | ✅     |                                                                                                                                                                                                                                                                                                           |
| Host binding generation (a `bindgen!` equivalent)                                                                                      | ❌     | Planned. A design card is on the project board.                                                                                                                                                                                                                                                           |
| **Concurrency: tasks, waitables, and threads**                                                                                         |        |                                                                                                                                                                                                                                                                                                           |
| Cooperative scheduler per `Store`                                                                                                      | ✅     | Guest code runs only when the scheduler gives it a turn. The order of turns matches Wasmtime on both targets.                                                                                                                                                                                             |
| `task.return`                                                                                                                          | ✅     |                                                                                                                                                                                                                                                                                                           |
| `task.cancel`                                                                                                                          | 🟡     | `Component::new` accepts it, so a guest that imports it runs normally until it cancels. The call to `task.cancel` then fails with `Error::Unsupported`.                                                                                                                                                   |
| `backpressure.inc`, `backpressure.dec`                                                                                                 | ✅     |                                                                                                                                                                                                                                                                                                           |
| `context.get`, `context.set`                                                                                                           | ✅     |                                                                                                                                                                                                                                                                                                           |
| `waitable-set.new`, `waitable-set.wait`, `waitable-set.poll`, `waitable-set.drop`                                                      | ✅     |                                                                                                                                                                                                                                                                                                           |
| `waitable.join`                                                                                                                        | ✅     |                                                                                                                                                                                                                                                                                                           |
| `thread.yield`                                                                                                                         | ✅     |                                                                                                                                                                                                                                                                                                           |
| `thread.index`, `thread.new-indirect`, `thread.resume-later`                                                                           | 🔒     | Turn on with `wasm_component_model_threading`. The spec marks them as still in development (🧵). They work with or without a suspend provider.                                                                                                                                                            |
| `thread.suspend`, `thread.suspend-then-resume`, `thread.yield-then-resume`, `thread.suspend-then-promote`, `thread.yield-then-promote` | 🔒     | Turn on with `wasm_component_model_threading`. With a suspend provider, each one pauses the current thread or switches to the thread it names. Without a provider, a pause waits in nested turns, and a switch to a thread that paused lower on the same stack fails with `StackSwitchNeeded`.            |
| Event codes and callback status words                                                                                                  | ✅     |                                                                                                                                                                                                                                                                                                           |
| Reentrance rules                                                                                                                       | ✅     | No call traps because it enters an instance again. The entry gate of the instance is the only thing that makes calls wait for each other.                                                                                                                                                                 |
| Trap poisoning of an instance                                                                                                          | ❌     | A poisoned instance is one that a trap made unusable. The rules that decide which traps poison an instance are not implemented.                                                                                                                                                                           |
| Suspending a guest thread (stack switching, JSPI)                                                                                      | 🟡     | Uses the [suspend provider](#suspend-providers) that the engine selects: stack switching natively on x86_64 Linux, and JSPI in the browser. On other platforms, or with the provider off, a blocking built-in runs nested turns.                                                                          |
| Cancellation                                                                                                                           | ❌     | Cancelling a task or subtask, and the cancelled event, are not implemented. `Component::new` accepts the two cancel built-ins, but each one fails when called.                                                                                                                                            |
| **Subtasks and the asynchronous import**                                                                                               |        |                                                                                                                                                                                                                                                                                                           |
| Subtask records and supertasks                                                                                                         | ✅     |                                                                                                                                                                                                                                                                                                           |
| `subtask.drop`                                                                                                                         | ✅     |                                                                                                                                                                                                                                                                                                           |
| `subtask.cancel`                                                                                                                       | 🟡     | The same as `task.cancel`: `Component::new` accepts it, and a call to it fails with `Error::Unsupported`.                                                                                                                                                                                                 |
| Subtask events (started, returned) through a waitable set                                                                              | ✅     |                                                                                                                                                                                                                                                                                                           |
| Asynchronous lower of a host `async` function                                                                                          | ✅     | If the host future is still pending, the guest gets a subtask that it can wait on.                                                                                                                                                                                                                        |
| Synchronous lower of a host `async` function                                                                                           | ✅     | With a suspend provider, the guest thread pauses until the future resolves. Without one (Safari 26, or a native host other than x86_64 Linux), the call waits in nested turns. If the future stays pending, the call fails with `StackSwitchNeeded`.                                                      |
| **Streams, futures, and error contexts**                                                                                               |        |                                                                                                                                                                                                                                                                                                           |
| `stream.new`, `stream.read`, `stream.write`                                                                                            | ✅     | Reads and writes pair up as in the reference implementation of the spec. This includes partial copies, zero-length copies, and the packed result value.                                                                                                                                                   |
| `stream.cancel-read`, `stream.cancel-write`, `stream.drop-readable`, `stream.drop-writable`                                            | ✅     | A cancel reports how much of the copy completed.                                                                                                                                                                                                                                                          |
| `future.new`, `future.read`, `future.write`                                                                                            | ✅     | Each end can be used one time only.                                                                                                                                                                                                                                                                       |
| `future.cancel-read`, `future.cancel-write`, `future.drop-readable`, `future.drop-writable`                                            | ✅     |                                                                                                                                                                                                                                                                                                           |
| Stream readiness, partial copies, and drop notification                                                                                | ✅     | When one end is dropped, the other end gets a notification, whether it is idle or waiting. The current spec and Wasmtime do the same.                                                                                                                                                                     |
| A synchronous copy                                                                                                                     | ✅     | With a suspend provider, the guest thread pauses until the copy completes. Without one (Safari 26, or a native host other than x86_64 Linux), it waits in nested turns. If the store cannot complete the copy that way, the copy fails with one of the causes in [Suspend providers](#suspend-providers). |
| Byte copy of a number payload, and the same-instance rule                                                                              | ✅     | A payload of a number type, or no payload, copies as raw bytes. A read and a write from the same instance must use such a payload. This is a temporary rule of the spec.                                                                                                                                  |
| Transfer of a stream or future end between components                                                                                  | ✅     | Ends move through the adapters and through `task.return`.                                                                                                                                                                                                                                                 |
| Host-side stream and future types                                                                                                      | ✅     | A `StreamReader` or `FutureReader` gets its data from a `StreamProducer` or `FutureProducer` and pipes it to a `StreamConsumer` or `FutureConsumer`. They support `close`, `close_with`, and `guard`. The names match Wasmtime 49.                                                                        |
| Untyped values (`Val::Stream`, `Val::Future`)                                                                                          | ✅     | You can close them and convert them to typed values, as in Wasmtime. Untyped reads and writes will follow when Wasmtime adds them.                                                                                                                                                                        |
| `error-context.new`, `error-context.debug-message`, `error-context.drop`                                                               | ❌     |                                                                                                                                                                                                                                                                                                           |
| **Limits of the core runtime**                                                                                                         |        |                                                                                                                                                                                                                                                                                                           |
| Core modules that import or export exception tags                                                                                      | ⛔     | `wasm_runtime_layer` has no tag type.                                                                                                                                                                                                                                                                     |
| GC reference types in core modules (`i31ref`, typed function references, non-nullable refs)                                            | ⛔     | `wasm_runtime_layer` has no such value types.                                                                                                                                                                                                                                                             |
| Wasm Core proposals                                                                                                                    | ⛔     | The core engine supplies these. The polyfill implements none of them.                                                                                                                                                                                                                                     |

### Toward Component Model 1.0

The [Component Model 1.0 roadmap] announces changes to the Canonical ABI itself:
the lazy ABI, multivalue returns at the C ABI level, an `error-context` in every
`result`, and a GC ABI option. The polyfill does not implement them. Its lift
and lower strategy sits behind one seam so that a second ABI can sit beside the
eager one.

## Suspend providers

A guest thread may block where it stands: in a synchronous call to an `async`
function, the synchronous start of a call into another component, a synchronous
stream or future copy or cancel, `waitable-set.wait`, `thread.yield`, a stackful
export, or a thread built-in that suspends or switches. Wasmtime serves such a
block by switching fibers. The polyfill runs a guest on the one real stack of
its target, so it needs a suspend provider to set that stack aside and resume it
later. The engine selects the provider once, when it is constructed, and
`Engine::suspend_provider()` answers which one it selected, as a
`SuspendProviderKind`:

| Target                                                                                    | Provider                                                        | Answer           |
| ----------------------------------------------------------------------------------------- | --------------------------------------------------------------- | ---------------- |
| Native, on an engine that implements the stack-switching proposal                         | Stack switching. Under Wasmtime 49, x86_64 Linux only.          | `StackSwitching` |
| A browser that ships JSPI (`WebAssembly.Suspending` and `WebAssembly.promising`)          | JavaScript Promise Integration. Every current browser ships it. | `Jspi`           |
| Every other native platform, an older browser such as Safari 26, or a host that opted out | None                                                            | `None`           |

Under a provider, each guest thread starts on a stack of its own, and a blocking
built-in suspends that stack until the scheduler resumes it. The stack-switching
provider resumes a thread synchronously and the JSPI provider resumes it on a
microtask. The scheduler runs nothing else until the thread stops again, so a
guest sees the same order under both.

Without a provider, a blocking built-in runs the waiting work in nested
scheduler turns above the blocked call. That serves every block whose releasing
work the store holds. When the store goes idle under a block, the block fails.
Its cause depends on what could still release it, checked in this order:

- The blocked thread's own instance has a synchronous call in progress, so it
  must not suspend, and no other thread of that instance is ready. The block
  fails with the cannot-block cause, `SchedulerCause::CannotBlock`.
- A frame below it would go on under a stack switch. That frame is either a
  nested start, where a start intrinsic ran an `async`-typed callee inside its
  own frame and the caller of that start would go on, or a thread built-in's
  switch whose switching thread is not suspended. The block fails with the
  stack-switch cause, `SchedulerCause::StackSwitchNeeded`: "blocking here
  requires a stack switch, but this thread cannot switch its stack".
- A caller below it waits for the blocked callee through a synchronous call, in
  an instance that must not suspend and has no other thread ready. The block
  fails with the cannot-block cause.
- A host future that can still resolve is pending. The block fails with the
  stack-switch cause.
- None of these. Nothing left can meet the block's condition, and the block
  fails with `SchedulerCause::Deadlock`: "deadlock detected: event loop cannot
  make further progress".

Nested turns also keep a budget. They count the turns in a row that run nothing,
or nothing but a resumption after a yield, against a store with no host future
that can still resolve. Past the budget, the blocked call fails with the
stack-switch cause, because only a stack switch could reach the frame that would
release it. The budget applies under a provider too, to a thread that runs on
another thread's stack and so waits in nested turns.

A host reads `Engine::suspend_provider()` to explain a stack-switch failure. A
failure raised inside a guest call reaches the host today as an error whose text
carries that message, not as the typed cause.

`EngineConfig::suspend_provider(false)` turns the provider off, and the engine
then answers `None` whatever its probes would find. A host turns it off to keep
the order of nested turns, or to avoid a provider that fails on one engine
version. Wasmtime has no counterpart, because its fibers always exist.

### Target differences

The provider is a difference between the targets. Natively the stack-switching
provider runs only on x86_64 Linux, and every other native platform runs nested
turns. In the browser the JSPI provider runs in every browser that ships JSPI,
and an older browser runs nested turns. Two shapes pass natively and fail in the
browser under the JSPI provider:

- A host function as a thread's entry, such as an imported function that
  `thread.new-indirect` names as its start function. The JSPI provider hands the
  entry to its start as a function reference, and such an entry cannot start
  there.
- A resumption needed while a host frame lies below the suspending thread: in a
  guest destructor, a `post-return` function, or a core start function during
  instantiation. The JSPI provider cannot resume a stack there, and the block
  fails with the stack-switch cause instead.

No test of the corpora reaches either shape.

## Conformance

The test suite vendors two `.wast` corpora and runs every file on both targets:
the [Component Model test corpus] and the [Wasmtime component tests]. Every
directive the polyfill does not pass is recorded, with a reason, in
`rust/wasm-component-model-polyfill/tests/corpus/expected-failures.txt`. The
harness fails when a listed directive starts to pass, or an unlisted one fails,
so the list stays current.

The corpus runs in four states: each target with its suspend provider, and each
target with the provider turned off through `EngineConfig`. `tests all` runs all
four. The shared list records the failures under a provider. Two overlays sit
beside it in the same directory. `expected-failures.web.txt` holds the eleven
directives the browser's engine fails where Wasmtime passes, such as a trap that
V8 words differently. `expected-failures.no-provider.txt` holds the 44
directives that fail only without a provider. Each line's reason names the stack
switch the directive needs, and 36 of them fail with the stack-switch cause.
Five are cascades that fail with "cannot resume thread which is not suspended",
because the thread they resume ended when an earlier directive failed. Three
fail with the deadlock cause, because no nested start and no host task lies
below the block: `cm/async/during-sync-scheduling-candidates.wast` lines 304 and
308, and `wasmtime/async/task-deletion.wast` line 311. The harness applies the
overlay whenever no provider runs the store's guest threads: with the provider
turned off, natively off x86_64 Linux, and in a browser without JSPI.

The progress summary from the `tests all` run of 2026-09-26, as directives
passed and pass percentage (`tests conformance` prints the current tables for
the two provider states):

| Corpus           | Directives | Native, stack switching | Browser, JSPI | Native, no provider | Browser, no provider |
| ---------------- | ---------- | ----------------------- | ------------- | ------------------- | -------------------- |
| `cm`             | 1126       | 1096 (97.3)             | 1095 (97.2)   | 1096 (97.3)         | 1095 (97.2)          |
| `cm/async`       | 393        | 379 (96.4)              | 378 (96.2)    | 351 (89.3)          | 350 (89.1)           |
| `fixtures`       | 59         | 53 (89.8)               | 53 (89.8)     | 53 (89.8)           | 53 (89.8)            |
| `wasmtime`       | 469        | 434 (92.5)              | 427 (91.0)    | 434 (92.5)          | 427 (91.0)           |
| `wasmtime/async` | 387        | 369 (95.3)              | 367 (94.8)    | 353 (91.2)          | 351 (90.7)           |
| total            | 2434       | 2331 (95.8)             | 2320 (95.3)   | 2287 (94.0)         | 2276 (93.5)          |

Under a provider, what the `async` rows still fail on is cancellation, an
`error-context`, or the rules for which trap poisons an instance, and every
later directive in a file whose component is refused fails as bookkeeping. The
two WASI 0.3 handler fixtures stop at link, because the harness provides no
`wasi:http/types`. Without a provider the `async` rows also fail every block
that only a stack switch can serve.

## Targets and requirements

- **Native.** Any target Wasmtime 49 supports. The runtime layer's Wasmtime
  backend is the core engine. `tokio` supplies the executor in the tests, but
  the library itself is executor-agnostic. Guest threads suspend through the
  stack-switching provider on x86_64 Linux. On every other platform a blocking
  built-in runs nested turns instead.
- **Web.** `wasm32-unknown-unknown` with `wasm-bindgen`. The browser's
  `WebAssembly` API is the core engine. Guest threads suspend through JSPI,
  which every current browser ships. In an older browser a blocking built-in
  runs nested turns instead. The test suites run in headless Chrome.
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
and streams and futures. `PDD022` designs the suspend providers, the stackful
export, and the thread built-ins, all of which are built.

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
