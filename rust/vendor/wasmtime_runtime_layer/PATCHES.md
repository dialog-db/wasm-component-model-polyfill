# Patched `wasmtime_runtime_layer`

This directory is `wasmtime_runtime_layer` 48.0.0 from crates.io, which is
identical to upstream `main` at commit `d4c702c` (2026-09-02) for this crate,
with the patches listed below. The workspace routes the registry name here
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

## 2. The stack-switching proposal on x86_64 Linux (`Cargo.toml`, `src/lib.rs`)

The polyfill fills its suspend capability natively with the instructions of the
WebAssembly stack-switching proposal (`cont.new`, `resume`, `suspend`). Wasmtime
49 implements them with Cranelift on x86_64 Linux, behind the `stack-switching`
cargo feature and the `Config::wasm_stack_switching` setting, both off by
default.

The patch turns both on for x86_64 Linux only. The cargo feature comes through
a target-specific `wasmtime` dependency, which cargo merges with the main one.
`Engine` no longer derives `Default`; its hand-written `Default` builds the
engine from a `Config` with the proposal on, and falls back to Wasmtime's
default engine if that configuration is refused. On every other target the
engine is upstream's default, and the polyfill's probe finds no stack switching
there.

The cost: Wasmtime refuses compiler inlining while the proposal is on
(`Config::compiler_inlining`, and the check in `Config::build_compiler`).
Inlining is off by default in Wasmtime 49, so the default engine compiles the
same code as before, but a later patch that wants inlining cannot have both.
Nothing else in the engine's configuration changes.

The cost in memory: each `cont.new` allocates a continuation with a stack of
its own, of the engine's `async_stack_size` (2 MiB by default), and Wasmtime
never frees a continuation before its store drops
(`StoreOpaque::allocate_continuation`: "we currently don't support deallocating
them"). A store that made one continuation per guest thread would grow without
bound. The polyfill's switch module therefore makes a continuation only when no
idle one is at hand: a thread runs on a worker, a continuation that never
returns. A worker whose thread finished suspends and waits in a pool, and the
next thread to start resumes it. A store holds as many stacks as it ever had
threads alive at once, so its memory is bounded by its peak of live threads
rather than by the number of threads it ran. A thread that traps loses its
worker, and a thread the store gives up on while it is suspended keeps its
worker until the store drops. The polyfill's repository test
`it_reuses_the_workers_of_finished_threads` runs fifty calls of two threads
each on two workers.

The runtime layer passes the continuation types of the polyfill's switch module
through its module parsing unchanged: `Module::new` hands the bytes to
`wasmtime::Module::from_binary` and then checks only the types of imports and
exports, and the switch module imports and exports only functions over number types
and `funcref`. Its tags, its continuation types, and its tables of
continuations stay inside the base module.

Upstream: https://github.com/DouglasDwyer/wasm_runtime_layer/blob/d4c702c/backends/wasmtime_runtime_layer/src/lib.rs
