# Runtime Layer

The polyfill runs core WebAssembly through a runtime layer. [PDD002] made an
upstream crate, [`wasm_runtime_layer`], the runtime layer. It asked the polyfill
to extend that crate upstream before it changed anything locally. The polyfill
now owns its runtime layer. This document designs it.

The upstream crate does not meet the needs of the polyfill. Its type model has
no tags and no reference types other than `funcref` and `externref` ([upstream
types]). Its browser backend has no asynchronous compilation, no JavaScript
Promise Integration (JSPI), and no way to enter a host function a second time
while a first call runs. The polyfill carried local patches for each of these.
The patched backends reach a consumer of the polyfill only through a `[patch]`
section, which Cargo applies in the root workspace alone. A module that the
polyfill must run, such as each module that the Zena compiler emits, fails to
load for reasons that have no effect on the component.

The polyfill keeps the shape of the upstream crate: one trait over core
WebAssembly, and one backend for each engine. It writes every line new, from the
practices of this project. The upstream crate is prior art. The polyfill reads
it and does not copy it.

Eleven terms recur:

- The runtime layer is the family of crates that runs core WebAssembly for the
  polyfill.
- An engine is an implementation of Wasm Core, such as the browser's engine,
  Wasmi, or Wasmtime.
- A backend is one crate of the family that implements the runtime layer over
  one engine.
- The host is the Rust code that calls the runtime layer. The polyfill is one
  host.
- The floor is the set of Wasm features that every backend must implement.
- A capability is a Wasm feature above the floor that a backend can declare.
- The lexicon is the fixed list of capability names.
- The boundary of a module is the set of its imports and exports.
- A generated module is a small core module that a backend builds in memory at
  run time.
- The control is the Wasmtime backend. It runs beside the other backends so that
  a person can compare their results.
- The faithfulness suite is the official WebAssembly specification test suite
  ([spec tests]) at a pinned revision, run on one backend.

## Goals

- The polyfill owns the runtime layer, and it evolves with the polyfill.
- The runtime layer is one trait over Wasm Core, with one crate for each
  backend. A person can add a backend for another engine without a change to the
  polyfill.
- Three backends exist: the browser, Wasmi, and Wasmtime as the control.
- The host selects the backend at run time, on every target. The types of the
  polyfill do not name a backend.
- The floor is Wasm 2.0. Every other Wasm feature is a capability, and a backend
  declares only what it implements faithfully.
- The polyfill derives the Wasm features of its translator from the capabilities
  of the backend. A component that needs a missing capability fails with a
  structured error that names the capability.
- The runtime layer describes the full Wasm 3.0 type model at the boundary of a
  module. Nothing inside a module is a reason to refuse it.
- Tags are an extern kind. A module that exports a tag loads on every backend
  whose engine accepts the module.
- Compilation and instantiation are asynchronous on every backend.
- One primitive, host suspension, gives the scheduler a way to suspend a call
  where the engine has one.
- Memory access is sound for shared memory, copies nothing natively where it
  reads, and crosses into JavaScript at most once for each region in the
  browser.
- A trap has one structured kind with one message on every backend. A host error
  is a trap that no guest can catch.
- The capability lexicon reserves names for fuel, epoch interruption, and
  resource limits.
- The runtime layer and the polyfill build as dependencies of an outside crate
  with no `[patch]` section.
- The polyfill moves to the runtime layer in steps, with no regression in the
  conformance record or the Zena record at any step.

## Non-goals

- A general facade for every engine. A backend exists when the polyfill has a
  reason to run on its engine.
- An implementation of a Wasm Core feature inside the runtime layer. The engine
  supplies every instruction. A backend never rewrites a module to add a
  feature, such as fuel counters.
- Parallel execution. The runtime layer does not run guest code on two host
  threads against one store or one shared memory.
- Tags that the host creates, exceptions that the host throws into a guest, and
  a payload that the host reads from a caught exception.
- Access to the fields of a GC object from the host.
- The configuration of fuel, epoch interruption, and resource limits. This
  design reserves their names only.
- Wasmi in the browser as a supported configuration. The design permits it. No
  test lane runs it.
- The rename of the polyfill's own crate.
- The release process: crate ownership, change logs, and release cadence.

## Facts This Design Rests On

Each fact below was read from the cited source. Wasmtime facts are from
`v49.0.0-rc.1`. Wasmi facts are from its `main` branch at version 2.0.0.

- Wasm 3.0 is complete. It adds GC, exception handling, typed function
  references, tail calls, memory64, multi-memory, and relaxed SIMD to Wasm 2.0
  ([Wasm 3.0]).
- Wasmi implements tail calls, SIMD, relaxed SIMD, memory64, and multi-memory.
  It does not implement GC, exception handling, typed function references, or
  threads ([Wasmi]).
- Safari does not implement multi-memory. Chrome implements it from version 120,
  and Firefox from version 125. Safari has memory64 only behind a flag ([feature
  status]).
