# Patched `js_wasm_runtime_layer`

This directory is `js_wasm_runtime_layer` 0.7.0 from crates.io, which is
identical to upstream `main` at commit `d4c702c` (2026-09-02) for this crate,
with the patches listed below. The workspace routes the registry name here through
`[patch.crates-io]` in the root `Cargo.toml`. Drop the directory and the patch
entry when upstream ships the fixes. The licenses are upstream's.

Each patch is marked `PATCH (wcmp)` in the source.

## 1. Memory, global, and tag imports (`src/module.rs`)

Upstream's module parser reaches `todo!()` for a core module that imports a
memory, a global, or a tag. The polyfill's adapter modules import the
instance-flag globals, and the libc-shared-instance linking pattern imports a
memory. The patch parses memory and global imports into the extern types the
runtime layer already has, and pushes them onto the index spaces so a later
export by index resolves. A tag import or export is an error instead of a
panic, since the runtime layer has no tag type.

Upstream: https://github.com/DouglasDwyer/wasm_runtime_layer/blob/d4c702c/backends/js_wasm_runtime_layer/src/module.rs#L231-L234

## 2. Host errors trap the guest (`src/func.rs`)

Upstream's host-function shim returns `undefined` to the guest when the host
returns `Err(_)`, so the failure is lost and the outer call succeeds. The patch
makes the JS closure return `Result<JsValue, JsValue>` and throws a JS `Error`
carrying the host error's message, so the guest traps and the error reaches
`Func::call` as it does with the native backend.

Upstream: https://github.com/DouglasDwyer/wasm_runtime_layer/blob/d4c702c/backends/js_wasm_runtime_layer/src/func.rs#L88-L94

## 3. Out-of-bounds memory access is an error (`src/memory.rs`)

Upstream's `Memory::read` and `Memory::write` build a `Uint8Array` view over
the requested range without a bounds check. For a range outside the buffer
the JS API throws a `RangeError`, and a JS exception thrown from inside a
host call is not catchable by the Rust caller (wasm-bindgen aborts). The patch
checks the range against the buffer length first and returns an `Err`, which
is what the native backend does for the same access.

## 4. The host's own error survives a guest catch (`src/store.rs`, `src/func.rs`, `src/instance.rs`)

With patch 2 a host error is a JS exception in the guest. A guest `catch_all`
can intercept it: the Component Model adapters do exactly that and re-trap
with "uncaught exception propagated out of component", so the outer call
would report the adapter's trap instead of the host's error. The native
backend never has this problem because a host error there is a trap, which no
`catch_all` sees. The patch records the first host error of a call on the store before
throwing (a later one, such as the adapter's re-trap through the same shim,
does not replace it), and a failed export call or instantiation reports that
recorded error in place of the JS exception that ended the call. A call that returns
normally clears the slot, so a guest that handles the exception and continues
is not reported as failed.

## 5. `Memory::current_pages` (`src/memory.rs`)

Upstream's `current_pages` is a `todo!()`. The polyfill reads the memory size
to bound-check `cabi_realloc` results and string and list pointers before it
touches memory, as Wasmtime does. The patch computes the page count from the
underlying `ArrayBuffer`'s byte length.

## 6. Asynchronous compilation (`src/lib.rs`, `src/module.rs`, `Cargo.toml`)

Upstream compiles every module with the synchronous `WebAssembly.Module`
constructor, which some browsers refuse on the main thread above a size limit.
The runtime layer's `WasmModule::new` is synchronous, so the patch adds
`Engine::precompile`, an `async fn` that compiles the bytes with
`WebAssembly.compile` and keeps the result on the engine, keyed by the bytes.
The next `Module::new` on that engine with the same bytes takes the compiled
module instead of compiling again. The polyfill awaits `precompile` for every
core module of a component before it constructs the runtime layer's `Module`.
The patch adds `wasm-bindgen-futures` for the promise-to-future bridge.

`Module::new` takes the entry for its bytes, and nothing else removed one, so
a caller that compiled several modules and then gave up — one compile failed,
or the caller was dropped — left the finished ones on the engine for its
lifetime. Taking them through `Module::new` would only move the leak, since
the engine never removes a module either. The patch adds
`Engine::discard_precompiled`, which drops the entry for some bytes, and
`Engine::precompiled_count`, which is what a test of a failed batch reads. The
polyfill discards every entry of a batch that it did not build. It also
compiles byte-identical modules of one component once and shares the result,
since the entry for those bytes serves one `Module::new` only. The
proposal for upstream is an asynchronous constructor on `WasmModule` itself, so
the handoff through the engine becomes unnecessary.

