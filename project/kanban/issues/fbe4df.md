---
id: fbe4df
title: Survey Wasm Core engines for a full-fidelity fourth backend
type: docs
blocked_by: []
labels: [runtime-layer, backlog-burndown-001-q4]
created: 2026-09-30T19:38:03Z
disposition: accepted
disposition_at: 2026-10-16T06:31:58Z
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
- [x] A research note under `project/` (the owner picks the path) compares every candidate on every point above, with a citation for each claim.
- [x] It names the candidates that would reach full fidelity (every lexicon capability plus host suspension), and ranks them by cost.
- [x] It says what each would add over the browser and Wasmtime lanes, including browser-engine parity for Safari.
- [x] `markdown format` and `lint` pass.

## Review notes
- 2026-10-08 delivered (host session, backlog-burndown-001-q4), commit
  `docs(research): Survey Wasm Core engines for a fourth backend`. The note is
  `project/research/Wasm Core Engine Survey.md` (the owner picks the path; this
  one is a new `project/research/` directory, now in the `markdown` check's
  include list). Researched by a sub-agent from sources pinned to commits or
  tags; nothing was built.
  - Full fidelity (all nine Wasm capabilities plus host suspension): only V8
    through the `v8` crate, with the experimental `--wasm-wasmfx` flag.
  - Ranked by cost: V8 (prebuilt 38 MiB archive; port of the browser backend
    to `v8` handle scopes); then the near misses JavaScriptCore (WebKitGTK
    2.54, already in the flake, no stack switching, the only Safari-parity
    lane), SpiderMonkey (`mozjs`, Firefox semantics), Wizard (no host
    suspension, no embedding API). WasmEdge, Wasmer, WAMR and Wasm3 miss two
    or more capabilities or host suspension.
  - Two cheaper routes it names, untested: Chromium with
    `--js-flags=--wasm-wasmfx`, and the web suite under the existing WebKitGTK
    smoke driver.
  - Its "Claims Not Verified" section lists what it could not check (build
    sizes, V8 tier fallback, testsuite revisions of V8 and WebKit, and more).
  - Process note for the owner: one research helper put the owner's email in
    the User-Agent of one crates.io API request, then repeated the query with
    a generic one.
- 2026-10-08 gate on the series tip (`fix(zena): Move the Zena input back
  from the fork to Zena`): `tests all` green on every lane (native debug and
  release 1730, web debug and release 1716, no-provider 169 on each target,
  Wasmi 1447, fidelity 264/267/264, Zena Browser 18/18, Native 18/18, Wasmi
  2/18, Wasmtime 18/18, demo 68/68), with no `unexpected:` or `stale
  expectation:` line, and the full `lint` green. Conformance: native
  2412/2443, browser 2407, Wasmi 2352, no provider 2356.
- 2026-10-08 context-free review, round 1: the note's two `flake.nix` links
  cite the `spec-testsuite` input and `smokeWebkit` by name, not by line, and
  the summary no longer implies Wasmtime turns stack switching on by default.
- 2026-10-08 context-free review, round 2: the rusty_v8 citations now point
  at the release-artifact URLs (`build.rs` 838-873) and `archive_urls`
  (901-915) at `v152.2.0`, and the summary qualifies JavaScriptCore's
  memory64 with `JSC_useWasmMemory64=1`.
- 2026-10-08 context-free review, round 3: WasmEdge now ranks among the near
  misses (eight features with `threads` behind a flag, as SpiderMonkey is
  counted; no `stack_switching`; host suspension partial, one OS thread per
  waiting call; GC that never collects), and the table, its section and the
  summary agree on that.
- 2026-10-08 context-free review, round 4: **accepted**, with no code finding.
  The Wasm3 atomics claim cites `m3_config.h` 275-280, and the summary gives
  Wizard its own line.
