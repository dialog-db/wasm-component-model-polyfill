# Patched `wasmtime_runtime_layer`

This directory is `wasmtime_runtime_layer` 48.0.0 from crates.io, which is
identical to upstream `main` at commit `d4c702c` (2026-09-02) for this crate,
with the patch listed below. The workspace routes the registry name here
through `[patch.crates-io]` in the root `Cargo.toml`. Drop the directory and
the patch entry when upstream ships a release built against Wasmtime 49. The
licenses are upstream's.

Each patch is marked `PATCH (wcmp)` in the source.

## 1. Build against Wasmtime 49 (`Cargo.toml`)

Upstream's newest release of this backend depends on `wasmtime` 48. The
workspace builds against 49, which removed the rule that compiled a fused
adapter between a lift and a lower of one component instance to an
unconditional trap, and cargo resolves one `wasmtime` for the whole graph. The
patch moves the dependency to `=49.0.0-rc.1`, the pin the workspace uses. The
backend's own source needs no change: every Wasmtime API it touches is the
core-module surface, which 49 left alone.

This pin and the workspace's own `wasmtime` pin move together: cargo folds
them into a single `wasmtime` only while they match, so a bump that moves one
across a major resolves two copies and the `gc-drc` feature the polyfill
enables on its `wasmtime` dependency stops reaching the copy this backend's
`Engine` builds.

Upstream: https://github.com/DouglasDwyer/wasm_runtime_layer/blob/d4c702c/backends/wasmtime_runtime_layer/Cargo.toml
