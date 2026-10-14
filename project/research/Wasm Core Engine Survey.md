# Wasm Core Engine Survey

This note answers one question for the owner. Which Wasm Core engine can be the
fourth backend of the runtime layer, and reach full fidelity on a lane that is
not the browser and not Wasmtime? The survey was done on 2026-10-08, for board
card `fbe4df`. It is a research note, not a design.

Full fidelity here means two things. The engine implements every Wasm feature of
the capability lexicon: `multi_memory`, `memory64`, `tail_call`, `exceptions`,
`function_references`, `gc`, `relaxed_simd`, `threads`, and `stack_switching`.
And the backend can declare `host_suspension`, which WASI 0.3 needs.

The short answer:

- One candidate reaches full fidelity: V8, embedded natively through the `v8`
  crate. It needs V8's experimental stack-switching flag. It is Chrome's engine,
  so it adds little that the browser lane does not already test.
- No candidate reaches full fidelity with every feature on by default. No
  shipping engine turns on stack switching by default. Wasmtime has it off by
  default too, and the Wasmtime backend turns it on.
- JavaScriptCore, Safari's engine, misses only `stack_switching` in WebKitGTK
  2.54, once `JSC_useWasmMemory64=1` turns memory64 on. That memory64 is not
  stable: the option's status is `testable`, and it runs only in the IPInt
  interpreter tier. It is the only candidate that gives a native lane with
  Safari's semantics. It is also the cheapest to build, because the flake
  already holds it.
- SpiderMonkey also misses only `stack_switching`. It has Firefox's semantics,
  which the project weighs low.
- WasmEdge also misses only `stack_switching`, but its GC never collects, and it
  gives host suspension only at one OS thread per waiting call.
- Wizard has eight features with partial `threads`, but no host suspension and
  no embedding path a Rust backend can use.
- Wasmer, WAMR, and Wasm3 each miss two or more capabilities, or host
  suspension, or a usable embedding API.

## Terms

- An engine is an implementation of Wasm Core. A backend is a crate that
  implements the runtime layer over one engine.
- The lexicon is the fixed list of capability names in
  [`capability.rs`][wcmp-capability]. Each name for a Wasm feature is the name
  of the same feature in `wasmparser`.
- Host suspension is the primitive of the runtime layer for a call that waits. A
  suspending host function answers "not yet". The guest call then ends as a
  handle, and the host resumes the handle later with the results.
- JSPI is JavaScript Promise Integration. It is the JavaScript API pair
  `WebAssembly.Suspending` and `WebAssembly.promising`. The browser backend
  fills host suspension with it.
- WasmFX is the common name of the stack-switching proposal. Its instructions
  are `cont.new`, `cont.bind`, `suspend`, `resume`, `resume_throw`,
  `resume_throw_ref`, and `switch`. A `contref` is a reference to a
  continuation.
- An `exnref` is a reference to a caught exception, from the final exception
  handling proposal (`try_table` and `throw_ref`). The legacy proposal
  (`try`/`catch`/`delegate`) does not count for the `exceptions` capability.
- nixpkgs is the Nix package set. "In nixpkgs" means the project's flake can use
  a package without writing a derivation for it.
- A JS-only boundary is an engine whose only embedding surface for Wasm is the
  JavaScript `WebAssembly` object. Every extern, reference, and host function
  crosses as a JavaScript value.

## Method and Pins

Eight research passes read each engine's source at a pinned commit or tag. Every
claim below cites a link pinned to that commit or tag. Nothing was built or run.
Where the research did not check a claim, the note says so.

| Subject                 | Pin                                                              |
| ----------------------- | ---------------------------------------------------------------- |
| This repository         | `735583275d14097f121d4cbac124c96a3bb171cf`                       |
| The project's nixpkgs   | `b6c8664de9b6cc07fe5666a29f91884ba81197c4` (2026-09-29)          |
| The control             | Wasmtime `v49.0.0-rc.1` (`62ef0a5586d4`)                         |
| Wasmi                   | `v2.0.0` (`2970aa871cc1`)                                        |
| V8                      | `v8` crate `v152.2.0` (`2768994f664e`), V8 15.2.124.1            |
| JavaScriptCore          | WebKitGTK `2.54.0` (`5220e80b97a2`), `2.54.1`, WebKit `2808f5b4` |
| SpiderMonkey            | `mozjs-sys-v153.4.0-0` (`34ad101f42ba`), SpiderMonkey 153.4.0    |
| WasmEdge                | `87b11e6f7d71` (release 0.18.0), Rust SDK `37e693cfe0c4`         |
| Wasmer                  | `2b2e6851832d` (release 7.5.0)                                   |
| WAMR                    | `38b044a67689` (release 2.4.5), Rust SDK `d2508adf1517`          |
| Wizard                  | `fb8300acf10c`, Virgil `a3a6ab7b5d53`                            |
| Wasm3                   | `28ecb9af6d20` (unreleased 0.9.2)                                |
| webassembly.org table   | WebAssembly/website `08147a438d02`, `features.json`              |
| Spec tests of this repo | WebAssembly/testsuite `0dc0343c9876`                             |

The project's root flake follows nixpkgs `b6c8664`, not `3e41b24`
([`flake.lock`][wcmp-lock], node `nixpkgs_3`). The `3e41b24` node belongs to a
sub-input. Every nixpkgs fact below is at `b6c8664` unless the note says
otherwise.

## The Bar and the Lanes Today

The runtime-layer trait follows Wasmtime's core API ([`lib.rs`][wcmp-lib]). A
backend implements five traits. The contract tests (36 test functions in
`rust/wcmp-wasm-core-contract/src/`) and the fidelity suite (the official spec
tests at testsuite `0dc0343`, the `spec-testsuite` input of
[`flake.nix`][wcmp-testsuite]) are the bar.

The host suspension contract has four rules ([PDD025 lines
434-444][wcmp-pdd025-hs]):

- A suspension succeeds only when WebAssembly frames alone lie between the start
  of the resumable call and the suspending host function.
- Any number of handles can wait at once in one store, and the host can resume
  them in any order.
- A handle does not hold the store. It borrows the store only while it resumes.
- When a store drops, its waiting handles drop without a resumption. A
  resumption that started before the drop runs to its next suspension or its end
  first, and the backend keeps the store's state alive until then.

The three lanes today:

| Lane     | Wasm capabilities declared                                | `host_suspension`    |
| -------- | --------------------------------------------------------- | -------------------- |
| Browser  | Probed at run time. Chromium in CI.                       | Yes, through JSPI    |
| Wasmi    | `multi_memory`, `memory64`, `tail_call`, `relaxed_simd`   | Yes, resumable calls |
| Wasmtime | All nine. `stack_switching` only on x86_64 Linux or macOS | No                   |

Sources: the Wasmi features ([`backend.rs` lines 33-54][wcmp-wasmi-backend]),
the Wasmtime features ([`backend.rs` lines 37-73][wcmp-wasmtime-backend]),
Wasmtime's platform rule for stack switching ([`config.rs` lines
2455-2470][wasmtime-ss-platform]), and the browser probes
([`probes.rs`][wcmp-probes], [`backend.rs` lines 117-126][wcmp-web-backend]).

Two facts about the lanes matter for this survey:

- The Wasmtime backend does not declare host suspension. Its `Func::call_async`
  borrows the store mutably until the call finishes ([`func.rs` line
  1113][wasmtime-call-async]), so one store has at most one suspended call
  ([PDD025 lines 457-461][wcmp-pdd025-hs]).
- Chromium ships stack switching only behind a flag ([`features.json` line
  308][wf-chrome-ss]). The flake passes no such flag. A search of `flake.nix`
  and `nix/` for `wasmfx`, `stack-switching`, and `js-flags` finds nothing. So
  in Chromium the browser lane cannot declare `stack_switching`. No lane today
  reaches full fidelity in the strict sense of the card.

Wasmi backs memory with a zero-filled `Vec` ([`buffer.rs` line
143][wasmi-buffer]). The card notes that this makes large-memory tests cost real
RAM.

## Candidates

Each section answers the same six points: proposals, host suspension, embedding,
fidelity, fit with the browsers, and cost. The comparison table after the
sections collects the answers.

### V8, Embedded Natively

V8 is the engine of Chrome, Edge, Node.js, and Deno. The `v8` crate (repository
`denoland/rusty_v8`, MIT) binds V8's C++ embedder API. Tag `v152.2.0` embeds V8
15.2.124.1 ([README line 3][rv8-readme]). V8 itself is BSD-3-Clause
([LICENSE][v8-license]).

**Proposals.** V8 15.2 has no flag for most finished proposals. The list
`FOREACH_WASM_NON_FLAG_FEATURE` holds `multi_memory`, `memory64`, `return_call`,
`exnref`, `typed_funcref`, `gc`, `relaxed_simd`, `threads`, `shared_memory`,
`extended_const`, and `jspi` ([`wasm-features.h` lines 22-43][v8-features]). So
all of Wasm 3.0 is on by default. Two features are behind flags
([`wasm-feature-flags.h`][v8-flags]):

