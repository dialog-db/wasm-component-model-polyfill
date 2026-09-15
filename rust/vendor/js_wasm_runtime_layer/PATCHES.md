# Patched `js_wasm_runtime_layer`

This directory is `js_wasm_runtime_layer` 0.7.0 from crates.io, which is
identical to upstream `main` at commit `d4c702c` (2026-09-02) for this crate,
with two patches. The workspace routes the registry name here through
`[patch.crates-io]` in the root `Cargo.toml`. Drop the directory and the patch
entry when upstream ships both fixes. The licenses are upstream's.

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