- Wasmtime's fused adapters wrap their body in an exception barrier only when
  the translator's features include exception handling ([Wasmtime exception
  barrier]).
- The JavaScript API makes a Wasm GC object opaque. A JavaScript wrapper "does
  not support inspecting or modifying the value in any way" ([GC JS API]).
- A `try_table` catches a JavaScript exception that an imported function throws.
  It never catches a trap ([exception handling]).
- The JavaScript API gives a trap as an error object with a message that each
  engine words in its own way. It gives no trap code.
- Chromium refuses a synchronous `new WebAssembly.Module` on the main thread for
  a module above 8 MB. It refuses a synchronous `new WebAssembly.Instance` of
  such a module too. This repository measured the second refusal in Chromium.
- A page can create a shared `WebAssembly.Memory` without cross-origin
  isolation. Only a transfer of it to another worker needs cross-origin
  isolation ([SharedArrayBuffer]).
- Rust on `wasm32` addresses only memory 0. The intrinsic `memory_size` aborts
  for any other index ([Rust memory_size]).
- Wasmtime lends the bytes of an unshared memory as a slice, `Memory::data`
  ([Wasmtime memory data]). It lends a shared memory only as
  `&[UnsafeCell<u8>]`, which "must be accessed safely through the `Atomic*`
  types" ([Wasmtime shared memory data]).
- Wasmtime's component runtime accepts a shared memory as the memory of a
  canonical ABI option ([Wasmtime shared canonical memory]).
- Wasmtime lifts a string and a list of numbers without a copy.
  `WasmStr::to_str` borrows from guest memory, and so does
  `WasmList::as_le_slice` ([Wasmtime WasmStr], [Wasmtime WasmList]).
- Wasmtime's trap enum names each core trap, with a fixed message ([Wasmtime
  traps]). Wasmi's `TrapCode` uses the same names ([Wasmi traps]).
- Wasmtime reports an exception that reaches the host uncaught as
  `ThrownException` ([Wasmtime ThrownException]).
- Wasmi suspends a call when a host function returns an error during a resumable
  call. The call hands back a `ResumableCallHostTrap`, which owns its own stack.
  The host resumes it with the results of the host function ([Wasmi resumable]).
- Wasmtime's `Func::call_async` borrows the store mutably until the call
  finishes. So one store has at most one suspended call at a time ([PDD022],
  Non-goals).
- A Zena program that uses exceptions exports the tag `__zena_exception` and
  defines a mutable global of a GC reference type. The component that wraps the
  program aliases only `realloc`, the memory, and the exported functions of the
  core instance. It aliases neither the tag nor the global. `wasm-tools print`
  of a component built by Zena at revision `b2237f7` shows this.
- The upstream browser backend reaches `todo!()` for a tag import or export
  ([upstream browser tags]). It refuses every global of a reference type other
  than `funcref` or `externref`, internal or not ([upstream browser refs]). The
  upstream Wasmtime backend refuses a tag at the boundary ([upstream Wasmtime
  tags]).

## The Runtime Layer

The runtime layer is four crates in this repository. They share the workspace
version, so they release together.

| Crate                     | Role                                                                       |
| ------------------------- | -------------------------------------------------------------------------- |
| `wcmp-wasm-core`          | The trait, the types, the lexicon, the trap kinds, and the errors.         |
| `wcmp-wasm-core-web`      | The browser backend, over the WebAssembly JavaScript API.                  |
| `wcmp-wasm-core-wasmi`    | The Wasmi backend. It is the practical native backend.                     |
| `wcmp-wasm-core-wasmtime` | The Wasmtime backend. It is the control, and it runs beside the other two. |

The polyfill depends on `wcmp-wasm-core` alone. A host adds the crate of the
backend it wants. It gives the backend to the polyfill when it makes an engine:

```text
engine = Engine::with_backend(Wasmi::default())      // native
engine = Engine::with_backend(Web::default())        // browser
```

The polyfill has no default backend on any target. The choice is always
explicit, and the same on every target. A backend crate builds for every target
that its engine builds for. For example, Wasmi builds for `wasm32`, so a page
can run the polyfill over Wasmi. This design does not test that configuration.

The polyfill holds the backend behind dynamic dispatch. Its public types do not
carry a type parameter for the backend. One binary can hold two backends at
once, and a test can run one guest on Wasmi and on Wasmtime side by side.

The types of the runtime layer follow Wasmtime's core API in name and in
ownership. They are `Engine`, `Store`, `Module`, `Instance`, `Func`, `Memory`,
`Global`, `Table`, `Tag`, `Extern`, and `Val`. Where this design leaves a choice
open, Wasmtime's choice decides. The runtime layer has no linker. An
instantiation takes the imports of a module as an ordered list of externs, as
`wasmtime::Instance::new` does.

The rule of [PDD002] stays. No public signature of the polyfill names a type of
the runtime layer.

## The Contract

### The Floor

