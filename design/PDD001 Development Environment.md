# Development Environment

Wasm Component Model Polyfill's development environment is designed to be fully
reproducible, immediately productive, and honest about the multi-target nature
of the stack. Because the polyfill is a Rust library compiled to Wasm and
exercised in both native and browser-based Wasm environments, the developer
experience must span those contexts without ceremony. A [Nix flake][Nix Flakes]
encodes the complete environment, and a [katsuobushi]-powered menu system
surfaces every workflow — from local builds to multi-platform testing — as a
single, discoverable shell interface.

## Goals

- A developer can enter a fully-provisioned shell with a single command
  (`nix develop`) and immediately build and test the polyfill without any
  additional setup steps.
- The Nix flake encodes all tool dependencies — Rust toolchain, wasm-bindgen,
  binaryen, Chrome, ChromeDriver, and any auxiliary utilities — so that the
  environment is reproducible across machines and CI.
- Development and test workflows are surfaced through a katsuobushi menu so that
  contributors do not need to memorize command-line incantations.
- The test suite exercises polyfill logic across all relevant targets: native
  (for fast iteration) and `wasm32-unknown-unknown` (for browser-fidelity),
  authoring tests once and running them everywhere via a cross-target test
  macro.
- Browser automation is available as a first-class workflow, enabling
  integration tests that load the polyfill in a real browser and validate
  Component Model behavior end-to-end.

## Non-goals

- This document does not define how the polyfill is packaged for publication or
  consumed by downstream projects. Packaging is out of scope here.
- This document does not specify CI/CD pipeline configuration beyond noting that
  the same Nix derivations used locally must also be usable in CI.
- This document does not prescribe a specific editor or IDE setup, though the
  Nix shell provides all tools needed by any language server.

## Architecture Overview