- `stack_switching` is experimental, behind `--wasm-wasmfx` ("core stack
  switching", [lines 49-52][v8-flags-wasmfx]). V8 decodes all seven
  instructions, `switch` included ([`wasm-opcodes.h` lines
  260-267][v8-opcodes]). The baseline compiler Liftoff has no code for them
  ([`liftoff-compiler.cc` lines 2229-2285][v8-liftoff]). Only the optimizing
  compiler Turboshaft compiles them ([`turboshaft-graph-interface.cc` lines
  4083-4471][v8-turboshaft]).
- `wide_arithmetic` is pre-staging, behind `--wasm-wide-arithmetic` ([lines
  85-88][v8-flags-wide]).

An embedder sets flags with `v8::V8::set_flags_from_string` before it makes the
first isolate ([`V8.rs` lines 164-170][rv8-flags]).

**Host suspension: meets the contract, through JSPI.** JSPI is always on in V8
15.2 ([`wasm-features.h` line 41][v8-features]). It shipped in Chrome 137 ([New
in Chrome 137][chrome-137], fetched 2026-10-08). A Rust host makes the import
return a promise from `v8::PromiseResolver` ([`promise.rs` lines
152-160][rv8-promise]). To resume, it resolves the promise and calls
`perform_microtask_checkpoint` ([`isolate.rs` lines 1961-1975][rv8-microtask]).

- Many calls can wait at once and resume in any order. Each `promising` call has
  its own stack and its own promise.
- A waiting call is a pair of `v8::Global` handles. It holds no Rust borrow of
  the isolate.
- V8 refuses a suspension through JavaScript frames with "trying to suspend JS
  frames" ([`runtime-wasm.cc` lines 227-247][v8-suspend-error]).
- WasmFX cannot serve the host. A `contref` cannot cross to JavaScript ("invalid
  type (ref cont)", [`wasm-objects.cc` lines 3157-3161][v8-contref]).

**Embedding.** The crate exposes only `WasmModuleObject`, `WasmStreaming`,
`CompiledWasmModule`, and `WasmMemoryObject::buffer` ([`wasm.rs`][rv8-wasm]).
Everything else goes through the JavaScript `WebAssembly` object. This is the
same boundary the browser backend uses. The standard C API `wasm.h` exists in V8
only in the separate `wee8` library ([`BUILD.gn` lines 7975-7995][v8-wee8]),
which the crate does not link ([`BUILD.gn` lines 4-17][rv8-buildgn]).

- GC structs and arrays reach the host as opaque objects. An `i31` reaches it as
  a number.
- An `exnref` and a `contref` cannot cross as values ([`wasm-objects.cc` lines
  3057-3066][v8-exnref]). An uncaught exception reaches the host as a
  `WebAssembly.Exception` object, not an `exnref`.
- `WebAssembly.Module.imports` and `exports` give module, name, and kind, and no
  types ([`wasm-module.cc` lines 364-450][v8-imports]). The backend must parse
  types itself. The browser backend already does this with `wasmparser`.
- A Rust `&[u8]` view of memory is possible through the backing store of the
  `ArrayBuffer`. A grow detaches the buffer, so the view must be fetched again.
- The isolate is single-threaded. `threads` needs one isolate per thread over a
  shared backing store.

**Fidelity.** V8 runs the spec tests and the proposal tests for JSPI, threads,
and stack switching ([`update-wasm-spec-tests.sh`][v8-spec-update],
[`testcfg.py` lines 37-38][v8-testcfg]). It publishes no pass rate. On x64 the
status file expects three failures: `memory64/memory64`, `memory64/table64`, and
`table` ([`wasm-spec-tests.status` lines 14-18][v8-status]). The testsuite
revision of V8's test archive is not recorded in the repository. With the trap
handler on, each 32-bit memory reserves 8 GiB of address space with guard
regions ([`backing-store.cc` lines 74-96][v8-memory]). Traps reach the host as
`WebAssembly.RuntimeError` objects with fixed strings ([`message-template.h`
lines 713-742][v8-traps]). The browser backend already maps these strings
([`traps.rs` lines 52-63][wcmp-traps]).

**Fit with the browsers.** It is Chrome's engine. Crate major 152 is Chrome
152's V8 ([README][rv8-readme]). It adds no Safari coverage.

**Cost.**

- Build: by default `build.rs` downloads a prebuilt static library ([`build.rs`
  lines 838-873][rv8-download], the URLs of the GitHub release artifact). The
  Linux x86_64 release archive is 39,784,686 bytes gzipped (measured from the
  release download, 2026-10-08). `RUSTY_V8_ARCHIVE` takes a local file
  ([`build.rs` lines 901-915][rv8-env]), so a fixed-output derivation can feed
  it. A source build takes "often 30 minutes" ([README line 19][rv8-readme]).
- nixpkgs builds `rusty_v8` 150.4.0 from source for Deno, as a `big-parallel`
  job ([`rusty-v8/default.nix` lines 74, 122, 190][nix-rusty-v8]). The project
  must not use that path, because it is a long C++ build.
- Distance from the trait: large in code, small in design. The backend is a port
  of the browser backend onto `v8` handle scopes, plus a microtask pump. The
  browser backend was 12,813 lines with its tests at this note's commit, 9,540
  of them in `src/`, much of it `wasm-bindgen` specific.
- Threads: an isolate is single-threaded, so `threads` needs one isolate per
  thread over a shared backing store. The browser backend has no counterpart,
  since the browser makes a worker's isolate, so this is new code. The trap
  table, the type registry over `wasmparser`, and the JSPI design carry over.

**Verdict.** Full fidelity, with `--wasm-wasmfx`. All nine features and host
suspension. Stack switching is experimental and compiles in one tier only.

[rv8-readme]: https://github.com/denoland/rusty_v8/blob/v152.2.0/README.md
[rv8-flags]:
  https://github.com/denoland/rusty_v8/blob/v152.2.0/src/V8.rs#L164-L170
[rv8-promise]:
  https://github.com/denoland/rusty_v8/blob/v152.2.0/src/promise.rs#L152-L160
[rv8-microtask]:
  https://github.com/denoland/rusty_v8/blob/v152.2.0/src/isolate.rs#L1961-L1975
[rv8-wasm]: https://github.com/denoland/rusty_v8/blob/v152.2.0/src/wasm.rs
[rv8-buildgn]:
  https://github.com/denoland/rusty_v8/blob/v152.2.0/BUILD.gn#L4-L17
[rv8-download]:
  https://github.com/denoland/rusty_v8/blob/v152.2.0/build.rs#L838-L873
[rv8-env]: https://github.com/denoland/rusty_v8/blob/v152.2.0/build.rs#L901-L915
[v8-license]:
  https://github.com/denoland/v8/blob/c4ca1eccb90c5464d826b7713dc27178a53b0cfe/LICENSE#L36-L45
[v8-features]:
  https://github.com/denoland/v8/blob/c4ca1eccb90c5464d826b7713dc27178a53b0cfe/src/wasm/wasm-features.h#L22-L43
[v8-flags]:
  https://github.com/denoland/v8/blob/c4ca1eccb90c5464d826b7713dc27178a53b0cfe/src/wasm/wasm-feature-flags.h
[v8-flags-wasmfx]:
  https://github.com/denoland/v8/blob/c4ca1eccb90c5464d826b7713dc27178a53b0cfe/src/wasm/wasm-feature-flags.h#L49-L52
[v8-flags-wide]:
  https://github.com/denoland/v8/blob/c4ca1eccb90c5464d826b7713dc27178a53b0cfe/src/wasm/wasm-feature-flags.h#L85-L88
[v8-opcodes]:
  https://github.com/denoland/v8/blob/c4ca1eccb90c5464d826b7713dc27178a53b0cfe/src/wasm/wasm-opcodes.h#L260-L267
[v8-liftoff]:
  https://github.com/denoland/v8/blob/c4ca1eccb90c5464d826b7713dc27178a53b0cfe/src/wasm/baseline/liftoff-compiler.cc#L2229-L2285
[v8-turboshaft]:
  https://github.com/denoland/v8/blob/c4ca1eccb90c5464d826b7713dc27178a53b0cfe/src/wasm/turboshaft-graph-interface.cc#L4083-L4471
[chrome-137]: https://developer.chrome.com/blog/new-in-chrome-137
[v8-suspend-error]:
  https://github.com/denoland/v8/blob/c4ca1eccb90c5464d826b7713dc27178a53b0cfe/src/runtime/runtime-wasm.cc#L227-L247
[v8-contref]:
  https://github.com/denoland/v8/blob/c4ca1eccb90c5464d826b7713dc27178a53b0cfe/src/wasm/wasm-objects.cc#L3157-L3161
[v8-exnref]:
  https://github.com/denoland/v8/blob/c4ca1eccb90c5464d826b7713dc27178a53b0cfe/src/wasm/wasm-objects.cc#L3057-L3066
[v8-wee8]:
  https://github.com/denoland/v8/blob/c4ca1eccb90c5464d826b7713dc27178a53b0cfe/BUILD.gn#L7975-L7995
[v8-imports]:
  https://github.com/denoland/v8/blob/c4ca1eccb90c5464d826b7713dc27178a53b0cfe/src/wasm/wasm-module.cc#L364-L450
[v8-spec-update]:
  https://github.com/denoland/v8/blob/c4ca1eccb90c5464d826b7713dc27178a53b0cfe/tools/wasm/update-wasm-spec-tests.sh#L92
[v8-testcfg]:
  https://github.com/denoland/v8/blob/c4ca1eccb90c5464d826b7713dc27178a53b0cfe/test/wasm-spec-tests/testcfg.py#L37-L38
[v8-status]:
  https://github.com/denoland/v8/blob/c4ca1eccb90c5464d826b7713dc27178a53b0cfe/test/wasm-spec-tests/wasm-spec-tests.status#L14-L18
[v8-memory]:
  https://github.com/denoland/v8/blob/c4ca1eccb90c5464d826b7713dc27178a53b0cfe/src/objects/backing-store.cc#L74-L96
[v8-traps]:
  https://github.com/denoland/v8/blob/c4ca1eccb90c5464d826b7713dc27178a53b0cfe/src/common/message-template.h#L713-L742
[nix-rusty-v8]:
  https://github.com/NixOS/nixpkgs/blob/b6c8664de9b6cc07fe5666a29f91884ba81197c4/pkgs/by-name/de/deno/rusty-v8/default.nix

Deno's `deno_core` crate wraps the same V8 with an event loop that drives
promises. It adds no Wasm feature over the `v8` crate and keeps the same JS-only
boundary. Node.js has no Rust embedding crate.

### JavaScriptCore, Embedded Natively

JavaScriptCore (JSC) is the engine of WebKit and Safari. On Linux it ships as
`libjavascriptcoregtk` inside WebKitGTK. JSC is LGPL-2 with many BSD-2 files
([COPYING.LIB][jsc-license]).

JSC takes its Wasm flag defaults from WebKit's shared preference file. A plain
embedder gets the same defaults as the matching Safari
([`GeneratePreferences.rb` lines 120-122][jsc-prefs-gen]). An embedder can set a
flag with the environment variable `JSC_<name>=1` ([`Options.cpp` lines
1095-1124][jsc-options-env]).

**Proposals at WebKitGTK 2.54.** GC, typed function references, `exnref`
exceptions, extended constants, and threads have no flag, so they are always on
([`wasm.json`][jsc-wasm-json]). The flagged features at 2.54.1:

- `tail_call`: on ([YAML lines 9790-9801][jsc-yaml-tail]).

- `multi_memory`: on ([YAML lines 9751-9762][jsc-yaml-mm]).
- `relaxed_simd`: on ([YAML lines 9764-9775][jsc-yaml-rsimd]).
- `memory64`: off, behind `useWasmMemory64` ([YAML lines
  9725-9736][jsc-yaml-m64]).
- `wide_arithmetic`: off, behind `useWasmWideArithmetic` ([YAML lines
  9803-9814][jsc-yaml-wide]).
- `stack_switching`: not implemented. `wasm.json` has no continuation
  instruction and no `contref` type, at 2.54.x or at WebKit `main`.

WebKit `main` (2026-10-08) turns `memory64` and `wide_arithmetic` on ([YAML
lines 6891-6900][jsc-main-m64], [lines 6957-6966][jsc-main-wide]). So `main` has
all of Wasm 3.0 on by default. Safari 27.0 lists JSPI and no other new Wasm
feature ([WebKit features in Safari 27.0][safari-27], fetched 2026-10-08).

These defaults match the project's own record of `jsc --options` on WebKitGTK
2.54: `useWasmMemory64=false`, `useJSPI=true`, multi-memory on.

**Host suspension: meets the contract, through JSPI.** `useJSPI` is on and
stable at 2.54.1 ([YAML lines 4758-4769][jsc-yaml-jspi]). WebKitGTK 2.54.0 is
the first stable release with it. 2.52.4 has none. JSC does not keep a separate
stack per call. On a suspension it copies the Wasm frames between the import and
the `promising` frame to the heap, and replays them on resumption
([`EvacuatedStack.h` lines 43-125][jsc-evacuated]). The cost of a suspension
grows with the number of frames.

- Each `promising` call has its own context, so many calls can wait and resume
  in any order ([`JSPIContext.h` lines 40-59][jsc-jspi-context]).
- JSC raises `SuspendError` when JavaScript frames lie between the import and
  `promising` ([`WebAssemblySuspending.cpp` lines 105-175][jsc-suspend-error]).
- A native host makes the promise with `JSObjectMakeDeferredPromise`
  ([`JSObjectRef.h` line 490][jsc-deferred]) or `jsc_value_new_promise`.
  Microtasks drain when the outermost JavaScript lock is released, at the end of
  each C API call ([`JSLock.cpp` lines 301-315][jsc-jslock]). Asynchronous
  compiles need the GLib main context pumped. The synchronous constructors do
  not.

**Embedding.** The boundary is JS-only, as in the browser backend.

- The GObject API (`jsc/jsc.h`) has no Wasm calls. The classic C API
  (`JavaScriptCore/JavaScript.h`) installs headers only for the 4.1 ABI, but the
  6.0 library still exports its symbols ([`PlatformGTK.cmake` lines
  21-25][jsc-headers]).
- Rust crates: `javascriptcore6` (GNOME, 6.0 ABI, MIT) binds
  `jsc_value_new_promise` ([`sys/src/lib.rs` line 556][jsc-rs6]).
  `javascriptcore-rs` (Tauri) targets the 4.1 ABI and stops at 2.38. `rusty_jsc`
  has had no commit since 2023.
- `JSObjectGetArrayBufferBytesPtr` refuses the buffer of a Wasm memory
  ([`JSTypedArray.cpp` lines 332-348][jsc-buffer]). So memory access copies, as
  in the browser.
- An `exnref` cannot cross to JavaScript. An uncaught exception reaches the host
  as a `WebAssembly.Exception`. Type reflection is off, so types come from
  `wasmparser`.

**Fidelity.** JSC runs the spec tests through the Web Platform Tests import
`wasm/core` ([`JSTests/wasm.yaml` lines 64-79][jsc-wasm-yaml]). The committed
expectations at `main` hold 188,151 passes and 138 failures. All the failures
are cases where a module must fail to compile, in four files
([`call_ref.wast.js-expected.txt`][jsc-wpt-expected] is one). The researcher
counted these from 258 expectation files. WebKit does not publish the figure,
and the testsuite revision of the import is not recorded. "Fast memory" reserves
4 GiB with a signal handler, for 32-bit memories only. A memory64 memory uses
explicit bounds checks ([`WasmMemory.cpp` lines 188-245][jsc-memory]). Traps
carry fixed messages such as "Out of bounds memory access"
([`WasmExceptionType.h` lines 38-75][jsc-traps]). The browser backend already
maps them ([`traps.rs` line 208][wcmp-traps-jsc]).

**Fit with the browsers.** It is Safari's engine. WebKitGTK 2.54 comes from the
Safari 27 cycle, so a 2.54 lane is the closest native stand-in for Safari 27.
WebKit `main` is close to Safari Technology Preview. The differences are the
platform: x86_64 Linux and a signal handler, not Apple silicon and Mach
exceptions.

**Cost.**

- nixpkgs ships `webkitgtk_6_0` 2.54.0 ([`package.nix` line 85][nix-webkit]).
  The flake already uses it for `tests smoke webkit` (`smokeWebkit` in
  [`flake.nix`][wcmp-smoke-webkit]). So a JSC backend adds no new build input.
  The closure of the older 2.52.4 package is 1.2 GiB. The 2.54.0 closure was not
  measured.
- Nothing vendors C++. The binding crates link the system library through
  pkg-config.
- Distance from the trait: the same as V8. The backend is a port of the browser
  backend onto a JSC binding, with declarations of the C API functions the 6.0
  headers omit.

**Verdict.** Not full fidelity: `stack_switching` is missing in every version.
With `JSC_useWasmMemory64=1`, WebKitGTK 2.54 has the other eight features and
host suspension. At 2.54.1 the `useWasmMemory64` option's status is `testable`,
and memory64 is "only supported in the IPInt tier", JSC's interpreter. So the
flag gives memory64 that is neither stable nor compiled, which weakens the case
for this lane as Safari parity for memory64.

[jsc-license]:
  https://github.com/WebKit/WebKit/blob/2808f5b4f299d92496145faa85f15d69a16eca49/Source/JavaScriptCore/COPYING.LIB
[jsc-prefs-gen]:
  https://github.com/WebKit/WebKit/blob/628d6ce1850e17225185a5807a9d0aa375bae2be/Source/WTF/Scripts/GeneratePreferences.rb#L120-L122
[jsc-options-env]:
  https://github.com/WebKit/WebKit/blob/2808f5b4f299d92496145faa85f15d69a16eca49/Source/JavaScriptCore/runtime/Options.cpp#L1095-L1124
[jsc-wasm-json]:
  https://github.com/WebKit/WebKit/blob/2808f5b4f299d92496145faa85f15d69a16eca49/Source/JavaScriptCore/wasm/wasm.json
[jsc-yaml-tail]:
  https://github.com/WebKit/WebKit/blob/628d6ce1850e17225185a5807a9d0aa375bae2be/Source/WTF/Scripts/Preferences/UnifiedWebPreferences.yaml#L9790-L9801
[jsc-yaml-mm]:
  https://github.com/WebKit/WebKit/blob/628d6ce1850e17225185a5807a9d0aa375bae2be/Source/WTF/Scripts/Preferences/UnifiedWebPreferences.yaml#L9751-L9762
[jsc-yaml-rsimd]:
  https://github.com/WebKit/WebKit/blob/628d6ce1850e17225185a5807a9d0aa375bae2be/Source/WTF/Scripts/Preferences/UnifiedWebPreferences.yaml#L9764-L9775
[jsc-yaml-m64]:
  https://github.com/WebKit/WebKit/blob/628d6ce1850e17225185a5807a9d0aa375bae2be/Source/WTF/Scripts/Preferences/UnifiedWebPreferences.yaml#L9725-L9736
[jsc-yaml-wide]:
  https://github.com/WebKit/WebKit/blob/628d6ce1850e17225185a5807a9d0aa375bae2be/Source/WTF/Scripts/Preferences/UnifiedWebPreferences.yaml#L9803-L9814
[jsc-yaml-jspi]:
  https://github.com/WebKit/WebKit/blob/628d6ce1850e17225185a5807a9d0aa375bae2be/Source/WTF/Scripts/Preferences/UnifiedWebPreferences.yaml#L4758-L4769
[jsc-main-m64]:
  https://github.com/WebKit/WebKit/blob/2808f5b4f299d92496145faa85f15d69a16eca49/Source/WTF/Scripts/Preferences/UnifiedWebPreferences.yaml#L6891-L6900
[jsc-main-wide]:
  https://github.com/WebKit/WebKit/blob/2808f5b4f299d92496145faa85f15d69a16eca49/Source/WTF/Scripts/Preferences/UnifiedWebPreferences.yaml#L6957-L6966
[safari-27]: https://webkit.org/blog/18325/webkit-features-for-safari-27-0/
[jsc-evacuated]:
  https://github.com/WebKit/WebKit/blob/2808f5b4f299d92496145faa85f15d69a16eca49/Source/JavaScriptCore/runtime/EvacuatedStack.h#L43-L125
[jsc-jspi-context]:
  https://github.com/WebKit/WebKit/blob/2808f5b4f299d92496145faa85f15d69a16eca49/Source/JavaScriptCore/runtime/JSPIContext.h#L40-L59
[jsc-suspend-error]:
  https://github.com/WebKit/WebKit/blob/2808f5b4f299d92496145faa85f15d69a16eca49/Source/JavaScriptCore/wasm/js/WebAssemblySuspending.cpp#L105-L175
[jsc-deferred]:
  https://github.com/WebKit/WebKit/blob/2808f5b4f299d92496145faa85f15d69a16eca49/Source/JavaScriptCore/API/JSObjectRef.h#L490
[jsc-jslock]:
  https://github.com/WebKit/WebKit/blob/2808f5b4f299d92496145faa85f15d69a16eca49/Source/JavaScriptCore/runtime/JSLock.cpp#L301-L315
[jsc-headers]:
  https://github.com/WebKit/WebKit/blob/webkitgtk-2.52.4/Source/JavaScriptCore/PlatformGTK.cmake#L21-L25
[jsc-rs6]:
  https://gitlab.gnome.org/World/Rust/webkit6-rs/-/blob/082e3e08775be065833d328f57787bf65416c20d/javascriptcore6/sys/src/lib.rs#L556
[jsc-buffer]:
  https://github.com/WebKit/WebKit/blob/2808f5b4f299d92496145faa85f15d69a16eca49/Source/JavaScriptCore/API/JSTypedArray.cpp#L332-L348
[jsc-wasm-yaml]:
  https://github.com/WebKit/WebKit/blob/2808f5b4f299d92496145faa85f15d69a16eca49/JSTests/wasm.yaml#L64-L79
[jsc-wpt-expected]:
  https://github.com/WebKit/WebKit/blob/2808f5b4f299d92496145faa85f15d69a16eca49/LayoutTests/imported/w3c/web-platform-tests/wasm/core/call_ref.wast.js-expected.txt
[jsc-memory]:
  https://github.com/WebKit/WebKit/blob/2808f5b4f299d92496145faa85f15d69a16eca49/Source/JavaScriptCore/wasm/WasmMemory.cpp#L188-L245
[jsc-traps]:
  https://github.com/WebKit/WebKit/blob/2808f5b4f299d92496145faa85f15d69a16eca49/Source/JavaScriptCore/wasm/WasmExceptionType.h#L38-L75
[nix-webkit]:
  https://github.com/NixOS/nixpkgs/blob/b6c8664de9b6cc07fe5666a29f91884ba81197c4/pkgs/by-name/we/webkitgtk_6_0/package.nix#L85

Bun embeds a fork of WebKit, but it has no library API. It can serve only as a
subprocess. nixpkgs ships Bun 1.4.2 ([`package.nix` line 20][nix-bun]).

[nix-bun]:
  https://github.com/NixOS/nixpkgs/blob/b6c8664de9b6cc07fe5666a29f91884ba81197c4/pkgs/by-name/bu/bun/package.nix#L20

### SpiderMonkey, Embedded Natively

SpiderMonkey is the engine of Firefox. The `mozjs` and `mozjs_sys` crates
(repository `servo/mozjs`, MPL-2.0) vendor it. Tag `mozjs-sys-v153.4.0-0`
carries SpiderMonkey 153.4.0, the ESR 153 release build
([`milestone.txt`][sm-milestone]).

**Proposals.** SpiderMonkey gates only the features in `JS_FOR_WASM_FEATURES`
([`WasmFeatures.h` lines 82-170][sm-features]). GC, `exnref` exceptions, tail
calls, `memory64`, `multi_memory`, typed function references, and extended
constants are not in that list, so they are always on.

- `relaxed_simd`: on by default on x86_64 ([`moz.configure` lines
  931-956][sm-configure-simd]).
- `threads`: compiled in. A realm needs `setSharedMemoryAndAtomicsEnabled(true)`
  ([`WasmFeatures.cpp` lines 50-53][sm-threads]).
- `wide_arithmetic`: compiled in, but its preference defaults to Nightly builds
  only, so it is off here ([`StaticPrefList.yaml` lines
  9953-9958][sm-pref-wide]).
- `stack_switching`: off and partial. The preference defaults to false
  ([`StaticPrefList.yaml` lines 9838-9842][sm-pref-ss]). The source states:
  "Implemented: `cont.new`, `resume` (with `on` suspend handlers), `suspend`.
  Not implemented: `switch`, `cont.bind`, `resume_throw`, `resume_throw_ref`,
  cont types with parameters, cont types with results" ([`WasmStacks.cpp` lines
  62-72][sm-stacks]).