Every backend must implement Wasm 2.0 fully ([Wasm 2.0]). A backend that does
not is not a backend. The floor is Wasm 2.0 and not Wasm 3.0, because Wasmi and
Safari each lack parts of Wasm 3.0, and both are engines that the polyfill must
serve.

### Capabilities

A backend reports its capabilities as a value, from the lexicon. The value is
fixed for the life of an engine. The same binary runs in browsers that differ.
So the browser backend runs a small probe for each feature when it makes an
engine.

The lexicon has these names:

| Name                  | Feature                                      |
| --------------------- | -------------------------------------------- |
| `multi_memory`        | Multi-memory                                 |
| `memory64`            | Memory64                                     |
| `tail_call`           | Tail calls                                   |
| `exceptions`          | Exception handling with `exnref`             |
| `function_references` | Typed function references                    |
| `gc`                  | Garbage collection                           |
| `relaxed_simd`        | Relaxed SIMD                                 |
| `threads`             | Threads and shared memory                    |
| `stack_switching`     | Stack switching                              |
| `host_suspension`     | The host suspension primitive of this design |
| `fuel`                | Reserved                                     |
| `epoch_interruption`  | Reserved                                     |
| `resource_limits`     | Reserved                                     |

Each name for a Wasm feature matches the name of the same feature in
`wasmparser`'s feature set. The lexicon is a value and not a set of marker
traits, for two reasons. The browser finds its capabilities only at run time.
Also, the polyfill must branch on a capability in generic code, and Rust cannot
branch on a trait bound.

A method that needs a capability exists on every backend. A backend that lacks
the capability returns `Unsupported` with the name of the capability.

### How the Polyfill Uses Capabilities

The polyfill sets the Wasm features of its translator from the capabilities of
the backend. For example, over Wasmi the translator does not enable exception
handling, so the fused adapters emit no exception barrier, and Wasmi can compile
them.

When a component needs a capability that the backend lacks, `Component::new`
fails with `Unsupported` and the name of the capability. The failure comes
before any core module compiles. Two examples:

- A component whose core module uses GC, on Wasmi, fails with `gc`.
- A composition whose fused adapter reads two memories, in Safari, fails with
  `multi_memory`.

### Faithfulness

A backend declares a capability only when its engine implements the feature
faithfully. The faithfulness suite proves it. Each backend runs the suite for
the floor and for each capability it declares. A failure is a defect of the
backend or of its engine, and never a behavior the runtime layer hides.

The expected failures of a backend in the faithfulness suite are a list. Each
entry cites a defect of the engine. An entry without a citation is not allowed.
If a feature has failures that no engine defect explains, the backend does not
declare that capability.

## Types at the Boundary

### The Type Model

The runtime layer describes the whole Wasm 3.0 type model:

```text
ValType  = i32 | i64 | f32 | f64 | v128 | Ref(RefType)
RefType  = { nullable: bool, heap: HeapType }
HeapType = func | extern | any | eq | i31 | struct | array | exn | cont
         | nofunc | noextern | none | noexn | nocont
         | Concrete(TypeHandle)
```

A `TypeHandle` is opaque. The host can print it, and can compare two handles
from one engine. The engine checks subtypes when it links a module. The runtime
layer does not.

### Only the Boundary

The runtime layer describes the boundary of a module and nothing else. The
types, globals, tables, and tags inside a module belong to the engine. A module
is never refused because of an item that does not cross its boundary. The engine
decides whether it compiles the module.

A Zena program shows the rule. It defines a global of a GC reference type and
exports a tag. The component aliases neither. The runtime layer loads the module
on every backend whose engine implements GC and exception handling. It needs no
model of the global's value to do so.

### References the Host Can Read

The host can read three kinds of reference:

- A `funcref`. The host can call it, and read its signature where the engine
  knows it.
- An `externref`. The host made it, and the host can read it.
- An `i31ref`. The host can read its integer.

All three work on every backend.

### References the Host Can Only Pass Through

Three kinds of reference are opaque to the host. They are a GC object (`struct`,
`array`, or an `any` or `eq` that holds one), an `exnref`, and a continuation.
The host can hold one, test it for null, and give it back to a guest in the same
store. The host cannot read its fields.

The browser sets this limit. The JavaScript API makes a GC object opaque, so the
browser backend cannot read a field. A read of fields on native backends alone
breaks the floor.

### Tags

A tag is a fourth extern kind, beside functions, memories, globals, and tables.
The host can import a tag, export a tag, link a tag from one instance to
another, and read the parameter types of a tag. The host cannot make a tag or
throw an exception.

### Shared Memory

A module that declares a shared memory loads and runs on a backend that declares
`threads`. The type model names a shared memory. The host can call such a
module, read and write its memory, and pass its memory to a canonical ABI
option, as Wasmtime's component runtime does. All of this happens on one host
thread.

The runtime layer does not run guest code on two host threads against one shared
memory. That is parallel execution, and it is a non-goal for three reasons:

