# Conformance corpora

This directory holds vendored copies of two upstream `.wast` suites. The
harness in `tests/conformance.rs` runs every file on every supported
target. A change to a file here is a deliberate update of the vendored
commit, recorded below.

| Directory         | Source                                                                                  | Commit         | Vendored   |
| ----------------- | ---------------------------------------------------------------------------------------- | -------------- | ---------- |
| `cm/`             | `WebAssembly/component-model`, `test/` (synchronous subsets)                              | `e5ee0af9c617` | 2026-09-14 |
| `cm/async/`       | `WebAssembly/component-model`, `test/async/`                                              | `e5ee0af9c617` | 2026-09-16 |
| `wasmtime/`       | `bytecodealliance/wasmtime`, `tests/misc_testsuite/component-model/` (synchronous subset) | `cb091c33cece` | 2026-09-14 |
| `wasmtime/async/` | `bytecodealliance/wasmtime`, `tests/misc_testsuite/component-model/async/`                | `cb091c33cece` | 2026-09-16 |

The polyfill runs the callback form of an asynchronous export and the
task built-ins that come with it, so part of `cm/async/` and
`wasmtime/async/` passes: a host call into such an export, `task.return`,
backpressure, the waitable set built-ins, `thread.yield`, the context
slots, and a synchronous lower of such an export from a sibling
component. A component whose import is lowered asynchronously translates
and instantiates too; only a guest that makes such a call fails, because
the call path behind it is not built yet. The directive that first
meets what is missing is an expected failure of category
`deferred-feature`, for one of six reasons: an asynchronous lower, a
future or stream built-in, the stackful lift, a thread built-in other
than `thread.yield`, cancellation, or a call to an asynchronous host
item. One directive is a `validation` failure instead, the first
component of `wasmtime/async/cancel-host.wast`: it lowers
asynchronously without the `memory` option, which the reference
requires and Wasmtime does not enforce. Most of the rest is `cascade`:
a component definition that fails leaves its name unbound and no
instance current, so every later directive in the file that names the
definition or invokes the instance fails as bookkeeping rather than on
its own merits. The async files define a component once and then drive
it over dozens of directives, so those rows carry far more `cascade`
lines than `deferred-feature` ones.

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

The harness links every file against the host environment Wasmtime's
wast runner provides: the `host` instance, `host-return-two`, and
the rest of its component spectest (`crates/wast/src/spectest.rs`
upstream), plus the module exports of every named component a file
instantiates, reflected under the component's name as the runner
does. The runner defines five of those items as asynchronous. The
harness registers one of them, `host-echo-u32`, through the concurrent
entry the runner uses, because the link rule holds an `async func`
import to a concurrent registration; its future answers with the
argument and never pends. It registers none of the other four:
`host.never-return`,
`host.return-two-slowly`, `host.echo-slowly`, and
`host.[method]resource1.never-return`. A file that imports one of the
four fails as a `deferred-feature`.

The harness also registers the `wasmtime` instance the runner provides
beside the spectest for its own misc tests, with the one item a file
of the corpus imports: `gc`, whose function does nothing, because the
polyfill's substrate collects its own garbage.

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
| `maps`        | `maps/maps.wit` (world `maps`), `maps/maps.wat`                               | as `guest`                                                                  |
| `fixed-lists` | `fixed-lists/fixed-lists.wit` (world `fixed-lists`), `fixed-lists.wat`        | as `guest`                                                                  |

`build.sh` records the exact commands. The `.wasm` binaries are checked
in next to their sources.

`wast` has no syntax for a `map` value or a fixed-length list value.
A directive spells a map as a list of two-element tuples, the map's
canonical-ABI layout, and a fixed-length list as a list; the harness
turns them into the polyfill's values wherever the invoked function
declares a `map` or a `list<T, N>`, for arguments and expected results
alike.

## Baseline

The progress summary on the native target, as of 2026-09-20 (`tests
conformance` prints the current one):

| Corpus           | Directives | Passed | Pass % | Expected failures by category                                  |
| ---------------- | ---------- | ------ | ------ | -------------------------------------------------------------- |
| `cm`             | 1126       | 1034   | 91.8   | deferred-feature 2, substrate 4, validation 20, cascade 66     |
| `cm/async`       | 393        | 22     | 5.6    | deferred-feature 49, cascade 322                               |
| `fixtures`       | 17         | 17     | 100.0  | none                                                           |
| `wasmtime`       | 469        | 431    | 91.9   | deferred-feature 2, substrate 8, cascade 28                    |
| `wasmtime/async` | 387        | 88     | 22.7   | deferred-feature 93, validation 1, cascade 205                 |
| total            | 2392       | 1592   | 66.6   | deferred-feature 146, substrate 12, validation 21, cascade 621 |

The browser's summary differs by the ten lines of
`expected-failures.web.txt`, which move ten passing directives into
`substrate`: `cm/async` passes 21 (5.3%), `wasmtime` 425 (90.6%),
`wasmtime/async` 85 (22.0%), and the total is 1582 (66.1%) with
substrate 22. Every other cell is the same.

The `async` rows still hold the pass rate down. The polyfill runs a host
call into a callback export, the task built-ins that export uses, and a
synchronous lower of a call between two components, but the six reasons
above cover most of what those directories exercise. Each component
those directories define that the polyfill rejects is a
`deferred-feature` failure, and every later directive in the same file
that names it is a `cascade` one, which is why the two async rows
together hold 527 of the 621 cascade lines, while `cm` and `wasmtime`
alone pass at 91.8% and 91.9%.
