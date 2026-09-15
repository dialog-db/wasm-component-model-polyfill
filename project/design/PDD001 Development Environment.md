# Development Environment

The development environment of the polyfill is reproducible and spans the two
targets the library supports. A [Nix flake][Nix Flakes] encodes every tool. The
flake takes its Rust build helpers, its Markdown tooling, and its project board
from [katsuobushi]. A katsuobushi menu lists every workflow, from a native build
to a test run in a real browser, as a single shell command. Every build and test
routine behind the menu is a Nix derivation.

## Goals

- A developer enters a fully provisioned shell with one command (`nix develop`)
  and can build and test the polyfill at once.
- The flake encodes every tool dependency: the Rust toolchain,
  `wasm-bindgen-cli`, Chrome, ChromeDriver, Prettier, and the auxiliary
  utilities. The environment is the same on every machine and in CI.
- Every build and test routine is a Nix derivation. The menu builds the
  derivation and replays its output. No menu command runs a build or a test
  outside Nix.
- The shared infrastructure comes from a pinned katsuobushi release. The
  repository carries no local copy of a katsuobushi library.
- The menu lists every development and test workflow. A contributor does not
  memorize command lines.
- The test suite runs the same test source on the native target and on
  `wasm32-unknown-unknown` in a real browser.
- Browser tests run without manual setup on Linux and on Darwin.
- The design corpus and the project board live in the repository and have the
  same tooling as the code: a formatter, a lint, and a flake check.

## Non-goals

- Packaging or publication of the polyfill for downstream consumers.
- CI pipeline configuration. The same Nix derivations that run locally must also
  run in CI, and that is the whole requirement.
- Editor or IDE setup. The shell provides every tool a language server needs.
- Agent sandboxes. katsuobushi provides a sandbox library, and the polyfill does
  not use it.

## Katsuobushi

katsuobushi is a Nix library and a command-line tool for the development
environment of a project. The flake pins katsuobushi to a release tag, and the
release is `v0.5.1`. The flake tells katsuobushi to follow the `nixpkgs` input
of the polyfill, so the dependency graph has one nixpkgs. katsuobushi carries
the Rust build infrastructure (crane, nix-filter, and rust-overlay) as its own
inputs, so the flake of the polyfill does not name them.

The flake consumes three katsuobushi libraries:

- `lib.rust` provides the Rust toolchain from `rust-toolchain.toml`, a crate
  builder, a test archive builder, the cargo checks (clippy, rustfmt, and
  workspace dependency hygiene), and a shell hook that keeps the cargo target
  directory out of the repository. It builds `wasm-bindgen-cli` at the version
  that `Cargo.lock` resolves.
- `lib.markdown` provides one Prettier configuration that drives both a
  formatter command and a flake check for the Markdown documents.
- `lib.project` provides the project board command and a flake check that keeps
  the board and its card notes consistent.

The menu helpers (`makeMenu` and `makeDevShellHook`) come from the katsuobushi
overlay.

## Workspace Layout

The polyfill is a Rust workspace with a top-level `Cargo.toml`. The root
manifest declares every shared dependency in `[workspace.dependencies]`. Every
crate in the workspace inherits its dependencies from that table with
`dependency = { workspace = true }`. A crate never pins its own version or
source for a dependency that the workspace table provides. The crates live under
`rust/`.

The polyfill is a Rust library crate (`rlib`). Its consumer is Rust code, so the
workspace produces no JavaScript artifacts. The consumer compiles the polyfill
into its own binary, natively or with `wasm-bindgen`.

The environment provides tooling for two compilation targets:

- The native target of the host machine, for fast iteration.
- `wasm32-unknown-unknown`, for browser-fidelity tests.

The design corpus lives under `project/design/`. Each PDD follows the
katsuobushi PDD template: Introduction, Goals, Non-goals, Body, Test Cases, and
References. The corpus README states the template and the writing rules.

The project board lives under `project/kanban/`. The board is an Obsidian Kanban
file, and the card notes live next to it. The `project` menu command drives the
board.

## User Stories

A developer checks out the polyfill for the first time and wants one command
that gives them a working shell.

> The developer runs `nix develop` at the repository root. The shell hook prints
> a menu of commands. They run `tests native debug` and the native test suite
> passes.

A developer iterates on polyfill internals and wants feedback in seconds.

> The developer runs `tests native debug`. Nix builds the test archive for the
> host target, or returns it from the cache, and `cargo-nextest` replays the
> tests against the working tree. The cycle is fast enough to run on every save.

A developer works on Component Model semantics and wants to test in the
environment where the polyfill runs.

> The developer runs `tests web debug`. The shell already has Chrome and
> ChromeDriver installed and the environment variables set.
> `wasm-bindgen-test-runner` starts the browser, runs the compiled test binary,
> and streams the results to the terminal.

A developer writes a new test and wants it to run on both targets without
duplication.

> The developer marks the test with the cross-target test attribute. The same
> source file compiles and runs under `tests native` and `tests web`.

A developer edits a design document and wants it to match the corpus.

> The developer runs `markdown format`. Prettier reflows the document to the
> shared configuration. `markdown lint` and the flake check report the same
> result, so a document that passes locally passes in CI.

A developer wants the next piece of work.

> The developer runs `project status --available`. The board lists the cards
> that no open card blocks. They pick one and move it with `project status set`.

## The Nix Shell and Menu

The flake at the repository root defines the development shell. The shell uses
katsuobushi to print a structured command menu on entry. Each menu entry has a
short name, a description, and a shell command. A command with several variants
is a branch with subcommands, so one menu row covers one workflow.

The menu contains at least these commands:

| Command                | Description                                                           |
| ---------------------- | --------------------------------------------------------------------- |
| `build debug`          | Build the polyfill crate for both targets, unoptimized                |
| `build release`        | Build the polyfill crate for both targets, optimized                  |
| `tests native debug`   | Unit and integration tests (host target, debug)                       |
| `tests native release` | Unit and integration tests (host target, release)                     |
| `tests web debug`      | Unit and integration tests (`wasm32-unknown-unknown`, debug)          |
| `tests web release`    | Unit and integration tests (`wasm32-unknown-unknown`, release)        |
| `tests all`            | The full suite across all configurations                              |
| `lint`                 | Every check the flake declares (`nix flake check`)                    |
| `markdown format`      | Format the Markdown documents with Prettier                           |
| `markdown lint`        | Check that the Markdown documents are formatted                       |
| `project`              | Manage the project board (`project status`, `project new`, and so on) |
| `fixtures`             | Rebuild the conformance fixtures with `wasm-tools` and `wac`          |
| `smoke native`         | Run the end-to-end smoke test host as a native binary                 |
| `smoke web`            | Serve the same smoke test compiled for the browser                    |

Each build and test command runs a Nix derivation. `build` builds the crate
derivation and prints its store path. A `tests` command builds a test archive
derivation, so a test archive is built once, cached, and replayed with
`cargo nextest` against the working tree. `lint` runs the flake checks. CI uses
the same derivations.

The flake checks cover clippy with warnings denied, rustfmt, workspace
dependency hygiene, the doctests, the Markdown format, and the board lint.

## Multi-Platform Testing

A test that exercises shared logic carries the cross-target test attribute. On
the native target the attribute expands to an asynchronous test under `tokio`.
On `wasm32-unknown-unknown` it expands to `#[wasm_bindgen_test]`, which makes
the test visible to `wasm-bindgen-test-runner`. A test that is specific to the
browser carries `#[wasm_bindgen_test]` directly.

`wasm-bindgen-test-runner` reads these environment variables. The shell sets
them on every platform:

- `CHROME_PATH`: the path to the Chrome or Chromium binary from Nix.
- `CHROMEDRIVER`: the path to the ChromeDriver binary.
- `WASM_BINDGEN_TEST_TIMEOUT`: the per-test timeout. Component instantiation
  under headless Chrome is slow, so the shell sets 180 seconds.
- `WASM_BINDGEN_TEST_WEBDRIVER_JSON`: the path to a WebDriver capabilities file
  that the flake generates.

The capabilities file starts Chrome with `--headless=new`, `--no-sandbox`,
`--disable-gpu`, and `--disable-dev-shm-usage`. Headless Chrome needs these
flags under the sandbox of a Nix build on Linux and under the default GPU
configuration on Darwin. The flake generates the file on every platform.

## Test Cases

A fresh clone gives a working shell. A developer clones the repository and runs
`nix develop`. The shell prints the menu with every command in the table above.
No manual step follows.

The native suite runs from the menu. `tests native debug` builds the native test
archive as a Nix derivation and replays it with `cargo nextest`. The suite
passes. `tests native release` does the same under the release profile.

The browser suite runs from the menu. `tests web debug` builds the
`wasm32-unknown-unknown` test archive as a Nix derivation and runs it in
headless Chrome through `wasm-bindgen-test-runner`. The suite passes on Linux
and on Darwin with no manual setup.

The full suite runs from one command. `tests all` runs the four archives and
reports each result.

A build is a derivation. `build debug` and `build release` each print a Nix
store path that holds the polyfill `rlib` for the native target and for
`wasm32-unknown-unknown`.

No menu command runs a build or a test outside Nix. The flake contains no menu
command that invokes `cargo build`, `cargo test`, or `cargo nextest run` except
the archive replay behind the `tests` commands.

The flake check covers the whole tree. `nix flake check` runs clippy with
warnings denied, rustfmt, workspace dependency hygiene, the doctests, the
Markdown format check, and the board lint. It passes on a clean tree.

The shared infrastructure is pinned. `flake.lock` pins katsuobushi at the
`v0.5.1` tag. The flake declares no separate crane, nix-filter, or rust-overlay
input, and the repository holds no local copy of the Rust build helpers.

The corpus and the board are tooled. `markdown lint` passes on a formatted
corpus and fails on a document that Prettier would change. `project status`
lists the board, and `project lint` passes on a consistent board.

## References

- [katsuobushi], the flake library and menu helper.
- [katsuobushi Rust helpers], the crate builder, test archives, and checks.
- [katsuobushi Markdown helpers], the shared Prettier configuration.
- [katsuobushi project backlog], the board command and its lint.
- [Dialog DB], the source of the test archive pattern and the multi-target test
  configuration.
- [Nix Flakes], the reproducible environment mechanism.
- [wasm-bindgen], the Rust to JavaScript bindings generator.
- [wasm-bindgen-test], the browser test harness for Rust Wasm.
- [PDD000], the product overview.

[Nix Flakes]: https://nixos.wiki/wiki/flakes
[katsuobushi]: https://github.com/cdata/katsuobushi
[katsuobushi Rust helpers]:
  https://github.com/cdata/katsuobushi/blob/v0.5.1/lib/rust/README.md
[katsuobushi Markdown helpers]:
  https://github.com/cdata/katsuobushi/blob/v0.5.1/lib/markdown/README.md
[katsuobushi project backlog]:
  https://github.com/cdata/katsuobushi/blob/v0.5.1/lib/project/README.md
[Dialog DB]: https://github.com/dialog-db/dialog-db
[wasm-bindgen]: https://rustwasm.github.io/docs/wasm-bindgen/
[wasm-bindgen-test]:
  https://rustwasm.github.io/docs/wasm-bindgen/wasm-bindgen-test/index.html
[PDD000]: ./PDD000%20Wasm%20Component%20Model%20Polyfill.md