- Parallel execution needs a store that host threads share. Every type of the
  runtime layer and of the polyfill assumes one owner at a time.
- In the browser, each host thread is a Web Worker. A `WebAssembly.Instance`
  cannot move between workers. Only a memory can, and only on a page with
  cross-origin isolation. Each worker needs its own instance of each module.
- The Component Model's design for shared-everything threads is an early
  proposal. The polyfill's own guest threads are cooperative, and they do not
  need parallel execution.

Single-threaded shared memory still constrains memory access. Another agent,
such as a worker that received the memory, can write a shared memory at any
time. A Rust slice over bytes that another agent writes is a data race. For that
reason, the runtime layer never lends a shared memory as a slice. Memory Access
below states the rule.

In the browser, a page makes a shared memory without cross-origin isolation. The
browser backend needs no special page headers for this goal.

## Compilation and Instantiation

Compilation and instantiation are asynchronous on every backend:

```text
module   = Module::compile(&engine, bytes).await
instance = Instance::instantiate(&mut store, &module, imports).await
```

The browser backend uses `WebAssembly.compile` and `WebAssembly.instantiate`.
Wasmi and Wasmtime finish at once. A module above the synchronous limit of the
browser compiles and instantiates in the browser.

A synchronous compile also exists, for small modules that a backend or the
polyfill generates, such as a probe. It keeps the making of an engine
synchronous. In the browser, a synchronous compile of a module above the limit
of the browser fails with a structured error.

Each compile gives its caller its own module. The engine keeps no cache of
modules by their bytes.

## Host Functions

A host function is a Rust closure that a guest imports. The runtime layer holds
four rules for it on every backend:

- The closure is `Fn`, not `FnMut`. The engine can enter it again while an
  earlier call of it runs, at any depth.
- Each call has its own arguments and its own results. No call shares a buffer
  with another call.
- The closure receives a context that reaches the store, as Wasmtime's `Caller`
  does.
- A host function that returns an error traps the guest. No guest can catch the
  trap.

The browser needs a generated module for the last rule. A JavaScript function
that throws into a guest throws an exception, and a guest's `catch_all` catches
it. The fused adapters of the Component Model catch every exception. A trap is
different, because no guest catches a trap. For each host function, the browser
backend puts a generated wrapper between the guest and the JavaScript function:

```text
wrapper for host function f (generated module):
    status = call $f_js(args...)      ;; the JavaScript function returns a flag
    if status == error: unreachable   ;; a trap, which no guest can catch
    return results
```

The JavaScript function never throws. When the host function fails, the backend
keeps the error of the host function, and the wrapper traps. The trap reaches
the host as `Host(error)`, the host's own error. The wrapper is WebAssembly, so
it does not break JSPI, which allows only WebAssembly frames between the start
of a stack and a suspension.

A call of a guest function is untyped. The host passes the arguments as a slice
of values, and a slice for the results.

## Host Suspension

Host suspension is the one primitive that the runtime layer gives for a call
that waits. It has two parts:

- A suspending host function is an import whose body can answer "not yet"
  instead of its results.
- A resumable call is a call of a guest function that ends in one of two ways:
  `Finished`, with the results, or `Suspended`, with a handle.

The host resumes a handle later with the results of the suspending host
function. The call then runs on, and it ends again in one of the two ways.

```text
outcome = func.call_resumable(&mut store, args, results)
match outcome:
    Finished:          read results
    Suspended(handle): ... later ...
                       outcome = handle.resume(&mut store, import_results, results)
```

A backend that declares `host_suspension` holds this contract:

- A suspension succeeds only when WebAssembly frames alone lie between the start
  of the resumable call and the suspending host function. Otherwise, the call
  traps.
- Any number of handles can wait at once in one store, and the host can resume
  them in any order.
- A handle does not hold the store. It borrows the store only while it resumes.
- When a store drops, its waiting handles drop without a resumption. A
  resumption that started before the drop runs to its next suspension or its end
  first. The backend keeps the state of the store alive until then. Under JSPI,
  such a resumption runs on a microtask after the host code that started it
  returned.

Two engines have this primitive:

- The browser backend fills it with JSPI. A suspending host function is an
  import made with `WebAssembly.Suspending`. A resumable call goes through
  `WebAssembly.promising`. The backend declares `host_suspension` when both
  functions exist.
- The Wasmi backend fills it with Wasmi's resumable calls. A suspending host
  function returns a marker error. The resumable call is `call_resumable`, and
  the handle is `ResumableCallHostTrap`. The faithfulness tests of the backend
  must prove the contract before the backend declares `host_suspension`.

The Wasmtime backend does not declare `host_suspension`. One store under
`Func::call_async` has at most one suspended call. Natively, Wasmtime suspends
through `stack_switching` instead, which is plain WebAssembly. For that
capability, the runtime layer only turns the feature on in the engine and lets
continuation types through.

