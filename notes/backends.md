# Backends

The polyfill runs core Wasm on one of three backends. The host makes one and
hands it to `Engine::with_backend`.

| Backend  | Crate                          | Runs                                                       | Engine                                  |
| -------- | ------------------------------ | ---------------------------------------------------------- | --------------------------------------- |
| Browser  | `rust/wcmp-wasm-core-web`      | In a browser, on `wasm32-unknown-unknown`                  | The browser's `WebAssembly` API         |
| Wasmi    | `rust/wcmp-wasm-core-wasmi`    | Natively. It also builds for `wasm32`, which no lane tests | Wasmi 2.0, an interpreter               |
| Wasmtime | `rust/wcmp-wasm-core-wasmtime` | Natively only                                              | Wasmtime 49, used as a core engine only |

The Wasmtime backend is the control. It runs beside the other two on every test
lane, so a person can compare their results with Wasmtime's.

Other notes compare the backends on [suspend providers](suspend-providers.md),
[memory](memory.md), [performance](performance.md), and
[known limits](known-limits.md).

## Capabilities

Every backend runs Wasm 2.0. A capability is a Wasm feature above Wasm 2.0, or
host suspension, that a backend declares only where its engine implements it
faithfully. The names come from one fixed list
(`rust/wcmp-wasm-core/src/capability/capability.rs`). A component that needs a
capability the backend does not declare fails at `Component::new` with
`Error::Unsupported` and the name of the capability.

| Capability            | Chromium       | WebKitGTK 2.54 | Wasmi | Wasmtime                         |
| --------------------- | -------------- | -------------- | ----- | -------------------------------- |
| `multi_memory`        | Yes            | Yes            | Yes   | Yes                              |
| `memory64`            | Yes            | No (off)       | Yes   | Yes                              |
| `tail_call`           | Yes            | Yes            | Yes   | Yes                              |
| `exceptions`          | Yes            | Yes            | No    | Yes                              |
| `function_references` | Yes            | Yes            | No    | Yes                              |
| `gc`                  | Yes            | Yes            | No    | Yes                              |
| `relaxed_simd`        | Yes            | Yes            | Yes   | Yes                              |
| `threads`             | Yes            | Yes            | No    | Yes                              |
| `stack_switching`     | No             | No             | No    | Where Wasmtime's compiler has it |
| `host_suspension`     | Yes, with JSPI | Yes, with JSPI | Yes   | No                               |

Sources: each backend's `src/backend.rs` and crate docs (`src/lib.rs`). The
Chromium and WebKitGTK columns are what the browser backend's probes find, as
the test `it_declares_what_the_engine_of_the_page_implements` in
`rust/wcmp-wasm-core-web/tests/web.rs` holds them, in the Chrome of the web
lanes and in the WebKitGTK 2.54.1 of the WebKitGTK lanes. WebKitGTK 2.54 ships
memory64 turned off. No browser ships `stack_switching`
(`rust/wcmp/src/concurrency/stack_switching_provider.rs`).

The browser backend decides each Wasm row at run time. When `Web::new` runs, it
gives one small module per feature to `WebAssembly.validate`, and it declares
each feature whose module the browser accepts
(`rust/wcmp-wasm-core-web/src/probes.rs`). It declares `host_suspension` where
the browser has both functions of JavaScript Promise Integration (JSPI),
`WebAssembly.Suspending` and `WebAssembly.promising`. A browser that lacks a
feature still loads the backend, which then declares less. Two browsers show
this today:

- WebKitGTK 2.54.0 ships JavaScriptCore with memory64 off and with multi-memory
  and JSPI on. The polyfill refuses a component with a 64-bit memory there with
  `Unsupported(memory64)` (card `629412`, 2026-10-05).
- In a browser without `multi_memory`, as older Safari releases were
  (`rust/wcmp/tests/baseline_content_security_policy.rs`), the backend copies
  bytes in bulk with JavaScript `TypedArray.set`, which is the slower path. The
  translator then refuses a fused adapter that copies between the memories of
  two components (card `b5a406`).

## Fidelity

| Suite                          | Browser                      | Wasmi                 | Wasmtime              |
| ------------------------------ | ---------------------------- | --------------------- | --------------------- |
| Conformance, directives passed | 2410 of 2446 (98.5%)         | 2355 of 2446 (96.3%)  | 2415 of 2446 (98.7%)  |
| Fidelity, script tests passed  | 264, with 83 listed failures | 264, with none listed | 267, with none listed |
| Zena scenarios passed          | 18 of 18                     | 2 of 18               | 18 of 18              |

The conformance row is the Component Model and Wasmtime `.wast` corpora under
the [conformance](conformance.md) harness, with the suspend provider on. The
figures are the Baseline tables of `rust/wcmp/tests/corpus/README.md`, from the
`tests all` run of 2026-10-13. On Wasmi, the 60 lines of
`expected-failures.wasmi.txt` are components that need a capability Wasmi lacks,
and the directives that fail after them.

The fidelity row is the WebAssembly specification test suite, run on one backend
through the runtime layer (`rust/wcmp-wasm-core-fidelity`). The counts are from
card `c91d3d` (2026-10-01). The Wasmtime file has three more tests, which test
the runner itself. A script that needs a capability the backend does not declare
passes without a run, so the same count does not mean the same coverage. Each
backend lists its expected failures, each with a citation, in
`tests/fidelity/expected-failures.txt` in its crate. The browser's 83 are V8
defects and limits that the web embedding requires (see
[known limits](known-limits.md)).

The Zena row runs programs that the Zena toolchain compiles, as components
(`rust/wcmp/tests/zena/README.md`). The counts are the passes per subject in
`rust/wcmp/tests/zena/record.txt`. The report of 2026-10-13 in the README gives
the same counts. The Wasmtime column is the polyfill over the Wasmtime backend
(`native` in the record). Wasmtime's own run (`wasmtime`) also passes 18 of 18.
On Wasmi, 16 scenarios stop at `Component::new` with `Unsupported(gc)`.