The crate does not wrap the `JS::Prefs` setters that turn on a preference. The
backend can need a small C++ shim for them.

**Host suspension: meets the contract, through JSPI.** JSPI shipped on by
default in Firefox 153 ([bug 2044809][sm-jspi-bug], fetched 2026-10-08). The
vendored preference defaults to true ([`StaticPrefList.yaml` lines
9864-9868][sm-pref-jspi]). Each suspended call has its own continuation stack, 1
MiB by default ([`WasmStacks.cpp` lines 88-96][sm-stack-size]). A suspension
without a matching `promising` frame raises `SuspendError` ([`ErrorNumbers.msg`
lines 524-525][sm-suspend-error]). The host resolves the promise, then drains
the job queue with `RunJobs` ([`jsapi2_wrappers.in.rs` lines
33-36][sm-runjobs]). There is no synchronous resume.

**Embedding.** The boundary is JS-only. SpiderMonkey has no native Wasm API
beyond module serialization ([`WasmModule.h` lines 38-40][sm-wasm-module]).

- Every GC thing must be rooted, and most of the API is `unsafe`.
- The engine can be initialized once per process ([`rust.rs` lines
  166-170][sm-init]).
- An `exnref` and a `contref` cannot cross as values ([`WasmValue.cpp` lines
  328-340][sm-value]). GC objects cross opaquely.