The scheduler of the polyfill uses host suspension as a provider of its suspend
capability, the host-suspension provider of [PDD022]. The provider, the switch
module, and the scheduler stay in the polyfill, because they belong to the
Component Model. The runtime layer supplies the mechanism of the engine.

## Memory Access

### The Methods

A memory has these methods:

```text
memory.size(&store)                                 -> bytes
memory.grow(&mut store, pages)                      -> old size in pages
memory.read(&store, offset, &mut buffer)            -> Result
memory.write(&mut store, offset, &buffer)           -> Result
memory.load_u32(&store, offset)                     -> Result<u32>   // also u8, u16, u64
memory.store_u32(&mut store, offset, value)         -> Result        // also u8, u16, u64
memory.with_bytes(&store, offset, len, |bytes| ...) -> Result<R>
Memory::copy(&mut store, source, source_offset,
             destination, destination_offset, len)  -> Result
```

An offset is a 64-bit number, so a memory64 memory uses the same methods. Each
method checks its range against the size of the memory. A range outside the
memory is a structured error, never a panic and never an abort.

`with_bytes` lends a range of the memory to a closure as `&[u8]`. The store
stays borrowed for the length of the closure, so nothing can grow or write the
memory while the closure runs. `copy` moves bytes from one guest memory to
another in the same store, without a buffer on the host.

### What Each Backend Does

On Wasmi and Wasmtime, `with_bytes` lends the bytes of the memory itself. The
read copies nothing.

The browser cannot lend guest bytes to Rust. The polyfill runs in its own
instance, and Rust addresses only memory 0 of that instance. A guest memory is
another `WebAssembly.Memory`. So `with_bytes` in the browser copies the range
into a buffer of the polyfill once, and lends that buffer. The meaning is the
same on every backend. Only the cost differs.

The browser backend reaches guest memory through a generated module, not through
JavaScript. A generated accessor module imports the guest memory and exports the
scalar loads and stores. The backend puts those exports in the function table of
the polyfill's own instance. Rust then calls them through a function pointer,
which is a call from WebAssembly to WebAssembly. No JavaScript frame runs. The
requirement is the absence of a JavaScript frame. Engineering planning proves
the mechanism.

A bulk copy (`read`, `write`, `with_bytes`, and `copy`) needs two memories in
one module: the guest memory and the memory of the polyfill, or two guest
memories. Where the browser declares `multi_memory`, the accessor module imports
both, and a bulk copy is one `memory.copy`. Where the browser lacks it, a bulk
copy is one call of JavaScript `TypedArray.set`. So in every browser, a scalar
access never crosses into JavaScript, and a bulk copy crosses at most once.

### Shared Memory

The runtime layer never lends a shared memory as a slice. `with_bytes` on a
shared memory always copies, with atomic reads of the bytes, and lends the copy.
`read` and `write` on a shared memory use atomic access too. Wasmtime sets the
same rule for its own shared memory.

### Why Not Lend a Mutable Slice

A host writes guest memory when it lowers a value. It builds the bytes first,
and then writes them in one call. A mutable slice saves nothing in that path,
and the browser cannot lend one. So the runtime layer has `write` and no mutable
view.

### The Measure

The benchmark suite of the repository measures memory access on each backend.
Its string and list benchmarks show the cost of each lift and each lower. The
polyfill's canonical ABI context counts the accesses that one crossing makes.
That count stays the same for a list of any length.

## Traps and Errors

### Trap Kinds

A trap has one structured kind, `TrapKind`, on every backend. Its names are
Wasmtime's names for the core traps:

```text
UnreachableCodeReached   MemoryOutOfBounds        TableOutOfBounds
IndirectCallToNull       BadSignature             IntegerOverflow
IntegerDivisionByZero    BadConversionToInteger   StackOverflow
NullReference            ArrayOutOfBounds         AllocationTooLarge
CastFailure              UnhandledTag             ContinuationAlreadyConsumed
HeapMisaligned           AtomicWaitNonSharedMemory
OutOfFuel                Interrupt                                   // reserved
Host(error)              UncaughtException(exnref)                   Other(message)
```

The message of each kind is Wasmtime's message for the same trap. The polyfill
reports one message for one trap on every backend. The kinds that Wasmtime has
for the Component Model are not in the runtime layer. The polyfill owns them.

`Host(error)` carries the error that a host function returned. An exception that
no guest catches reaches the host as `UncaughtException`. The kind carries the
`exnref` as an opaque reference, and its message is `thrown Wasm exception`, as
Wasmtime's `ThrownException` is.

### Traps in the Browser

The browser gives a trap as an error object, with a message that each engine
words in its own way. The browser backend maps the messages of V8, SpiderMonkey,
and JavaScriptCore to a `TrapKind` with a table. A message that is not in the
table becomes `Other`, with the message of the engine. It never becomes a wrong
kind.

### Errors

The runtime layer returns structured errors, never a panic:

- `Unsupported`, with a capability name, for a feature the backend lacks.
- A compile error, with the message of the engine, for a module that the engine
  refuses.