## 7. Function references as host-function arguments (`src/lib.rs`, `src/func.rs`, `src/store.rs`)

Upstream's `value_from_js_typed` refuses a `funcref`, so a host function
declared with one panics on its first call. The prepare-and-start intrinsics a
fused adapter imports carry the two functions the adapter generated for a call,
and the callee's core function, as `funcref` parameters. The patch converts
such an argument into a `Func` recorded in the store, so the host can call it
back.

The JS API hands over the function object alone and says nothing about its
signature, so the record is marked as carrying none. A call to such a function
reads its result count and types off the slice the caller supplied, with one
rule on top: a `BigInt` is always an `i64`, because the JS API represents that
core type and no other as one. A caller that cannot name a result type asks
for an `f64` and reads every number back faithfully, since a number passed on
to another wasm call reaches an `i32`, an `f32`, or an `f64` through the JS
API's own coercion. A null reference converts to `Val::FuncRef(None)`.

A record is the store's for as long as the store lives: the crate removes
nothing from its slabs, and a reference the host received outlives the call
that passed it anyway, since the prepare intrinsic keeps the functions of a
call until the call it prepared starts. The store therefore keeps one record
per function object rather than one per conversion. It holds a JS `Map` from
the function object to the record's index, built the first time a reference is
converted, and a conversion of a function it has already seen reads that record
back. Object identity is the function's identity here, because the JS API
returns one object per function address. Three references cross the boundary on
every prepared call, so without the map a store grows by three records per
guest-to-guest call and never gives one back; with it, the second call and
every call after it record nothing. `StoreInner::func_count` reports how many
records a store holds, which is what a test of a repeated call reads.

## 8. Host functions of more than eight parameters (`src/func.rs`)

Upstream builds the JS shim of a host function with `Closure::new`, which
`wasm_bindgen` implements for at most eight arguments, and reaches
`unimplemented!()` above that. The prepare-call intrinsic takes eight fixed
arguments followed by the caller's own flat arguments, which the canonical ABI
allows sixteen of, so the limit is reached by a prepared call that passes an
argument. A fused adapter prepares a call when the lower or the lift is
asynchronous, and only then: a synchronous lower of a synchronously lifted
callee calls the enter and exit intrinsics and nothing else. Wasmtime says the
same of its own `call_prepare` — "This is part of a async lower and/or async
lift adapter. This is not used for a sync->sync function call"
(`crates/environ/src/fact/trampoline.rs`) — and so does the polyfill, in the
module documentation of `src/executor/prepare_call.rs`. The patch adds a
variadic wrapper: the closure takes one JS array, and a small JS shim collects
the call's `arguments` into it, so the guest still imports an ordinary function
of the declared arity.