- Type reflection is a Nightly feature, off in this build ([`moz.configure`
  lines 1301-1322][sm-reflect]).

**Fidelity.** Firefox converts the spec tests into its own test format, pinned
to WebAssembly/spec `f500d7fbb` and stack-switching `8fe7145d2`
([`config-lock.toml`][sm-spec-lock]). It publishes no pass rate. The `mozjs`
crate strips those tests from its tree ([`filters.txt` line 3][sm-filters]).
Each 32-bit memory reserves 4 GiB plus a 32 MiB guard ([`WasmMemory.h` lines
224-253][sm-memory]). Traps reach the host as `WebAssembly.RuntimeError` with
messages such as "unreachable executed" ([`ErrorNumbers.msg` lines
448-468][sm-traps]). Stack overflow is an `InternalError` with "too much
recursion". The browser backend already maps these ([`traps.rs` lines
149-158][wcmp-traps-sm]).

**Fit with the browsers.** It is Firefox's engine. The project weighs Firefox
low.

**Cost.**

- Build: by default `build.rs` downloads an 18.1 MB prebuilt archive
  ([`build.rs` lines 1246-1285][sm-download]). `MOZJS_ARCHIVE` takes a local
  path ([`build.rs` lines 81-86][sm-archive-env]). Otherwise it builds all of
  SpiderMonkey from C++ with clang and Python. The build time is not documented.
- nixpkgs has `spidermonkey_128` and `spidermonkey_140` only
  ([`all-packages.nix` lines 4586-4587][nix-sm]). `mozjs_sys` cannot link a
  system `libmozjs`, so these do not help.
- Each ESR security release brings a new crate release.
- Distance from the trait: the same as V8, plus manual rooting.

**Verdict.** Not full fidelity: `stack_switching` is partial and off. Eight
features and host suspension are present.

[sm-milestone]:
  https://github.com/servo/mozjs/blob/34ad101f42bafbb197cb27b4b0d343469f7799e7/mozjs-sys/mozjs/config/milestone.txt
[sm-features]:
  https://github.com/servo/mozjs/blob/34ad101f42bafbb197cb27b4b0d343469f7799e7/mozjs-sys/mozjs/js/public/WasmFeatures.h#L82-L170
[sm-configure-simd]:
  https://github.com/servo/mozjs/blob/34ad101f42bafbb197cb27b4b0d343469f7799e7/mozjs-sys/mozjs/js/moz.configure#L931-L956
[sm-threads]:
  https://github.com/servo/mozjs/blob/34ad101f42bafbb197cb27b4b0d343469f7799e7/mozjs-sys/mozjs/js/src/wasm/WasmFeatures.cpp#L50-L53
[sm-pref-wide]:
  https://github.com/servo/mozjs/blob/34ad101f42bafbb197cb27b4b0d343469f7799e7/mozjs-sys/mozjs/modules/libpref/init/StaticPrefList.yaml#L9953-L9958
[sm-pref-ss]:
  https://github.com/servo/mozjs/blob/34ad101f42bafbb197cb27b4b0d343469f7799e7/mozjs-sys/mozjs/modules/libpref/init/StaticPrefList.yaml#L9838-L9842
[sm-stacks]:
  https://github.com/servo/mozjs/blob/34ad101f42bafbb197cb27b4b0d343469f7799e7/mozjs-sys/mozjs/js/src/wasm/WasmStacks.cpp#L62-L72
[sm-jspi-bug]: https://bugzilla.mozilla.org/show_bug.cgi?id=2044809
[sm-pref-jspi]:
  https://github.com/servo/mozjs/blob/34ad101f42bafbb197cb27b4b0d343469f7799e7/mozjs-sys/mozjs/modules/libpref/init/StaticPrefList.yaml#L9864-L9868
[sm-stack-size]:
  https://github.com/servo/mozjs/blob/34ad101f42bafbb197cb27b4b0d343469f7799e7/mozjs-sys/mozjs/js/src/wasm/WasmStacks.cpp#L88-L96
[sm-suspend-error]:
  https://github.com/servo/mozjs/blob/34ad101f42bafbb197cb27b4b0d343469f7799e7/mozjs-sys/mozjs/js/public/friend/ErrorNumbers.msg#L524-L525
[sm-runjobs]:
  https://github.com/servo/mozjs/blob/34ad101f42bafbb197cb27b4b0d343469f7799e7/mozjs/src/jsapi2_wrappers.in.rs#L33-L36
[sm-wasm-module]:
  https://github.com/servo/mozjs/blob/34ad101f42bafbb197cb27b4b0d343469f7799e7/mozjs-sys/mozjs/js/public/WasmModule.h#L38-L40
[sm-init]:
  https://github.com/servo/mozjs/blob/34ad101f42bafbb197cb27b4b0d343469f7799e7/mozjs/src/rust.rs#L166-L170
[sm-value]:
  https://github.com/servo/mozjs/blob/34ad101f42bafbb197cb27b4b0d343469f7799e7/mozjs-sys/mozjs/js/src/wasm/WasmValue.cpp#L328-L340
[sm-reflect]:
  https://github.com/servo/mozjs/blob/34ad101f42bafbb197cb27b4b0d343469f7799e7/mozjs-sys/mozjs/js/moz.configure#L1301-L1322
[sm-spec-lock]:
  https://github.com/mozilla-firefox/firefox/blob/FIREFOX_153_4_0esr_RELEASE/js/src/jit-test/etc/wasm/generate-spectests/config-lock.toml
[sm-filters]:
  https://github.com/servo/mozjs/blob/34ad101f42bafbb197cb27b4b0d343469f7799e7/mozjs-sys/etc/filters.txt#L3
[sm-memory]:
  https://github.com/servo/mozjs/blob/34ad101f42bafbb197cb27b4b0d343469f7799e7/mozjs-sys/mozjs/js/src/wasm/WasmMemory.h#L224-L253
[sm-traps]:
  https://github.com/servo/mozjs/blob/34ad101f42bafbb197cb27b4b0d343469f7799e7/mozjs-sys/mozjs/js/public/friend/ErrorNumbers.msg#L448-L468
[sm-download]:
  https://github.com/servo/mozjs/blob/34ad101f42bafbb197cb27b4b0d343469f7799e7/mozjs-sys/build.rs#L1246-L1285
[sm-archive-env]:
  https://github.com/servo/mozjs/blob/34ad101f42bafbb197cb27b4b0d343469f7799e7/mozjs-sys/build.rs#L81-L86
[nix-sm]:
  https://github.com/NixOS/nixpkgs/blob/b6c8664de9b6cc07fe5666a29f91884ba81197c4/pkgs/top-level/all-packages.nix#L4586-L4587

### WasmEdge

WasmEdge is a C++ engine of the CNCF, with an interpreter and LLVM-based AOT and
JIT tiers. It is Apache-2.0 ([LICENSE][we-license]). The latest release is
0.18.0, 2026-10-05 ([Changelog][we-changelog]).

**Proposals.** The default configuration turns on all of Wasm 3.0 except text
annotations ([`configure.h` lines 237-256][we-configure]). That covers
`multi_memory`, `memory64`, `tail_call`, `exceptions` (only the `exnref` form),
`function_references`, `gc`, `relaxed_simd`, and extended constants.

- `threads` is off by default, behind `Proposal::Threads` ([`toolConfig.cpp`
  lines 84-90][we-threads]).
- `stack_switching` is not implemented. The roadmap lists it as inactive
  ([`ROADMAP.md` lines 68-69][we-roadmap-ss]).
- `wide_arithmetic` is not implemented. A loader test asserts that its opcodes
  are invalid ([`instructionTest.cpp` lines 887-888][we-wide]).
- GC has no collector. Structs, arrays, and exceptions live in vectors that the
  module instance frees when it is destroyed ([`module.h` lines
  331-349][we-gc]).