- A link error for an import of the wrong kind or type.
- A trap, with its `TrapKind`.
- An out-of-bounds error for a memory access outside the memory.

## Reserved Names

Wasmtime has fuel, epoch interruption, and a limiter for memory and table
growth. Wasmi has fuel and a limiter, and its fuel can suspend a resumable call
instead of trapping it. The WebAssembly JavaScript API has none of these.

This design reserves the names and designs nothing else:

- The lexicon has `fuel`, `epoch_interruption`, and `resource_limits`. The
  browser backend declares none of them.
- `TrapKind` has `OutOfFuel` and `Interrupt`.
- A resumable call can end in a third way later, `OutOfFuel`, beside `Finished`
  and `Suspended`. The outcome type leaves room for it.

## The Browser Backend

The browser backend holds four more rules:

- It reaches WebAssembly through generated modules: the accessor module and the
  wrappers of host functions. JavaScript carries only what the WebAssembly
  JavaScript API alone can do, such as a compile, an instantiation, JSPI, and a
  bulk copy where `multi_memory` is missing.
- It runs under a content security policy without `unsafe-eval`. The policy
  `script-src 'self' 'wasm-unsafe-eval'` admits it. It never makes a function
  from a string of source.
- It reads each JSPI function and each feature probe when it makes an engine. A
  browser without a feature loads the backend and declares less.
- It passes an exported function to the import of another instance as the
  function object of the export itself. A call between two instances is then a
  call from WebAssembly to WebAssembly, which JSPI needs.

## Test Lanes

Every suite runs on every backend:

| Suite                                                   | Browser | Wasmi | Wasmtime |
| ------------------------------------------------------- | ------- | ----- | -------- |
| The tests of the polyfill (`tests native`, `tests web`) | Yes     | Yes   | Yes      |
| The conformance corpus (`tests conformance`)            | Yes     | Yes   | Yes      |
| The Zena scenarios (`tests zena`)                       | Yes     | Yes   | Yes      |
| The faithfulness suite                                  | Yes     | Yes   | Yes      |
| The benchmarks (`bench`)                                | Yes     | Yes   | Yes      |

Wasmi is the practical native backend. Wasmtime runs beside it to give a person
a point of comparison. Each backend has its own list of expected failures in the
conformance corpus and in the Zena record. A failure that comes from a missing
capability is `Unsupported` with the name of the capability.

## Migration

The polyfill moves to the runtime layer in five steps:

1. Build beside. Write `wcmp-wasm-core`, `wcmp-wasm-core-web`, and
   `wcmp-wasm-core-wasmtime` new. Prove them without the polyfill, with the
   faithfulness suite and the test cases of this design. The polyfill does not
   change.
2. Make a seam. Inside the polyfill, move every use of a runtime layer type
   behind one internal module. The behavior does not change. The conformance
   record and the Zena record stay byte for byte the same.
3. Switch. In one change, point the seam at the new runtime layer on both
   targets. The changes that cross the seam land here: asynchronous
   instantiation, the host-suspension provider, trap kinds, and the wrappers of
   host functions.
4. Retire. Remove the vendored copies of the upstream backends, the `[patch]`
   section, and every dependency on the upstream crates. Add the outside
   consumer build.
5. Add Wasmi. Build `wcmp-wasm-core-wasmi` beside, prove it, and add it to every
   test lane.

At every step, no outcome in the conformance record or the Zena record moves
from pass to fail, on any lane. An outcome can move from fail to pass. The
switch is where most Zena scenarios move past `parse`. The records are
regenerated when that happens.

Step 3 is the one large change. Steps 1 and 2 make it small: the runtime layer
is proven before it, and the polyfill has one place to change. A split of step 3
makes the polyfill build against two runtime layers at once. That costs more
than it saves.

## Publication and Versions

The runtime layer and the polyfill can be published. A crate outside the
workspace depends on the polyfill and on one backend, and it builds with no
`[patch]` section. `cargo package` succeeds for each crate. This design does not
publish them.

The polyfill depends on two Wasmtime crates in two roles:

- `wasmtime-environ` is the translator. The polyfill pins it exactly, because a
  new translator changes the adapters it emits, and so the behavior of the
  polyfill. The pin moves only in a deliberate change, gated by the conformance
  record and the Zena record on every lane.
- `wasmtime` is the engine of the control. The Wasmtime backend builds against
  the release of the translator, so the control and the translator agree about
  the Component Model. One workspace dependency holds the version for both.

A release candidate of Wasmtime can stay in the tree. It blocks a release,
because a release needs every dependency to be a stable release on crates.io.
The Wasmi backend depends on the current major version of Wasmi with a caret
requirement. Nothing else in the graph must agree with it.

## Revisions to Earlier Designs

[PDD002] is revised in place. The polyfill owns its runtime layer, and
`wasm_runtime_layer` is prior art. Its rule that the polyfill proposes browser
extensions upstream first no longer holds.

