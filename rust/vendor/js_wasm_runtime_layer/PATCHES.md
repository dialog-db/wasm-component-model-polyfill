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