**Host suspension: no engine support.** The C API "async" call runs the whole
call on a detached OS thread ([`async.h`][we-async]). A host function returns
its result at once and cannot answer "not yet" ([`wasmedge_instance.h` lines
545-547][we-hostfunc]). The Rust SDK's `async` feature runs a call on a fiber,
in Wasmtime's older design ([`fiber.rs`][we-fiber]). The executor saves its
per-thread state in last-in-first-out order ([`helper.cpp` lines
29-90][we-tls]), so the researcher judged out-of-order resume on one OS thread
unsafe. This was read from the code, not tested. One OS thread per waiting call
meets the contract, at the cost of a thread and a lock, so the comparison table
marks host suspension partial.

**Embedding.** The native C API links imports by registered module name, not by
an ordered list ([`wasmedge_execution.h` lines 165-183][we-instantiate]). The
host cannot make a tag. An uncaught exception reaches the host only as the error
`UncaughtException`, with its payload discarded ([`engine.cpp` lines
65-72][we-uncaught]). Value types expose only numbers, `v128`, `funcref`, and
`externref` ([`wasmedge_value.h` lines 21-37][we-valtype]). GC references pass
back only as opaque bits. The new standard `wasm.h` (0.18.0) takes an ordered
import list, but has no tags or exceptions ([`wasm.h` lines 188-195][we-wasmh]).
The Rust crates track 0.17.1 and have no tag API ([`wasmedge-types` lines
45-68][we-rs-types]).

**Fidelity.** CI runs the spec tests from the mirror tagged `wasm-core-20260922`
in the interpreter, AOT, and JIT ([`CMakeLists.txt` lines 9-40][we-spec]). It
publishes no pass rate, and the harness does not check the expected trap kind
([`spectest.cpp` line 1158][we-spec-todo]). Each memory reserves 12 GiB of
guarded address space ([`allocator.cpp` lines 25-106][we-memory]). Trap messages
follow the spec-test wording ([`enum.inc` lines 1386-1443][we-traps]).

**Fit with the browsers.** None.

**Cost.** nixpkgs ships WasmEdge 0.17.1 ([`package.nix` line 20][nix-we]). The
`wasmedge-sys` crate links an installed library or downloads a release tarball
([`build.rs` lines 12-50][we-rs-build]). Under Nix, `WASMEDGE_DIR` must point at
a Nix-built library. Distance from the trait: large. The backend needs its own
`-sys` binding for tags, exceptions, and GC types, and the C API still lacks
host tags and `exnref`.

**Verdict.** Not full fidelity. Missing `stack_switching`, and host suspension
is partial (one OS thread per waiting call).

[we-license]:
  https://github.com/WasmEdge/WasmEdge/blob/87b11e6f7d718fec20d30a7d591b1cae2346e104/LICENSE#L1
[we-changelog]:
  https://github.com/WasmEdge/WasmEdge/blob/87b11e6f7d718fec20d30a7d591b1cae2346e104/Changelog.md#L1
[we-configure]:
  https://github.com/WasmEdge/WasmEdge/blob/87b11e6f7d718fec20d30a7d591b1cae2346e104/include/common/configure.h#L237-L256
[we-threads]:
  https://github.com/WasmEdge/WasmEdge/blob/87b11e6f7d718fec20d30a7d591b1cae2346e104/lib/driver/toolConfig.cpp#L84-L90
[we-roadmap-ss]:
  https://github.com/WasmEdge/WasmEdge/blob/87b11e6f7d718fec20d30a7d591b1cae2346e104/docs/ROADMAP.md#L68-L69
[we-wide]:
  https://github.com/WasmEdge/WasmEdge/blob/87b11e6f7d718fec20d30a7d591b1cae2346e104/test/loader/instructionTest.cpp#L887-L888
[we-gc]:
  https://github.com/WasmEdge/WasmEdge/blob/87b11e6f7d718fec20d30a7d591b1cae2346e104/include/runtime/instance/module.h#L331-L349
[we-async]:
  https://github.com/WasmEdge/WasmEdge/blob/87b11e6f7d718fec20d30a7d591b1cae2346e104/include/common/async.h
[we-hostfunc]:
  https://github.com/WasmEdge/WasmEdge/blob/87b11e6f7d718fec20d30a7d591b1cae2346e104/include/api/wasmedge/wasmedge_instance.h#L545-L547
[we-fiber]:
  https://github.com/WasmEdge/wasmedge-rust-sdk/blob/37e693cfe0c47f40642d029812d5824762f9c3a1/crates/wasmedge-sys/src/async/fiber.rs
[we-tls]:
  https://github.com/WasmEdge/WasmEdge/blob/87b11e6f7d718fec20d30a7d591b1cae2346e104/lib/executor/helper.cpp#L29-L90
[we-instantiate]:
  https://github.com/WasmEdge/WasmEdge/blob/87b11e6f7d718fec20d30a7d591b1cae2346e104/include/api/wasmedge/wasmedge_execution.h#L165-L183
[we-uncaught]:
  https://github.com/WasmEdge/WasmEdge/blob/87b11e6f7d718fec20d30a7d591b1cae2346e104/lib/executor/engine/engine.cpp#L65-L72
[we-valtype]:
  https://github.com/WasmEdge/WasmEdge/blob/87b11e6f7d718fec20d30a7d591b1cae2346e104/include/api/wasmedge/wasmedge_value.h#L21-L37
[we-wasmh]:
  https://github.com/WasmEdge/WasmEdge/blob/87b11e6f7d718fec20d30a7d591b1cae2346e104/thirdparty/wasm/wasm.h#L188-L195
[we-rs-types]:
  https://github.com/WasmEdge/wasmedge-rust-sdk/blob/37e693cfe0c47f40642d029812d5824762f9c3a1/crates/wasmedge-types/src/lib.rs#L45-L68
[we-spec]:
  https://github.com/WasmEdge/WasmEdge/blob/87b11e6f7d718fec20d30a7d591b1cae2346e104/test/spec/CMakeLists.txt#L9-L40
[we-spec-todo]:
  https://github.com/WasmEdge/WasmEdge/blob/87b11e6f7d718fec20d30a7d591b1cae2346e104/test/spec/spectest.cpp#L1158
[we-memory]:
  https://github.com/WasmEdge/WasmEdge/blob/87b11e6f7d718fec20d30a7d591b1cae2346e104/lib/system/allocator.cpp#L25-L106
[we-traps]:
  https://github.com/WasmEdge/WasmEdge/blob/87b11e6f7d718fec20d30a7d591b1cae2346e104/include/common/enum.inc#L1386-L1443
[nix-we]:
  https://github.com/NixOS/nixpkgs/blob/b6c8664de9b6cc07fe5666a29f91884ba81197c4/pkgs/by-name/wa/wasmedge/package.nix#L20
[we-rs-build]:
  https://github.com/WasmEdge/wasmedge-rust-sdk/blob/37e693cfe0c47f40642d029812d5824762f9c3a1/crates/wasmedge-sys/build.rs#L12-L50

### Wasmer

Wasmer is a Rust engine with three compilers: Cranelift, LLVM, and Singlepass.
It also has a `v8` backend and a `js` backend for the browser. It is MIT
([LICENSE][wr-license]). The latest release is 7.5.0, 2026-10-01
([CHANGELOG][wr-changelog]). Wasmer 7.2 dropped its WAMR and Wasmi backends
([CHANGELOG line 437][wr-drop]).

**Proposals.** The `Features` struct has no field for `gc`,
`function_references`, or `stack_switching` ([`features.rs` lines
16-43][wr-features]). Each compiler turns on what it supports ([`config.rs`
lines 361-373][wr-cranelift], [LLVM `config.rs` lines 450-457][wr-llvm]).

- On with Cranelift and LLVM: `multi_memory`, `exceptions` (`exnref` form),
  `relaxed_simd`, `threads`, extended constants, `wide_arithmetic`.
- `tail_call`: LLVM only. Cranelift returns "proposed tail-call operator" as
  unsupported ([`code_translator.rs` lines 2510-2512][wr-tail]).
- `memory64`: not implemented. The translator calls
  `unimplemented!("64bit memory not implemented yet")` ([`sections.rs` lines
  153-155][wr-m64]).
- `gc`, `function_references`, `stack_switching`: not implemented. Cranelift has
  `todo!()` for the continuation operators ([`code_translator.rs` lines
  2597-2600][wr-ss]).

The webassembly.org table gives Wasmer "stackSwitching 7.0" ([`features.json`
line 634][wf-wasmer-ss]). The source does not support that entry. Wasmer 7.0
added a WASIX context-switching API, which is not the proposal.

**Host suspension: partly.** The experimental feature `experimental-async` gives
`Function::new_async` and `call_async` ([`function/mod.rs` lines
159-229][wr-async-api]). Each call runs in its own coroutine, and the store lock
drops while it waits ([`async_runtime.rs` lines 160-236][wr-async-runtime]). So
many calls can wait on one store and resume in any order. But the coroutine also
suspends through host frames ([`jspi_async.rs` lines 248-319][wr-nested]), so
the backend must enforce the "only Wasm frames" rule itself. The `v8` backend
does not support it ([`inner.rs` line 230][wr-v8-async]).

**Embedding.** The Rust API is close to Wasmtime's. `Instance::new_by_index`
takes an ordered list of externs ([`instance.rs` lines 95-99][wr-instance]).
`Tag`, `Exception`, and `Value::ExceptionRef` exist ([`value.rs` lines
16-45][wr-value]). There is no `anyref`, `i31`, or `contref`, and value types
have no concrete heap types ([`types.rs` lines 25-42][wr-types]). A borrowed
`&[u8]` of memory needs `unsafe data_unchecked` ([`view/mod.rs` lines
28-137][wr-view]).

**Fidelity.** The spec tests run against testsuite `67c8b9dc`, through a list of
allowed failures that swallows GC, typed reference, and `memory64` errors
([`wast.rs` lines 7-171][wr-wast]). `docs/TEST.md` claims "100% compatibility"
([lines 3-4][wr-test-md]) under that list. Memory is a mapped 8 GiB reservation
([`memory.rs` lines 16-44][wr-memory]). `TrapCode` has 16 kinds and no null,
cast, or array traps ([`trapcode.rs` lines 33-89][wr-traps]).

**Fit with the browsers.** None. The `v8` backend embeds V8 11.9, from 2023.

**Cost.** nixpkgs ships Wasmer 7.4.2 ([`package.nix` line 64][nix-wr]). The
Cranelift tier is pure Rust. LLVM needs a system LLVM 22. The `v8` feature
downloads a prebuilt library at build time ([`build.rs` lines
10-75][wr-v8-build]). Distance from the trait: medium for the API shape, but GC
and `memory64` are absent from the engine itself.