[PDD005] is revised in place. The engine takes the backend that the host
selects, not the backend of the target.

[PDD022] is revised in place. Its JSPI provider becomes the host-suspension
provider, on every backend that declares `host_suspension`.

[PDD003] is an inventory of upstream facts. Its row for the runtime layer is
corrected in place.

[PDD024] cites the refusals of the upstream backends. Its links point at the
upstream repository, at the commit the vendored copies came from.

The README of the design corpus defines the runtime layer as this family of
crates.

## User Stories

A developer runs a Zena program in the browser.

> The Zena compiler emits a component whose core module defines a GC-typed
> global and exports a tag. The developer compiles it with the polyfill in
> Chrome. The browser backend describes the boundary only, and Chrome compiles
> the module. The program runs. When the program throws an exception that
> nothing catches, the developer reads `thrown Wasm exception`, the same message
> that Wasmtime gives.

A developer embeds the polyfill in a plugin host built on Wasmi.

> The developer adds `wcmp-wasm-core-wasmi` and makes an engine with the Wasmi
> backend. A plugin written in Rust instantiates and runs. A plugin that a GC
> language produced fails at `Component::new` with `Unsupported(gc)`. The
> developer knows at once which feature is missing.

A developer runs a large component on a page.

> One core module of the component is 12 MB. The polyfill compiles and
> instantiates it asynchronously, and the page stays responsive while it loads.

A contributor adds a backend for another engine.

> The contributor implements the trait of `wcmp-wasm-core` in a new crate. The
> contributor runs the faithfulness suite, and declares the capabilities that
> pass. The polyfill needs no change. The conformance corpus shows which
> components the new backend runs.

A maintainer reads a trap in a bug report from a Safari user.

> The trap names `IntegerDivisionByZero`, with Wasmtime's message. The
> maintainer does not need to know how JavaScriptCore words the trap.

## Test Cases

A module that defines a global of a GC reference type and exports a tag
instantiates on every backend that declares `gc` and `exceptions`. On the
browser and Wasmtime backends, the test instantiates the module and calls an
exported function. On Wasmi, a component that holds the same module fails at
`Component::new` with `Unsupported` that names `gc` or `exceptions`. It never
fails with a panic.

A module inside a component is never refused for an item that does not cross its
boundary. A test module defines an internal table of a concrete reference type,
an internal tag, and an internal GC global, and exports one function. It loads
on every backend whose engine accepts it.

A tag crosses the boundary. A test exports a tag from one instance and imports
it into another. The host reads the tag's parameter types. A guest throws with
the tag in one instance and catches it in the other.

A module above the synchronous limit of the browser compiles and instantiates in
Chromium. The test uses a module above 8 MB and runs in the web lane.

The same trap has the same kind on every backend. A test fixture raises each
core `TrapKind` on each backend whose capabilities permit it. Each backend
reports the same kind with the same message. In the browser, a message that the
table does not know becomes `Other`.

A host error survives a guest catch. A guest calls a host function that fails,
inside a `try_table` with `catch_all`. On every backend, the call fails with
`Host` and the host's own error. The guest's handler does not run.

A host function is entered at any depth. A host function calls into the guest,
and the guest calls the same host function again. Each depth returns its own
results. The test runs on every backend.

An uncaught exception is a trap. A guest throws an exception that nothing
catches. On every backend that declares `exceptions`, the call fails with
`UncaughtException` and the message `thrown Wasm exception`.

Host suspension meets its contract. On the browser and Wasmi backends, a test
starts three resumable calls in one store. Each call suspends in a suspending
host function. The test resumes them in the order third, first, second, and each
call finishes with its own results. A fourth call suspends with a host frame
between its start and the suspension, and it traps. Wasmtime does not declare
`host_suspension`.

Capabilities decide translation. On Wasmi, a composition of two components
translates without the exception barrier and runs. In a browser without
`multi_memory`, a composition whose adapter needs two memories fails at
`Component::new` with `Unsupported(multi_memory)`.

Memory access is sound and cheap. On each backend, `with_bytes` on an unshared
memory returns the bytes of the range. On Wasmi and Wasmtime it copies nothing.
On a shared memory it returns a copy. A range outside the memory is an error on
every backend. In the browser, a scalar load runs with no JavaScript frame on
the stack.

The canonical ABI makes a fixed number of memory accesses for one list. A test
lifts and lowers lists of lengths 1, 100, and 10,000. The count of accesses is
the same for each length, on every backend.

The benchmarks do not regress. The string and list benchmarks run on each
backend before and after the switch. No native result is slower beyond the noise
of the suite, and each web result is the same or faster.

The browser backend runs under a strict content security policy. A browser test
installs `script-src 'self' 'wasm-unsafe-eval'` and makes sure that the browser
enforces it. Under the policy, it runs a composition that prepares a call. It
also calls a host function of more than eight parameters.