The polyfill is a Rust workspace defined by a top-level `Cargo.toml` at the
repository root. All shared dependencies are declared in a
`[workspace.dependencies]` section in that root manifest, and every crate in the
workspace must inherit its dependencies from the workspace (i.e.,
`dependency = { workspace = true }` in each crate's `Cargo.toml`). Crates must
never pin their own version or source for a dependency that is available in the
workspace table.

The primary artifact is a Rust library crate compiled to Wasm and post-processed
with [wasm-bindgen] to produce a JavaScript-loadable module. Consumers of the
polyfill — typically web applications — load the resulting Wasm module to gain
Component Model capabilities (wasip3) on top of the browser's existing Wasm Core
support.

The Nix environment must therefore provide tooling for at least two compilation
targets:

- `<native target>` — for fast unit testing and any native tooling
- `wasm32-unknown-unknown` — for browser-fidelity testing and the published
  library artifacts

## User Stories

**As a developer checking out the polyfill for the first time**, I want to run a
single command and land in a shell where every build and test workflow is
already available, so that I do not spend time debugging my local environment.

> A developer runs `nix develop` at the repository root. The shell hook prints a
> formatted menu of available commands. They run `build` and the library
> artifacts are produced under `dist/`.

**As a developer iterating on polyfill internals**, I want to run fast native
tests against my changes, so that I get feedback in seconds rather than waiting
for a Wasm compilation cycle.

> The developer runs `test:native` from the menu. Cargo compiles and runs the
> test suite for the host target using `cargo-nextest`. The cycle is fast enough
> that it can be run on every save.

**As a developer working on Component Model semantics**, I want to run tests
compiled to Wasm against a real browser, so that I can verify behavior in the
environment where the polyfill actually runs.

> The developer runs `test:web` from the menu. The Nix shell already has Chrome
> and ChromeDriver installed and the relevant environment variables
> (`CHROME_PATH`, `CHROMEDRIVER`) pre-set. `wasm-bindgen-test-runner` launches
> the browser, executes the compiled test binary, and streams results back to
> the terminal.

**As a developer writing a new test for a polyfill feature**, I want to author
it once and have it run on both native and Wasm targets without manual
duplication, so that coverage is always complete.

> The developer annotates their test with a cross-target `#[test]` macro that
> conditionally expands to `#[tokio::test]` for native targets and
> `#[wasm_bindgen_test]` for Wasm targets. The same source file is compiled and
> exercised by both `test:native` and `test:web`. (See Dialog DB's
> `dialog-common` crate for an upstream reference example of this pattern.)

**As a developer writing an integration test that loads the polyfill in a
browser**, I want to drive a browser programmatically from Rust, so that I can
assert on end-to-end behavior without leaving the language or the toolchain.

> The developer writes a test that uses the `fantoccini` or equivalent WebDriver
> client. The test is gated on a `web-integration-tests` Cargo feature so it is
> excluded from routine unit test runs and included when the developer
> explicitly invokes `test:browser:integration` from the menu.

## The Nix Shell and Menu System

The development shell is encoded in the repository's `flake.nix`. It consumes
[katsuobushi] to produce a structured, colorized command menu that is printed on
shell entry and re-printed on demand. Each menu entry has a short name, a
human-readable description, and a shell command body.

The menu surface for the polyfill's development environment includes at minimum:

| Command               | Description                                                    |
| --------------------- | -------------------------------------------------------------- |
| `build`               | Produce the Wasm library artifacts and JS bindings (debug)     |
| `build:release`       | Produce a release-optimised library bundle                     |
| `test:native:debug`   | Unit and integration tests (host target, debug)                |
| `test:native:release` | Unit and integration tests (host target, release)              |
| `test:web:debug`      | Unit and integration tests (`wasm32-unknown-unknown`, debug)   |
| `test:web:release`    | Unit and integration tests (`wasm32-unknown-unknown`, release) |
| `test:all`            | Full suite across all configurations                           |
| `lint`                | Clippy and format checks across the workspace                  |

The shell hook is produced by katsuobushi's `makeDevShellHook` and the commands
are registered via `makeMenu`. The flake exposes `buildTestArchive` and
`menuTestCommand` helpers (following the pattern established in Dialog DB's
`flake.nix`) so that test packages are first-class Nix derivations and can be
cached and re-used in CI.

## The Library Build

The polyfill crate is compiled to `wasm32-unknown-unknown` and post-processed
with [wasm-bindgen] to generate JavaScript glue. The Nix shell provides `cargo`,
`wasm-bindgen-cli`, and `binaryen` (for `wasm-opt`) as build inputs. The `build`
menu command drives the full pipeline:

1. `cargo build --target wasm32-unknown-unknown` (release for `build:release`)
2. `wasm-bindgen` to emit the JS module and `.wasm` artifact
3. `wasm-opt` for size and performance optimisation (release builds only)

Build output goes to a `dist/` directory at the repository root. Consumers of
the polyfill load the produced module directly; how the library is packaged for
publication is out of scope for this document.

## Multi-Platform Testing

Tests that exercise logic shared between native and browser contexts are
annotated with a cross-target `#[test]` macro that conditionally expands based
on the compilation target:

- On native targets it expands to `#[tokio::test]` (or a synchronous
  equivalent), enabling async test bodies with no additional annotation.
- On `wasm32-unknown-unknown` it expands to `#[wasm_bindgen_test]`, making the
  test discoverable by `wasm-bindgen-test-runner`.

Dialog DB's `dialog-common` crate provides a working implementation of this
macro that the polyfill may consume directly or use as a reference. Tests that
are inherently browser-specific (e.g. JS interop assertions, DOM-adjacent
globals) are annotated directly with `#[wasm_bindgen_test]` and gated on the
appropriate Cargo feature.

`wasm-bindgen-test-runner` is configured by the following environment variables,
which the Nix shell sets unconditionally:

- `CHROME_PATH` — path to the Chrome or Chromium binary provided by Nix
- `CHROMEDRIVER` — path to the ChromeDriver binary
- `WASM_BINDGEN_TEST_TIMEOUT` — extended to accommodate slower Wasm
  initialisation (180s is a reasonable default, consistent with Dialog DB's
  configuration)

On Darwin, a `webdriver.json` configuration file is generated by the Nix flake
and pointed to by `WASM_BINDGEN_TEST_WEBDRIVER_JSON`, disabling the GPU and
sandboxing flags that interfere with headless Chrome in that environment.

## References

- [Dialog DB] — upstream Nix flake patterns, `buildTestArchive`, menu system,
  and multi-target test configuration
- [Dialog DB `#[test]` macro] — cross-platform test annotation
- [katsuobushi] — Nix flake template and menu helper library
- [Nix Flakes] — reproducible development environment
- [wasm-bindgen] — Rust ↔ JavaScript bindings generator
- [wasm-bindgen-test] — browser-targeting test harness for Rust Wasm
- [PDD000] — the Wasm Component Model Polyfill product overview

[Nix Flakes]: https://nixos.wiki/wiki/flakes
[katsuobushi]: https://github.com/cdata/katsuobushi
[Dialog DB]: https://github.com/dialog-db/dialog-db
[Dialog DB `#[test]` macro]: <https://github.com/dialog-db/dialog-db/blob/00c7bc5fa8ea187da7abda27c2a0a8edbd8c05ed/rust/dialog-common/src/lib.rs#L132>
[wasm-bindgen]: https://rustwasm.github.io/docs/wasm-bindgen/
[wasm-bindgen-test]: https://rustwasm.github.io/docs/wasm-bindgen/wasm-bindgen-test/index.html
[PDD000]: ./PDD000%20Wasm%20Component%20Model%20Polyfill.md