**Verdict.** Not full fidelity. Missing `memory64`, `gc`, `function_references`,
and `stack_switching`. `tail_call` needs LLVM. Host suspension is partial.

[wr-license]:
  https://github.com/wasmerio/wasmer/blob/2b2e6851832ded446e4eb52520c26e6ff305d449/LICENSE#L1-L3
[wr-changelog]:
  https://github.com/wasmerio/wasmer/blob/2b2e6851832ded446e4eb52520c26e6ff305d449/CHANGELOG.md#L9
[wr-drop]:
  https://github.com/wasmerio/wasmer/blob/2b2e6851832ded446e4eb52520c26e6ff305d449/CHANGELOG.md#L437
[wr-features]:
  https://github.com/wasmerio/wasmer/blob/2b2e6851832ded446e4eb52520c26e6ff305d449/lib/types/src/features.rs#L16-L43
[wr-cranelift]:
  https://github.com/wasmerio/wasmer/blob/2b2e6851832ded446e4eb52520c26e6ff305d449/lib/compiler-cranelift/src/config.rs#L361-L373
[wr-llvm]:
  https://github.com/wasmerio/wasmer/blob/2b2e6851832ded446e4eb52520c26e6ff305d449/lib/compiler-llvm/src/config.rs#L450-L457
[wr-tail]:
  https://github.com/wasmerio/wasmer/blob/2b2e6851832ded446e4eb52520c26e6ff305d449/lib/compiler-cranelift/src/translator/code_translator.rs#L2510-L2512
[wr-m64]:
  https://github.com/wasmerio/wasmer/blob/2b2e6851832ded446e4eb52520c26e6ff305d449/lib/compiler/src/translator/sections.rs#L153-L155
[wr-ss]:
  https://github.com/wasmerio/wasmer/blob/2b2e6851832ded446e4eb52520c26e6ff305d449/lib/compiler-cranelift/src/translator/code_translator.rs#L2597-L2600
[wr-async-api]:
  https://github.com/wasmerio/wasmer/blob/2b2e6851832ded446e4eb52520c26e6ff305d449/lib/api/src/entities/function/mod.rs#L159-L229
[wr-async-runtime]:
  https://github.com/wasmerio/wasmer/blob/2b2e6851832ded446e4eb52520c26e6ff305d449/lib/api/src/backend/sys/async_runtime.rs#L160-L236
[wr-nested]:
  https://github.com/wasmerio/wasmer/blob/2b2e6851832ded446e4eb52520c26e6ff305d449/lib/api/tests/jspi_async.rs#L248-L319
[wr-v8-async]:
  https://github.com/wasmerio/wasmer/blob/2b2e6851832ded446e4eb52520c26e6ff305d449/lib/api/src/entities/function/inner.rs#L230
[wr-instance]:
  https://github.com/wasmerio/wasmer/blob/2b2e6851832ded446e4eb52520c26e6ff305d449/lib/api/src/entities/instance.rs#L95-L99
[wr-value]:
  https://github.com/wasmerio/wasmer/blob/2b2e6851832ded446e4eb52520c26e6ff305d449/lib/api/src/entities/value.rs#L16-L45
[wr-types]:
  https://github.com/wasmerio/wasmer/blob/2b2e6851832ded446e4eb52520c26e6ff305d449/lib/types/src/types.rs#L25-L42
[wr-view]:
  https://github.com/wasmerio/wasmer/blob/2b2e6851832ded446e4eb52520c26e6ff305d449/lib/api/src/entities/memory/view/mod.rs#L28-L137
[wr-wast]:
  https://github.com/wasmerio/wasmer/blob/2b2e6851832ded446e4eb52520c26e6ff305d449/tests/compilers/wast.rs#L7-L171
[wr-test-md]:
  https://github.com/wasmerio/wasmer/blob/2b2e6851832ded446e4eb52520c26e6ff305d449/docs/TEST.md#L3-L4
[wr-memory]:
  https://github.com/wasmerio/wasmer/blob/2b2e6851832ded446e4eb52520c26e6ff305d449/lib/types/src/memory.rs#L16-L44
[wr-traps]:
  https://github.com/wasmerio/wasmer/blob/2b2e6851832ded446e4eb52520c26e6ff305d449/lib/types/src/trapcode.rs#L33-L89
[nix-wr]:
  https://github.com/NixOS/nixpkgs/blob/b6c8664de9b6cc07fe5666a29f91884ba81197c4/pkgs/by-name/wa/wasmer/package.nix#L64
[wr-v8-build]:
  https://github.com/wasmerio/wasmer/blob/2b2e6851832ded446e4eb52520c26e6ff305d449/lib/api/build.rs#L10-L75

### WAMR

WAMR (WebAssembly Micro Runtime) is a C engine for small devices, with two
interpreters, AOT, and two JIT tiers. It moved to the `wasm-micro-runtime`
GitHub organization in August 2026 ([README lines 14-19][wamr-readme]). It is
Apache-2.0 with the LLVM exception ([LICENSE][wamr-license]). The latest release
is 2.4.5, 2026-06-29.

**Proposals.** Every feature is a compile-time CMake option, and the tiers
support different sets ([`stability_wasm_proposals.md` lines
13-64][wamr-stability]).

- `exceptions`: only the legacy proposal, in the classic interpreter. Opcode
  `0x0a` (`throw_ref`) is unused ([`wasm_opcode.h` lines 22-27][wamr-opcodes]).
- `relaxed_simd`, `stack_switching`, `wide_arithmetic`: not implemented.
- `gc` and `function_references`: present but "not fully compliant", with no
  `exn` type ([`build_wamr.md` lines 362-377][wamr-gc]).
- `multi_memory`: classic interpreter only, per the docs.
- `memory64`: classic interpreter and AOT only.
- `tail_call`, `threads`, extended constants: present, behind build options.

**Host suspension: no.** WAMR has no resumable call, fiber, or async host API. A
host function is a plain C function pointer ([`lib_export.h` lines
20-27][wamr-native]).

**Embedding.** One runtime per process. Host functions are registered globally
by module name. The standard C API takes an ordered import list but ignores
imported memories and tables ([`wasm_c_api.c` lines 4928-4934][wamr-capi]).
There is no tag kind and no `exnref`. An uncaught exception reaches the host
only as the string "Exception: uncaught wasm exception". `gc_export.h` gives a
usable host API for GC objects. The `wamr-rust-sdk` is not on crates.io, pins
WAMR 2.3.1, and builds without GC, exceptions, or threads ([`build.rs` lines
126-139][wamr-rs-build]).

**Fidelity.** No pass rate is published. CI runs testsuites pinned from 2023 to
2025, after patches that skip cases ([`test_wamr.sh` lines
560-660][wamr-tests]). Each memory reserves 8 GiB on 64-bit Linux
([`wasm_memory.c` lines 2060-2120][wamr-memory]). Traps are strings
([`wasm_runtime_common.c` lines 3236-3273][wamr-traps]).

**Fit with the browsers.** None.

**Cost.** nixpkgs ships WAMR 2.4.4 with SIMD off ([`package.nix` line
22][nix-wamr]). The backend needs its own CMake build and `-sys` crate.

**Verdict.** Not full fidelity. Missing `exceptions` (`exnref`), `relaxed_simd`,
`stack_switching`, and host suspension. `gc` is partial.

[wamr-readme]:
  https://github.com/wasm-micro-runtime/wasm-micro-runtime/blob/38b044a676893e678ceb19babaa431ca5e460114/README.md#L14-L19
[wamr-license]:
  https://github.com/wasm-micro-runtime/wasm-micro-runtime/blob/38b044a676893e678ceb19babaa431ca5e460114/LICENSE#L204
[wamr-stability]:
  https://github.com/wasm-micro-runtime/wasm-micro-runtime/blob/38b044a676893e678ceb19babaa431ca5e460114/doc/stability_wasm_proposals.md#L13-L64
[wamr-opcodes]:
  https://github.com/wasm-micro-runtime/wasm-micro-runtime/blob/38b044a676893e678ceb19babaa431ca5e460114/core/iwasm/interpreter/wasm_opcode.h#L22-L27
[wamr-gc]:
  https://github.com/wasm-micro-runtime/wasm-micro-runtime/blob/38b044a676893e678ceb19babaa431ca5e460114/doc/build_wamr.md#L362-L377
[wamr-native]:
  https://github.com/wasm-micro-runtime/wasm-micro-runtime/blob/38b044a676893e678ceb19babaa431ca5e460114/core/iwasm/include/lib_export.h#L20-L27
[wamr-capi]:
  https://github.com/wasm-micro-runtime/wasm-micro-runtime/blob/38b044a676893e678ceb19babaa431ca5e460114/core/iwasm/common/wasm_c_api.c#L4928-L4934
[wamr-rs-build]:
  https://github.com/wasm-micro-runtime/wamr-rust-sdk/blob/d2508adf15171e40473958067d0a5b91a5b61b45/crates/wamr-sys/build.rs#L126-L139
[wamr-tests]:
  https://github.com/wasm-micro-runtime/wasm-micro-runtime/blob/38b044a676893e678ceb19babaa431ca5e460114/tests/wamr-test-suites/test_wamr.sh#L560-L660
[wamr-memory]:
  https://github.com/wasm-micro-runtime/wasm-micro-runtime/blob/38b044a676893e678ceb19babaa431ca5e460114/core/iwasm/common/wasm_memory.c#L2060-L2120
[wamr-traps]:
  https://github.com/wasm-micro-runtime/wasm-micro-runtime/blob/38b044a676893e678ceb19babaa431ca5e460114/core/iwasm/common/wasm_runtime_common.c#L3236-L3273
[nix-wamr]:
  https://github.com/NixOS/nixpkgs/blob/b6c8664de9b6cc07fe5666a29f91884ba81197c4/pkgs/by-name/wa/wamr/package.nix#L22

### Wizard

Wizard is a research engine written in Virgil, a language by the same author.
Both are Apache-2.0 ([LICENSE][wz-license]). Neither repository has a release or
a tag.

**Proposals.** The default set is Wasm 3.0 ([`Extension.v3` lines
25-38][wz-ext]). It turns on `multi_memory`, `memory64`, `tail_call`,
`exceptions` (`exnref`), `function_references`, `gc`, `relaxed_simd`, and
extended constants.

- `stack_switching`: implemented in every tier, `switch` included, behind
  `--ext:stack-switching` ([`Opcodes.v3` lines 611-617][wz-opcodes]). It is
  under active work.
- `wide_arithmetic`: behind `--ext:wide-arithmetic`.
- `threads`: partial. The compiler tier has none, and the engine spawns no
  threads ([README line 33][wz-readme-threads]).

**Host suspension: no.** A host function returns a value, a throw, or a tail
call, and nothing else ([`Host.v3` lines 24-31][wz-host]).

