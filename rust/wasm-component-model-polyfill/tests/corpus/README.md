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

The test `it_reports_conformance_progress` runs every file in one
process and prints a summary per corpus directory: directives, passes,
the pass percentage, and the expected failures per category. The
`tests conformance` menu command runs only that test and writes the
summary as JSON to `$CARGO_TARGET_DIR/conformance/summary.json`.