The shim is a `wasm_bindgen(inline_js)` snippet, `collect_arguments`.
`wasm_bindgen` writes the snippet to a file beside the module's own glue and
the page loads it as ordinary script, so the whole backend needs no more of a
page's content-security policy than the module itself does: `script-src 'self'
'wasm-unsafe-eval'` admits it. The shim was first written with
`Function::new_with_args`, which is `new Function` under another name, and a
policy without `'unsafe-eval'` — the common hardened setting — refused it; a
page under such a policy could then run no composition that prepares a call,
which is every composition with an asynchronous lower or lift.

Two artifacts hold that down, and each covers one half of it. The browser test
`it_runs_a_prepared_call_under_a_policy_without_unsafe_eval`, in
`rust/wcmp/tests/baseline_prepared_call.rs`, installs
the policy, proves the browser enforces it, and under it both runs a
composition whose lift is asynchronous — which does prepare a call — and builds
and calls a host function of nine parameters directly, so the wrapper is
exercised whatever the adapter emits. That test is what catches a return to
`new Function`. The web smoke page declares the same policy in a `<meta>`
element and proves the browser enforces it, which is what shows the snippet
file itself loads under the policy like any other script; its fixtures lift and
lower synchronously throughout, so no prepare-call trampoline is emitted there
and the page would not catch the regression on its own.

The proposal for upstream is closures of arbitrary arity, which would remove
the shim altogether. The filing carries the arity limit and this
content-security-policy consequence with it.

## 9. A thrown value with no message string (`src/lib.rs`)

Upstream's `JsErrorMsg` conversion reads the `message` property of a thrown
JS value and calls `expect` on it being a string. `Reflect::get` answers
`Ok(undefined)` for a property an object does not have, so the `expect` fires
for every thrown value that is neither a string nor an `Error` — a
`WebAssembly.Exception` a guest threw, for one, which is what a component
that uses the exception-handling proposal hands the host. A panic there
aborts the page rather than failing the call. The patch falls through to the
debug rendering when the property is not a string, which is the branch
upstream already has for a value with no `message` at all.

The same branch names the one such value the polyfill can identify. An
exception a guest threw and did not catch reaches the host as the
`WebAssembly.Exception` object, and the debug rendering of it says nothing a
caller can read. The native backend reports that failure as `thrown Wasm
exception`, so the patch gives the JS backend the same wording for the same
object, and a caller of either backend reads one message.

## 10. A host function is entered at any depth (`src/func.rs`)

Upstream wraps every host function in one `Closure<dyn FnMut(..)>`. The JS glue
`wasm_bindgen` generates for a mutable closure clears the closure's pointer for
the length of a call and restores it afterwards, so a call made while another
call of the same closure is still on the stack reaches the shim with a null
pointer and `throw_str("closure invoked recursively or after being dropped")`
(`wasm-bindgen`'s `src/convert/closures.rs`). Upstream also allocates one
results buffer per host function (the `res` vector in `WasmFunc::new`) and
captures it in the body, which is what makes the body `FnMut`, and a second
call in flight would write over the first call's results. A native engine has
neither limit: it calls a host function already on the stack with no more
ceremony than any other.

The patch removes both. The JS-facing shim is a `Closure<dyn Fn(..)>`, a shared
closure, which `wasm_bindgen` lets JavaScript enter at any depth, and it calls
the body directly. The body is `Fn`: the function the runtime layer hands
`WasmFunc::new` is `Fn` already, and the results buffer is allocated inside the
call, so the arguments and the results of one call live in that call's own
frame and never share a buffer with another call of the same function. A host
function re-entered from the guest runs as it does natively.

Re-entry adds no aliasing of `StoreInner` that the backend did not already
have. Each call of a host function rebuilds its store context from the raw
store pointer, and a host function that calls into the guest, which calls a
different host function, already stacks a second such context over the first;
that is the "re-entrant with exclusive but stacked calling contexts" case the
comment on `Store` describes. A second call of the same host function stacks
its context the same way, through the same pointer. The first-error rule of
patch 4 holds across depths as well: the store has one slot, an inner call
that fails records its error there before the outer one can, and an inner
`Func::call` that returns normally clears it, as it does for a nested call of a
different function.

An earlier form of this patch kept the body behind a `RefCell` and refused the
second call with a public `ReentrantHostCall` error. That type is gone, and the
backend exports nothing in its place. The borrow guard also gave up a
robustness property the old glue had, an engine-level throw leaving the guard
held for the life of the store; with no guard, nothing is held across a call,
so that gap is gone too.

The proposal for upstream is the shared closure and the per-call results
buffer, which make the web backend's host functions behave as a native
engine's do.

## 11. Types the runtime layer cannot name (`src/module.rs`)

Upstream's module parser reaches `unreachable!()` for a type that is not a plain
function type, such as a continuation type of the stack-switching proposal, and
`unimplemented!()` for a recursion group of more than one type. It also rejects
a module that defines a table of any reference type other than `funcref` and
`externref`. The polyfill compiles a probe module that defines a continuation
type and a table of continuations on every target, to ask the engine whether it
switches stacks, and in the browser that probe must fail with an error rather
than abort the page.

The patch gives every type of every recursion group its index. A type the
runtime layer cannot name, which is any type other than a function type over
the value types it has, takes its index as `None`, and so does a defined table
of such references. Only an import or an export of one is an error. A module
that keeps those types inside itself parses, and `WebAssembly.Module` decides
whether the browser compiles it.

## 12. JavaScript Promise Integration (`src/func.rs`, `src/lib.rs`, `src/store.rs`, `src/instance.rs`)

Upstream has no way to reach JavaScript Promise Integration (JSPI). The
polyfill switches the stacks of guest threads with it in the browser: a guest
thread starts through `WebAssembly.promising`, and it suspends by calling an
import made with `WebAssembly.Suspending`. The patch adds three items.

`Func::new_suspending` makes a host function that a guest imports through
`new WebAssembly.Suspending(f)`. Its body answers a promise, and the calling
stack suspends until the promise settles. The function shares the JS shim of
`WasmFunc::new`, which the patch moves into `js_shim`. The record of such a
function keeps the `WebAssembly.Suspending` object, and an imports object takes
that object in place of the function. It is an import and nothing else: a call
from the host, a table store, or a conversion to a value fails with an error,
because the JS API accepts the object only in an imports object and the
function behind it answers a promise instead of its declared results.

`Func::call_promising` calls an exported function through
`WebAssembly.promising` and answers the promise. The call runs the function
synchronously until it returns or first suspends. The wrapper is made once per
function and kept on its record. A trap never comes back from the call itself:
the browser rejects the promise with it.

`StoreInner::failure` turns the value a failed call threw into the error it
reports: the host's own error of patch 4 when one is pending, and the thrown
value otherwise. `Func::call` and `Instance::new` report through it now, and
the owner of a promising call uses it when the promise is rejected.

Both constructors of the JSPI API are read from the global `WebAssembly`
namespace when they are needed, so a browser without JSPI loads the backend
unchanged and fails only a call of the two functions.

The proposal's overview states that a suspending import suspends only when its
function answers a promise that is still pending. Chromium 147 suspends on
every call, even for a plain value or a resolved promise. The browser test
`it_suspends_on_a_suspending_import_whose_promise_is_already_resolved`, in
`rust/wcmp/src/baseline/jspi_switch_module.rs`, holds
the measured behavior down.

The proposal for upstream is the two functions and the failure helper, since a
runtime layer over the browser engine has no other way to suspend a stack.

## 13. Running guest threads through JSPI (`src/store.rs`, `src/func.rs`, `src/instance.rs`)

The polyfill's scheduler runs every guest thread through JavaScript Promise
Integration in the browser. A resumed stack runs on a microtask, after the Rust
frame that resolved its promise returned, so the scheduler has to know where a
host function runs, and a store has to outlive a stack that still runs. The
patch adds five things.

`StoreInner::guest_depth` counts the calls from the host into the guest that are
running: `Func::call` and an instantiation whose start function runs raise it
for their length. A promising call does not, because it returns as soon as its
stack first suspends. A host function that runs inside a promising stack at the
depth the stack began at has only WebAssembly frames between it and the start of
that stack, so a suspending import called above it may suspend the stack. Deeper
than that a host frame lies in between, and a suspension there traps.

`StoreInner::clear_failure` drops the error a host function returned during a
call that went on to succeed, as a `Func::call` that returns does, and
`StoreInner::pending_failure` says whether one is pending. The owner of a
promising call clears the slot when the stack suspends or returns, so that a
host error one stack caught never becomes the failure another stack reports. A
stack that fails before it first suspends reports the pending host error at
once; its promise is rejected only on a microtask.

`StoreInner::retain_on_drop` leaves the store allocated when its owner drops it.
A stack whose promise was resolved resumes on a microtask and reaches the store
through its raw pointer, so the owner calls this when the store drops in
between. `StoreInner::release_orphaned` then frees the store on a later
microtask. The stack calls it from a host function as it resumes, and reaches
the store no more once that function returns, so no frame holds the store when
it is freed.

`Func::from_exported_function` records an exported function's typed record as
the record a reference to the same function object converts to
(`StoreInner::remember_exported`). The JS API carries no signature with a
function reference, and a promising call runs its callee from WebAssembly, which
names the callee's exact type. A guest that passes one of its exports on as a
`funcref`, as a fused adapter passes the callee's core function, now hands the
host the function with its real signature. A function the guest never exported
still converts to a record of unknown signature.

`Instance::new` borrows the engine only to read the module out, and not across
the instantiation. A start function runs inside it, and a host function that
start function calls can compile a module of its own, which borrows the engine
again: the switch module the polyfill compiles for a thread's first entry of a
new type is one. Upstream panics with `RefCell already borrowed` there.

The proposal for upstream is the depth, the failure helpers, the typed export
records, and the shorter engine borrow. The retention is the polyfill's own
answer to a store that drops while one of its stacks is scheduled to run.