The faithfulness suite passes. Each backend runs the specification test suite
for Wasm 2.0 and for each capability it declares. Each expected failure cites a
defect of the engine.

The polyfill builds as a downstream dependency. A crate outside the workspace,
with no `[patch]` section, depends on the polyfill and on one backend, and
builds for native and for `wasm32`. `cargo package` succeeds for each crate of
the runtime layer and for the polyfill.

The conformance record and the Zena record do not regress. At each step of the
migration, every lane runs both. No outcome moves from pass to fail.

No public signature of the polyfill names a type of the runtime layer. The
public API check of the repository holds this.

## References

- [PDD002], the ecosystem foundation, revised in place by this design.
- [PDD003], the compatibility outlook.
- [PDD005], the library foundations.
- [PDD022], stack switching and threads, whose provider this design generalizes.
- [PDD024], the Zena toolchain compatibility tests.
- [Wasm 2.0], the floor, and [Wasm 3.0], the announcement of the complete
  specification.
- The [feature status] table of the WebAssembly project.
- [Wasmi], its [resumable calls][Wasmi resumable], and its [trap
  codes][Wasmi traps].
- [Wasmtime] at `v49.0.0-rc.1`: its [traps][Wasmtime traps], its [exception
  barrier][Wasmtime exception barrier], its [memory data][Wasmtime memory data]
  and [shared memory data][Wasmtime shared memory data], its [shared canonical
  memory][Wasmtime shared canonical memory], its [`WasmStr`][Wasmtime WasmStr]
  and [`WasmList`][Wasmtime WasmList], and its
  [`ThrownException`][Wasmtime ThrownException].
- The [GC JS API] and the [exception handling] proposal.
- MDN on [SharedArrayBuffer].
- The Rust documentation of [`memory_size`][Rust memory_size].
- The WebAssembly [spec tests].
- [`wasm_runtime_layer`], prior art: its [types][upstream types], and its
  [browser tags][upstream browser tags], [browser
  references][upstream browser refs], and [Wasmtime
  tags][upstream Wasmtime tags] handling.

[PDD002]: ./PDD002%20Ecosystem%20Foundation.md
[PDD003]: ./PDD003%20Compatibility%20Outlook.md
[PDD005]: ./PDD005%20Library%20Foundations.md
[PDD022]: ./PDD022%20Stack%20Switching,%20Stackful%20Exports,%20and%20Threads.md
[PDD024]: ./PDD024%20Zena%20Toolchain%20Compatibility.md
[Wasm 2.0]: https://www.w3.org/TR/2025/CRD-wasm-core-2-20250616/
[Wasm 3.0]: https://webassembly.org/news/2025-09-17-wasm-3.0/
[feature status]: https://webassembly.org/features/
[Wasmi]: https://github.com/wasmi-labs/wasmi
[Wasmi resumable]:
  https://github.com/wasmi-labs/wasmi/blob/2970aa8/crates/wasmi/src/engine/resumable.rs
[Wasmi traps]:
  https://github.com/wasmi-labs/wasmi/blob/2970aa8/crates/core/src/trap.rs
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
[Wasmtime traps]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/environ/src/trap_encoding.rs#L111-L180
[Wasmtime exception barrier]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/environ/src/fact/trampoline.rs#L3990-L3991
[Wasmtime memory data]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/memory.rs#L408
[Wasmtime shared memory data]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/memory.rs#L928-L943
[Wasmtime shared canonical memory]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/instance.rs#L947-L952
[Wasmtime WasmStr]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/func/typed.rs#L1562
[Wasmtime WasmList]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/func/typed.rs#L1870
[Wasmtime ThrownException]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/exception.rs#L43
[GC JS API]: https://github.com/WebAssembly/gc/blob/main/proposals/gc/MVP-JS.md
[exception handling]:
  https://github.com/WebAssembly/exception-handling/blob/main/proposals/exception-handling/Exceptions.md
[SharedArrayBuffer]:
  https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/SharedArrayBuffer
[Rust memory_size]:
  https://doc.rust-lang.org/core/arch/wasm32/fn.memory_size.html
[spec tests]: https://github.com/WebAssembly/testsuite
[`wasm_runtime_layer`]: https://github.com/DouglasDwyer/wasm_runtime_layer
[upstream types]:
  https://github.com/DouglasDwyer/wasm_runtime_layer/blob/d4c702c/src/lib.rs#L107-L160
[upstream browser tags]:
  https://github.com/DouglasDwyer/wasm_runtime_layer/blob/d4c702c/backends/js_wasm_runtime_layer/src/module.rs#L231-L252
[upstream browser refs]:
  https://github.com/DouglasDwyer/wasm_runtime_layer/blob/d4c702c/backends/js_wasm_runtime_layer/src/module.rs#L92-L100
[upstream Wasmtime tags]:
  https://github.com/DouglasDwyer/wasm_runtime_layer/blob/d4c702c/backends/wasmtime_runtime_layer/src/lib.rs#L770-L797
