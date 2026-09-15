# Conformance corpora

This directory holds vendored copies of two upstream `.wast` suites. The
harness in `tests/conformance.rs` runs every file on every supported
target. A change to a file here is a deliberate update of the vendored
commit, recorded below.

| Directory   | Source                                                          | Commit       | Vendored   |
| ----------- | --------------------------------------------------------------- | ------------ | ---------- |
| `cm/`       | `WebAssembly/component-model`, `test/` (synchronous subsets)   | `e5ee0af9c617` | 2026-09-14 |
| `wasmtime/` | `bytecodealliance/wasmtime`, `tests/misc_testsuite/component-model/` (synchronous subset) | `cb091c33cece` | 2026-09-14 |

The `async` directories of both suites are not vendored. They join the
corpus when the concurrency features they exercise are in scope.

`expected-failures.txt` lists every directive the polyfill does not pass
yet, one per line, as `<path>:<line> <category> <reason>`. The harness
fails when a listed directive passes, an unlisted directive fails, or a
line has no category, so the list stays current. The categories are
listed at the top of the file. `cm/nyi.txt` is the upstream list of
files that fail in the reference implementation itself.

`expected-failures.web.txt` is the browser-only delta: the harness
applies it on top of the shared list on `wasm32-unknown-unknown`. It
holds only differences of the substrate (the browser's engine against
Wasmtime), so a polyfill gap is recorded once, in the shared list, and
counts on both targets.

The test `it_reports_conformance_progress` runs every file in one
process and prints a summary per corpus directory: directives, passes,
the pass percentage, and the expected failures per category. The
`tests conformance` menu command runs only that test on both targets.
The native run prints its own table and the browser's table projected
from `expected-failures.web.txt` (exact while `tests web debug`
passes, since the browser's test runner shows no output for a passing
test), and writes both as JSON to `$CARGO_TARGET_DIR/conformance/`.

## Fixtures

`fixtures/` holds components built with the component toolchain rather
than written as `.wast` by hand. The `fixtures` menu command runs
`fixtures/build.sh` with the flake's pinned `wasm-tools` and `wac` and
regenerates every output byte for byte, including the harness manifest.
Each fixture directory holds the sources; the `.wast` next to it is
generated and runs under the harness like the vendored corpora.

| Fixture       | Sources                                                                       | Build                                                                       |
| ------------- | ----------------------------------------------------------------------------- | --------------------------------------------------------------------------- |
| `guest`       | `guest/guest.wit` (world `guest`), `guest/guest.wat`                          | `wasm-tools component embed --world guest`, then `wasm-tools component new` |
| `composition` | `composition/math.wit` (worlds `plug` and `socket`), `plug.wat`, `socket.wat` | each world as above, then `wac plug --plug plug.wasm socket.wasm`           |

`build.sh` records the exact commands. The `.wasm` binaries are checked
in next to their sources.
