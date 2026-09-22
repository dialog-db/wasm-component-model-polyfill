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
The patch adds `wasm-bindgen-futures` for the promise-to-future bridge. The
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
`rust/wasm-component-model-polyfill/tests/baseline_prepared_call.rs`, installs
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

## 10. A re-entrant host call is refused, not thrown at (`src/func.rs`, `src/lib.rs`)

Upstream wraps every host function in one `Closure<dyn FnMut(..)>`. The JS glue
`wasm_bindgen` generates for a mutable closure clears the closure's pointer for
the length of a call and restores it afterwards, so a call made while another
call of the same closure is still on the stack reaches the shim with a null
pointer and `throw_str("closure invoked recursively or after being dropped")`
(`wasm-bindgen`'s `src/convert/closures.rs`). That message names a
`wasm_bindgen` mechanism rather than anything the caller did, it reaches the
outer call as a bare JS exception, and it leaves nothing on the store, so the
host error slot of patch 4 has nothing to report and a guest `catch_all` can
replace it outright. Upstream also reuses one results buffer per host function
(the `res` vector in `WasmFunc::new`), which a second call in flight would
overwrite.

The patch makes the wrapper refuse the second call itself. The body of the call
moves behind a `RefCell`, and the JS-facing shim becomes a `Closure<dyn Fn(..)>`
— a shared closure, which `wasm_bindgen` lets JavaScript enter at any depth —
that borrows the body for the length of one call. A call that finds the borrow
already out is a re-entrant call: it records `ReentrantHostCall` on the store as
the call's first host error and throws a JS `Error` carrying that type's
message, so the guest traps where it made the call and the outer `Func::call`
reports the recorded error. `ReentrantHostCall` is public, so a caller that
wants to tell this failure from any other downcasts the `anyhow::Error` to it;
the polyfill does, and answers its own re-entrant-host-call scheduler cause.

The patch gives up one robustness property in the exchange. The old `FnMut`
shim's glue cleared the closure's pointer in a `try` and restored it in a
`finally`, so a call that ended by an engine-level throw — a stack overflow, an
out-of-memory — still left the closure enterable afterwards. The borrow this
patch takes is a Rust guard, released when the body's own frame returns, and an
engine-level throw leaves no frame to return through, so the borrow stays out
and every later call of that host function is refused for the life of the
store. An ordinary guest trap is not such a throw: `wasm_bindgen` turns the
value a guest threw into the `Err` of the call that entered the guest, so the
body returns and the guard drops. The gap is therefore out of reach of a guest
that merely traps, and a store that has overflowed the JS stack has little left
to give in any case; it is recorded because the property existed before and
does not now.

The refusal is the whole of the change: nothing here makes a host function
re-entrant. Doing that would need a results buffer per call and a JS shim per
call, and the aliasing of `StoreInner` that the whole backend rests on would
have to be examined first. The proposal for upstream is the shared closure and
the structured refusal, which is what a native engine's caller already gets:
there a host function is entered at any depth and this failure does not exist.