**Embedding: none for Rust.** Wizard has no C API, no library build, and no
crate. Host functions are Virgil closures compiled into the `wizeng` binary
([`Host.v3` lines 11-22][wz-host]). Virgil can emit an object file, but it loads
at a fixed address and needs a custom linker script ([`Linking.md` lines
213-241][virgil-linking]).

**Fidelity.** CI runs the core spec tests at the unpinned head of
WebAssembly/spec and publishes no pass rate. The runner ignores the expected
trap message ([`SpecTestInterpreter.v3` lines 90-103][wz-spec]). Memory is a
mapped reservation with guard pages on x86_64 Linux ([`NativeMemory.v3` lines
4-35][wz-memory]). Traps are an enum rendered as `trap[MEMORY_OOB]` ([`Trap.v3`
lines 94-105][wz-traps]).

**Fit with the browsers.** None.

**Cost.** Neither Virgil nor Wizard is in nixpkgs. Packaging needs two
derivations pinned together. The backend needs a new FFI or RPC layer written in
Virgil first.

**Verdict.** Not full fidelity. Eight features are complete and `threads` is
partial. Host suspension and an embedding API are missing. Wizard is useful as a
second test oracle for stack switching, not as a backend.

[wz-license]:
  https://github.com/titzer/wizard-engine/blob/fb8300acf10caa9c0fc8c741ca7fc049e8b5ad7b/src/LICENSE
[wz-ext]:
  https://github.com/titzer/wizard-engine/blob/fb8300acf10caa9c0fc8c741ca7fc049e8b5ad7b/src/engine/Extension.v3#L25-L38
[wz-opcodes]:
  https://github.com/titzer/wizard-engine/blob/fb8300acf10caa9c0fc8c741ca7fc049e8b5ad7b/src/engine/Opcodes.v3#L611-L617
[wz-readme-threads]:
  https://github.com/titzer/wizard-engine/blob/fb8300acf10caa9c0fc8c741ca7fc049e8b5ad7b/README.md#L33
[wz-host]:
  https://github.com/titzer/wizard-engine/blob/fb8300acf10caa9c0fc8c741ca7fc049e8b5ad7b/src/engine/Host.v3#L11-L31
[virgil-linking]:
  https://github.com/titzer/virgil/blob/a3a6ab7b5d5350b5928b71d29a9f4e829c2a2956/doc/tutorial/Linking.md#L213-L241
[wz-spec]:
  https://github.com/titzer/wizard-engine/blob/fb8300acf10caa9c0fc8c741ca7fc049e8b5ad7b/test/wasm-spec/SpecTestInterpreter.v3#L90-L103
[wz-memory]:
  https://github.com/titzer/wizard-engine/blob/fb8300acf10caa9c0fc8c741ca7fc049e8b5ad7b/src/engine/native/NativeMemory.v3#L4-L35
[wz-traps]:
  https://github.com/titzer/wizard-engine/blob/fb8300acf10caa9c0fc8c741ca7fc049e8b5ad7b/src/engine/Trap.v3#L94-L105

### Wasm3

Wasm3 is a C interpreter, MIT licensed ([LICENSE][w3-license]). Its README still
says it is in a "minimal maintenance phase" ([README lines 1-3][w3-readme]), but
its `main` still takes proposal work, as the next paragraph shows.

**Proposals.** Upstream `main` added stack switching on 2026-09-23, in commit
`d59778a` ([issue 583][w3-issue]). The instruction table covers all seven
instructions ([`m3_compile.c` lines 5698-5704][w3-opcodes]). The newest tag,
`v0.9.1-beta.1`, comes before it. This confirms the project's own claim that
Wasm3 implements the proposal, for unreleased `main` only. Wasm3 also has
`exnref` exceptions, `tail_call`, `memory64`, `multi_memory`, extended
constants, and `wide_arithmetic`. It has no `gc` and no SIMD at all ([README
line 66][w3-readme-66]). `threads` is atomics only ([`m3_config.h` lines
275-280][w3-atomics]), and typed function references are partial ([`m3_config.h`
lines 310-315][w3-typedrefs]).

**Host suspension: partly.** `m3_RequestSuspend` and `m3_ResumeRuntime` allow
one paused call per runtime, with no handle ([`m3_env.c` lines
3068-3100][w3-suspend]). The pause lands at the next loop or function entry, so
a host function cannot answer "not yet".

**Embedding.** A C API of its own, with tags and exceptions ([`wasm3.h` lines
486-525][w3-tags]). An `exnref` lives only until the outermost call returns. No
API lists imports or exports. The Rust crate `wasm3` stopped at 0.3.1 in 2021.

**Fidelity.** It claims to pass the Wasm 3.0 core suite, with a blacklist. CI
does not run the stack-switching proposal tests. Memory comes from `malloc` by
default ([`m3_core.c` lines 194-230][w3-memory]).

**Cost.** nixpkgs removed Wasm3 as "unmaintained upstream and has many known
vulnerabilities" ([`aliases.nix` line 2729][nix-w3]). The C build is small.

**Verdict.** Not full fidelity. Missing `gc`, `relaxed_simd`, and full
`threads`. Host suspension is partial.

[w3-license]:
  https://github.com/wasm3/wasm3/blob/28ecb9af6d2040e474a70f7cb7f43666740141fb/LICENSE
[w3-readme]:
  https://github.com/wasm3/wasm3/blob/28ecb9af6d2040e474a70f7cb7f43666740141fb/README.md#L1-L3
[w3-issue]: https://github.com/wasm3/wasm3/issues/583
[w3-opcodes]:
  https://github.com/wasm3/wasm3/blob/28ecb9af6d2040e474a70f7cb7f43666740141fb/source/m3_compile.c#L5698-L5704
[w3-readme-66]:
  https://github.com/wasm3/wasm3/blob/28ecb9af6d2040e474a70f7cb7f43666740141fb/README.md#L66
[w3-typedrefs]:
  https://github.com/wasm3/wasm3/blob/28ecb9af6d2040e474a70f7cb7f43666740141fb/source/m3_config.h#L310-L315
[w3-atomics]:
  https://github.com/wasm3/wasm3/blob/28ecb9af6d2040e474a70f7cb7f43666740141fb/source/m3_config.h#L275-L280
[w3-suspend]:
  https://github.com/wasm3/wasm3/blob/28ecb9af6d2040e474a70f7cb7f43666740141fb/source/m3_env.c#L3068-L3100
[w3-tags]:
  https://github.com/wasm3/wasm3/blob/28ecb9af6d2040e474a70f7cb7f43666740141fb/source/wasm3.h#L486-L525
[w3-memory]:
  https://github.com/wasm3/wasm3/blob/28ecb9af6d2040e474a70f7cb7f43666740141fb/source/m3_core.c#L194-L230
[nix-w3]:
  https://github.com/NixOS/nixpkgs/blob/b6c8664de9b6cc07fe5666a29f91884ba81197c4/pkgs/top-level/aliases.nix#L2729

### Engines Excluded After a Screen

Each of these engines fails the bar on a point that no backend work can fix.

