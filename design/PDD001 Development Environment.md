# Development Environment

The development environment of the polyfill is reproducible and spans the two
targets the library supports. A [Nix flake][Nix Flakes] encodes every tool.
A [katsuobushi] menu lists every workflow, from a native build to a test run
in a real browser, as a single shell command.

## Goals

- A developer enters a fully provisioned shell with one command
  (`nix develop`) and can build and test the polyfill at once.
- The flake encodes every tool dependency: the Rust toolchain,
  `wasm-bindgen-cli`, Chrome, ChromeDriver, and the auxiliary utilities. The
  environment is the same on every machine and in CI.
- The menu lists every development and test workflow. A contributor does not
  memorize command lines.
- The test suite runs the same test source on the native target and on
  `wasm32-unknown-unknown` in a real browser.
- Browser tests run without manual setup on Linux and on Darwin.

## Non-goals

- Packaging or publication of the polyfill for downstream consumers.
- CI pipeline configuration. The same Nix derivations that run locally must
  also run in CI, and that is the whole requirement.
- Editor or IDE setup. The shell provides every tool a language server needs.

## Workspace Layout

The polyfill is a Rust workspace with a top-level `Cargo.toml`. The root
manifest declares every shared dependency in `[workspace.dependencies]`.
Every crate in the workspace inherits its dependencies from that table with
`dependency = { workspace = true }`. A crate never pins its own version or
source for a dependency that the workspace table provides.

The polyfill is a Rust library crate (`rlib`). Its consumer is Rust code, so
the workspace produces no JavaScript artifacts. The consumer compiles the
polyfill into its own binary, natively or with `wasm-bindgen`.

The environment provides tooling for two compilation targets:

- The native target of the host machine, for fast iteration.
- `wasm32-unknown-unknown`, for browser-fidelity tests.

## User Stories

A developer checks out the polyfill for the first time and wants one command
that gives them a working shell.

> The developer runs `nix develop` at the repository root. The shell hook
> prints a menu of commands. They run `test:native:debug` and the native test
> suite passes.

A developer iterates on polyfill internals and wants feedback in seconds.

> The developer runs `test:native:debug`. Cargo compiles and runs the test
> suite for the host target with `cargo-nextest`. The cycle is fast enough to
> run on every save.

A developer works on Component Model semantics and wants to test in the
environment where the polyfill runs.

> The developer runs `test:web:debug`. The shell already has Chrome and
> ChromeDriver installed and the environment variables set.
> `wasm-bindgen-test-runner` starts the browser, runs the compiled test
> binary, and streams the results to the terminal.

A developer writes a new test and wants it to run on both targets without
duplication.

> The developer marks the test with the cross-target test attribute. The same
> source file compiles and runs under `test:native:*` and `test:web:*`.

## The Nix Shell and Menu

The flake at the repository root defines the development shell. The shell
uses [katsuobushi] to print a structured command menu on entry. Each menu
entry has a short name, a description, and a shell command.

The menu contains at least these commands:

| Command               | Description                                                    |
| --------------------- | -------------------------------------------------------------- |
| `build`               | Compile the polyfill crate for `wasm32-unknown-unknown`        |
| `test:native:debug`   | Unit and integration tests (host target, debug)                |
| `test:native:release` | Unit and integration tests (host target, release)              |
| `test:web:debug`      | Unit and integration tests (`wasm32-unknown-unknown`, debug)   |
| `test:web:release`    | Unit and integration tests (`wasm32-unknown-unknown`, release) |
| `test:all`            | The full suite across all configurations                       |
| `lint`                | Clippy and format checks across the workspace                  |
| `format:design`       | Format the Markdown files in `design/`                         |

Test packages are Nix derivations, so a test archive is built once, cached,
and replayed with `cargo nextest`. CI uses the same derivations.

## Multi-Platform Testing

A test that exercises shared logic carries the cross-target test attribute.
On the native target the attribute expands to an asynchronous test under
`tokio`. On `wasm32-unknown-unknown` it expands to `#[wasm_bindgen_test]`,
which makes the test visible to `wasm-bindgen-test-runner`. A test that is
specific to the browser carries `#[wasm_bindgen_test]` directly.

`wasm-bindgen-test-runner` reads these environment variables. The shell sets
them on every platform:

- `CHROME_PATH`: the path to the Chrome or Chromium binary from Nix.
- `CHROMEDRIVER`: the path to the ChromeDriver binary.
- `WASM_BINDGEN_TEST_TIMEOUT`: the per-test timeout. Component instantiation
  under headless Chrome is slow, so the shell sets 180 seconds.
- `WASM_BINDGEN_TEST_WEBDRIVER_JSON`: the path to a WebDriver capabilities
  file that the flake generates.

The capabilities file starts Chrome with `--headless=new`, `--no-sandbox`,
`--disable-gpu`, and `--disable-dev-shm-usage`. Headless Chrome needs these
flags under the sandbox of a Nix build on Linux and under the default GPU
configuration on Darwin. The flake generates the file on every platform.

## References

- [Dialog DB], the source of the flake patterns, the test archive helper, the
  menu system, and the multi-target test configuration.
- [katsuobushi], the flake template and menu helper library.
- [Nix Flakes], the reproducible environment mechanism.
- [wasm-bindgen], the Rust to JavaScript bindings generator.
- [wasm-bindgen-test], the browser test harness for Rust Wasm.
- [PDD000], the product overview.

[Nix Flakes]: https://nixos.wiki/wiki/flakes
[katsuobushi]: https://github.com/cdata/katsuobushi
[Dialog DB]: https://github.com/dialog-db/dialog-db
[wasm-bindgen]: https://rustwasm.github.io/docs/wasm-bindgen/
[wasm-bindgen-test]: https://rustwasm.github.io/docs/wasm-bindgen/wasm-bindgen-test/index.html
[PDD000]: ./PDD000%20Wasm%20Component%20Model%20Polyfill.md
