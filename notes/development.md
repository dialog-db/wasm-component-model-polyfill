# Development

The repository is a Nix flake. Enter the shell with `nix develop`. It prints a
menu, and `menu` prints it again. Every build and test goes through a menu
command, never through bare `cargo`. Outside the shell, prefix a command with
`nix develop -c`.

Nix sees only tracked files. Snapshot or commit before a `tests` or `lint`
command, or the run measures a stale tree.

| Command                      | What it does                                                                                   |
| ---------------------------- | ---------------------------------------------------------------------------------------------- |
| `build debug` / `release`    | Build the polyfill crate for both targets.                                                     |
| `tests native debug`         | Unit and integration tests on the host.                                                        |
| `tests web debug`            | The same tests in headless Chrome.                                                             |
| `tests webkit debug`         | The same tests in WebKitGTK's MiniBrowser on a headless Weston (Linux).                        |
| `tests conformance`          | The conformance progress summary for both targets.                                             |
| `tests fidelity <backend>`   | The WebAssembly spec tests on a backend: `wasmi`, `wasmtime`, `web`, or `webkit` (WebKitGTK).  |
| `tests smoke native` / `web` | The end-to-end smoke test as a binary or as a served page.                                     |
| `tests all`                  | Every test archive, debug and release, native and web, and on Linux the WebKitGTK lanes.       |
| `tests native wasmi`         | The polyfill's tests on the Wasmi backend, from the native debug build.                        |
| `tests zena` / `regenerate`  | The Zena scenarios on every subject, held to `rust/wcmp/tests/zena/record.txt`, or rewrite it. |
| `bench native` / `web`       | The benchmark suite on one target.                                                             |
| `bench wasmi`                | The benchmark suite over the Wasmi backend, with the binary `bench native` builds.             |
| `lint`                       | Every check the flake declares (`nix flake check`).                                            |
| `api list` / `update`        | Print or record the crate's public API snapshot.                                               |
