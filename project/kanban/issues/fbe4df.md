---
id: fbe4df
title: Survey Wasm Core engines for a full-fidelity fourth backend
type: docs
blocked_by: []
labels: [runtime-layer, backlog-burndown-001-q4]
created: 2026-09-30T19:38:03Z
---

## What to build

The owner wants to show a full-fidelity uplift from Wasm Core to WASI 0.3 on more than the browser and Wasmtime. Wasmi, the third backend, is an imperfect case for that: it has no GC, no exceptions, no threads and no stack switching; 12 of 14 Zena scenarios stop at `parse` with `Unsupported gc`; conformance is 2349/2440 against 2409/2440 on Wasmtime; and it backs memory with a zero-filled `Vec`, so large-memory tests cost real RAM. Wasmtime is the control, the one runtime you would not use the polyfill with. So no non-control native backend today reaches the full fidelity the browser does.

Survey candidate Wasm Core engines for a fourth backend, and write the findings as a research note for the owner (not a PDD; the design comes after). For each candidate, state with citations to its source or docs at a pinned commit:

- **Proposals.** Support for each lexicon capability (`multi_memory`, `memory64`, `tail_call`, `exceptions`, `function_references`, `gc`, `relaxed_simd`, `threads`, `stack_switching`) and for `extended_const`, `wide_arithmetic`, and Wasm 3.0 as a whole; whether each is on by default, behind a flag, or experimental.
- **Host suspension.** A way to suspend a guest in a host call and resume it later (resumable calls, stack switching, coroutines, fibers, or an async API), since the polyfill needs `host_suspension` for WASI 0.3.
- **Embedding.** A Rust embedding API or a C API a Rust backend can wrap; how externs, references (including GC references) and host functions cross it; whether it runs on Linux x86_64 in the Nix build; licence.
- **Fidelity.** Its own spec-test pass rate at a pinned testsuite, if published; how it stores memory (mapped or allocated); traps and their messages.
- **Fit with the project's browsers.** The project weighs Safari above Firefox. An engine that shares a browser's implementation (for example JavaScriptCore, SpiderMonkey or V8 embedded natively) would add a native lane with that browser's semantics, which may matter more than a new engine family.
- **Cost.** Build time and size in the Nix flake, maintenance activity, and how far it is from the `wcmp-wasm-core` trait (the contract tests and the fidelity suite are the bar).

Candidates to consider include, but are not limited to: WAMR (wasm-micro-runtime), WasmEdge, Wasmer, V8 embedded natively (rusty_v8), SpiderMonkey embedded natively, JavaScriptCore embedded natively, Wizard, and any engine the survey finds that implements Wasm 3.0 with stack switching.

## Acceptance criteria
- [ ] A research note under `project/` (the owner picks the path) compares every candidate on every point above, with a citation for each claim.
- [ ] It names the candidates that would reach full fidelity (every lexicon capability plus host suspension), and ranks them by cost.
- [ ] It says what each would add over the browser and Wasmtime lanes, including browser-engine parity for Safari.
- [ ] `markdown format` and `lint` pass.

## Review notes