- Wasmi roadmap: the issues for GC ([#775][wasmi-gc]), typed function references
  ([#774][wasmi-fr]), and threads ([#777][wasmi-threads]) have been open
  since 2023. Exception handling ([#1137][wasmi-eh]) has been open since 2024.
  None has an assignee or a pull request, and no issue covers stack switching.
  Wasmi stays the third backend as it is.
- wazero (Go): no GC, `memory64`, `multi_memory`, relaxed SIMD, or stack
  switching, and no Rust path ([`experimental/features.go`][wazero-features]).
- Chicory (Java): no `memory64`, relaxed SIMD, or stack switching, and no commit
  since 2026-05-27 ([README lines 89-104][chicory-readme]).
- toywasm (C): no GC, typed references, `memory64`, or stack switching. Its host
  functions can restart a call ([`exec_context.h` lines
  40-71][toywasm-restart]), which is prior art for host suspension.
- GraalWasm (Java): no stack switching. It embeds only through the GraalVM
  polyglot API ([`WasmOptions.java` lines 115-156][graal-options]).
- WABT's `wasm-interp`: no GC interpreter and no stack switching ([README lines
  47-67][wabt-readme]).
- The spec reference interpreter and the stack-switching reference interpreter
  (OCaml): no embedding API and no threads. They are oracles, not backends
  ([stack-switching `ast.ml` lines 175-181][ss-ast]).
- Wasmtime's Pulley interpreter: it is Wasmtime, the control, behind the same
  API. Wasmtime serves stack switching only on Cranelift ([`config.rs` lines
  2455-2470][wasmtime-ss-platform]).
- zwasm, Endive, and Chasm: listed on webassembly.org without stack switching
  ([`features.json` lines 782-816, 549-574, 1067-1097][wf]). Not read in source.

[wasmi-gc]: https://github.com/wasmi-labs/wasmi/issues/775
[wasmi-fr]: https://github.com/wasmi-labs/wasmi/issues/774
[wasmi-threads]: https://github.com/wasmi-labs/wasmi/issues/777
[wasmi-eh]: https://github.com/wasmi-labs/wasmi/issues/1137
[wazero-features]:
  https://github.com/tetratelabs/wazero/blob/e234f6fe6ecd4589dd0c643b641bd38f6d826ee5/experimental/features.go#L5-L39
[chicory-readme]:
  https://github.com/dylibso/chicory/blob/e2e2e4058f49fbffeffc5ea92c54b41534cb45d3/README.md#L89-L104
[toywasm-restart]:
  https://github.com/yamt/toywasm/blob/9f0a8ad1cacef54bf252d10aef4a138bf7f15d15/lib/exec_context.h#L40-L71
[graal-options]:
  https://github.com/oracle/graal/blob/c5aafc62d2296866f1a9705918d617a880acbb40/wasm/src/org.graalvm.wasm/src/org/graalvm/wasm/WasmOptions.java#L115-L156
[wabt-readme]:
  https://github.com/WebAssembly/wabt/blob/b835f1ce49e5803c597a0548099456d6be1d3d11/README.md#L47-L67
[ss-ast]:
  https://github.com/WebAssembly/stack-switching/blob/c7be390539e5fc10669c88cadc4fa4b32fba1d47/interpreter/syntax/ast.ml#L175-L181
[wf]:
  https://github.com/WebAssembly/website/blob/08147a438d024f02bfdea56c20b1af2cf91c6f42/features.json

## Capability Comparison

Key: **D** is on by default. **F** is behind a flag or a build option. **P** is
partial. **N** is not implemented. Each cell is cited in the engine's section.
JSC is WebKitGTK 2.54. Wasmer is the Cranelift tier.

| Capability            | V8  | JSC | SpiderMonkey | WasmEdge | Wasmer | WAMR | Wizard | Wasm3 |
| --------------------- | --- | --- | ------------ | -------- | ------ | ---- | ------ | ----- |
| `multi_memory`        | D   | D   | D            | D        | D      | F, P | D      | D     |
| `memory64`            | D   | F   | D            | D        | N      | F, P | D      | D     |
| `tail_call`           | D   | D   | D            | D        | N      | F    | D      | D     |
| `exceptions`          | D   | D   | D            | D        | D      | N    | D      | D     |
| `function_references` | D   | D   | D            | D        | N      | F, P | D      | P     |
| `gc`                  | D   | D   | D            | D        | N      | F, P | D      | N     |
| `relaxed_simd`        | D   | D   | D            | D        | D      | N    | D      | N     |
| `threads`             | D   | D   | F            | F        | D      | F    | F, P   | P     |
| `stack_switching`     | F   | N   | F, P         | N        | N      | N    | F      | D     |
| `extended_const`      | D   | D   | D            | D        | D      | F    | D      | D     |
| `wide_arithmetic`     | F   | F   | F            | N        | D      | N    | F      | D     |
| Wasm 3.0 by default   | Yes | No  | Yes          | Yes      | No     | No   | Yes    | No    |
| `host_suspension`     | Yes | Yes | Yes          | P        | P      | No   | No     | P     |
| Rust or C embedding   | JS  | JS  | JS           | C        | Rust   | C    | None   | C     |

"JS" means a JS-only boundary. WasmEdge's `gc` has no collector. Wasm3's
`stack_switching` is on unreleased `main` only.

The remaining points, side by side:

| Point                     | V8                | JSC                | SpiderMonkey      | WasmEdge         | Wasmer        | WAMR          |
| ------------------------- | ----------------- | ------------------ | ----------------- | ---------------- | ------------- | ------------- |
| `exnref` reaches the host | No, JS object     | No, JS object      | No, JS object     | No, error only   | Yes           | No, string    |
| GC refs cross             | Opaque            | Opaque             | Opaque            | Opaque bits      | No            | Yes, own API  |
| Import and export types   | Parse with crate  | Parse with crate   | Parse with crate  | No GC types      | No GC types   | Separate API  |
| Memory storage            | Mapped, 8 GiB     | Mapped, 4 GiB      | Mapped, 4 GiB     | Mapped, 12 GiB   | Mapped, 8 GiB | Mapped, 8 GiB |
| Trap form                 | Message string    | Message string     | Message string    | Code and message | Enum          | String        |
| Published spec pass rate  | None              | None (188,151/138) | None              | None             | None          | None          |
| Licence                   | MIT and BSD-3     | LGPL-2 and BSD-2   | MPL-2.0           | Apache-2.0       | MIT           | Apache-2.0    |
| In nixpkgs at `b6c8664`   | Source build only | Yes, 2.54.0        | No usable package | 0.17.1           | 7.4.2         | 2.4.4         |
| Native build              | Prebuilt, 38 MiB  | None, in flake     | Prebuilt, 18 MB   | Library needed   | Pure Rust     | CMake         |
| Releases in 2026          | 30                | 9 stable           | 26 (`mozjs_sys`)  | 9                | 10            | 1             |

The JSC pass count is computed from committed test expectations, not published
by WebKit.

## Full-Fidelity Verdict, Ranked

The candidates are ordered by fidelity first, then by cost. So V8 comes first
although its code cost is the highest, and WasmEdge comes after SpiderMonkey for
its fidelity (a GC that never collects, and partial host suspension), not for
its cost.

Only one candidate reaches full fidelity:

1. **V8, through the `v8` crate, with `--wasm-wasmfx`.** All nine features and
   host suspension through JSPI. The build cost is low: one fixed-output fetch
   of a 38 MiB archive, and no C++ build. The code cost is high: a port of the
   browser backend from `wasm-bindgen` to `v8` handle scopes, plus a microtask
   pump, plus one isolate per thread for `threads`. The trap table, the
   `wasmparser` type registry, and the JSPI design carry over. The risk is the
   flag. WasmFX is experimental in V8 and compiles only in Turboshaft.

No other candidate reaches full fidelity. The closest, by fidelity and then by
cost:

2. **JavaScriptCore, WebKitGTK 2.54.** Eight features, with
   `JSC_useWasmMemory64=1`, plus host suspension. It misses `stack_switching`,
   and its memory64 is `testable` and runs in the interpreter tier only. The
   build cost is zero, because the flake already holds `webkitgtk_6_0` 2.54.0.
   The code cost equals V8's, plus C API declarations the 6.0 headers omit, and
   memory access by copy.
3. **SpiderMonkey, through `mozjs`.** Eight features plus host suspension. It
   misses `stack_switching`, which is partial and off. The build cost is a
   prebuilt fetch or a long C++ build. The code cost is V8's plus manual rooting
   and a preference shim.
4. **WasmEdge.** Eight features, with `threads` behind a flag, as SpiderMonkey's
   is. It misses `stack_switching`, and its GC never collects. Host suspension
   is partial: one OS thread per waiting call, since its fiber path was judged
   unsafe for out-of-order resumes. Its C API is closer to the trait than a
   JS-only boundary, but the nixpkgs package (0.17.1) trails the surveyed
   release.
5. **Wizard.** Eight features, partial `threads`, no host suspension, and no
   embedding path. Not a backend without new work in Virgil.

Wasmer, WAMR, and Wasm3 each miss two or more features and host suspension in
full. Their native C or Rust APIs are closer to the trait, but their engines
lack the features.

Two routes reach part of the goal without a new backend. This note did not test
either.

- The browser lane can reach full fidelity on V8 if Chromium runs with
  `--js-flags=--wasm-wasmfx`. Chromium takes V8 flags that way, as the
  webassembly.org note for wide arithmetic shows ([`features.json` line
  320][wf-chrome-wide]). That lane stays a browser lane.
- The `tests smoke webkit` lane already drives the browser backend in WebKitGTK
  2.54 (`smokeWebkit` in [`flake.nix`][wcmp-smoke-webkit]). The same driver can
  run the web test suite there. That gives Safari's semantics with the existing
  backend. It is a browser lane too, and it needs a headless Weston display.

## What Each Candidate Adds over the Browser and Wasmtime Lanes

- V8 adds a native lane with strict full fidelity. It tests stack switching on a
  second engine beside Wasmtime, and it lets a person debug the JSPI path
  natively. Its semantics are Chrome's. Safari parity: none.
- JSC adds a native lane with Safari 27's Wasm semantics and its JSPI design,
  which copies frames to the heap. It runs in the native test harness, with no
  WebDriver. Safari parity: yes, from the same source as Safari 27.
- SpiderMonkey adds a native lane with Firefox 153's semantics. Safari parity:
  none.
- WasmEdge adds a third independent native engine family, with GC on by default.
  Its GC never collects, so memory tests behave differently. Safari parity:
  none.
- Wasmer adds an engine family on Cranelift, as Wasmtime is. It covers less of
  the lexicon than Wasmtime. Safari parity: none.
- WAMR adds an engine family for small devices. It covers less of the lexicon
  than the Wasmi lane. Safari parity: none.
- Wizard adds an independent stack-switching implementation, useful as an oracle
  in tests. Safari parity: none.
- Wasm3 adds a second small C engine with stack switching, useful as an oracle
  once 0.9.2 is released. Safari parity: none.

The owner weighs Safari above Firefox. On that weight, JSC adds the most that
the current lanes lack, even without `stack_switching`. V8 adds the strict full
fidelity the card asks for, but on the engine the browser lane already tests.
Both share one design: a native port of the browser backend over a JavaScript
engine. A backend for one of them makes the second cheaper.

## Claims Not Verified

- Build size and build time for every engine. Nothing was built.
- Whether V8's WasmFX instructions always fall back from Liftoff to Turboshaft
  in practice.
- The testsuite revision behind V8's test archive and behind WebKit's
  `wasm/core` import.
- The closure size of `webkitgtk_6_0` 2.54.0 at `b6c8664`, and whether it is a
  binary cache hit. The figure of 1.2 GiB is for 2.52.4 at `3e41b24`.
- Whether pinning the buffer of a Wasm memory in JSC blocks `memory.grow`.
- Whether `mozjs_sys` bindgen exposes the `JS::Prefs` setters.
- Whether WasmEdge's fiber path breaks on out-of-order resume. This was read
  from the code.
- Whether Wasmer's `v8` backend accepts `exnref` modules.
- Whether WAMR's multi-memory works in AOT.
- Whether Wasm3 passes the stack-switching proposal's own tests, and when 0.9.2
  ships.
- Whether Chromium accepts `--js-flags=--wasm-wasmfx` in the flake's browser
  lane, and whether the web test suite runs under WebKitWebDriver.
- No engine except JSC, through computed counts, has a spec pass rate to
  compare. None publishes one.

[wcmp-capability]: ../../rust/wcmp-wasm-core/src/capability/capability.rs
[wcmp-lock]: ../../flake.lock
[wcmp-lib]: ../../rust/wcmp-wasm-core/src/lib.rs
[wcmp-testsuite]: ../../flake.nix
[wcmp-pdd025-hs]: ../design/PDD025%20Runtime%20Layer.md#host-suspension
[wcmp-wasmi-backend]: ../../rust/wcmp-wasm-core-wasmi/src/backend.rs#L33-L54
[wcmp-wasmtime-backend]:
  ../../rust/wcmp-wasm-core-wasmtime/src/backend.rs#L37-L73
[wcmp-probes]: ../../rust/wcmp-wasm-core-web/src/probes.rs
[wcmp-web-backend]: ../../rust/wcmp-wasm-core-web/src/backend.rs#L117-L126
[wcmp-traps]: ../../rust/wcmp-wasm-core-web/src/traps.rs#L52-L63
[wcmp-traps-sm]: ../../rust/wcmp-wasm-core-web/src/traps.rs#L149-L158
[wcmp-traps-jsc]: ../../rust/wcmp-wasm-core-web/src/traps.rs#L208
[wcmp-smoke-webkit]: ../../flake.nix
[wasmtime-ss-platform]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/config.rs#L2455-L2470
[wasmtime-call-async]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/func.rs#L1113
[wasmi-buffer]:
  https://github.com/wasmi-labs/wasmi/blob/v2.0.0/crates/core/src/memory/buffer.rs#L143
[wf-chrome-ss]:
  https://github.com/WebAssembly/website/blob/08147a438d024f02bfdea56c20b1af2cf91c6f42/features.json#L308
[wf-chrome-wide]:
  https://github.com/WebAssembly/website/blob/08147a438d024f02bfdea56c20b1af2cf91c6f42/features.json#L320
[wf-wasmer-ss]:
  https://github.com/WebAssembly/website/blob/08147a438d024f02bfdea56c20b1af2cf91c6f42/features.json#L634
