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
component, and an asynchronous lower of one from a sibling too: the
lower answers with the status word, the subtask enters the caller's
handle table when the call does not resolve at once, and the callee's
start and resolution reach the caller as subtask events. A guest that
lowers a host `async` function asynchronously reaches it too: the call
answers with the status word, a future that is still running becomes a
subtask the guest waits on, the result crosses back when the future
completes, and `subtask.drop` takes the subtask's entry away once the
guest has taken delivery of it. A guest that lowers one synchronously
reaches it as well: the call blocks the guest thread where it stands
until the future resolves, and a future that never resolves traps. The
directive that first meets what is missing is an expected failure of
category `deferred-feature`, for one of six reasons: a call whose
callee can be released only by a caller that is on the stack, which
needs a stack switch, a future or stream built-in, the stackful lift, a
thread built-in other than `thread.yield`, cancellation, or an error
context. Two directives are `defect` failures, in the two
`reentrance.wast` files: a cycle that should go idle and fail with the
deadlock message instead answers the waiting task, and a call through a
shim's table reads past the one entry that table holds, because the
shim takes its element index from the call's argument. One directive is
a `validation` failure instead, the first component of
`wasmtime/async/cancel-host.wast`: it lowers asynchronously without the
`memory` option, which the reference requires and Wasmtime does not
enforce. Most of the rest is `cascade`: a component definition that
fails leaves its name unbound and no instance current, so every later
directive in the file that names the definition or invokes the instance
fails as bookkeeping rather than on its own merits. The async files
define a component once and then drive it over dozens of directives, so
those rows carry far more `cascade` lines than `deferred-feature` ones.

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
does. The runner defines five of those items as asynchronous, and the
harness registers all five through a concurrent entry, because the link
rule holds an `async func` import to a concurrent registration. Each
behaves as the runner makes it behave: `host-echo-u32` answers with its
argument and never pends; `host.never-return` and
`host.[method]resource1.never-return` stay pending for ever; and
`host.echo-slowly` and `host.return-two-slowly` are pending once and
resolve at the next poll, which is what the runner's single yield on
Wasmtime's executor comes to. Four go through the typed entry, and
`host.[method]resource1.never-return` through the untyped one, because
the typed entries derive no signature for the `borrow` it takes — the
same reason the synchronous methods of that resource are registered
untyped.

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

| Corpus           | Directives | Passed | Pass % | Expected failures by category                                            |
| ---------------- | ---------- | ------ | ------ | ------------------------------------------------------------------------ |
| `cm`             | 1126       | 1038   | 92.2   | deferred-feature 2, substrate 4, validation 20, cascade 62               |
| `cm/async`       | 393        | 82     | 20.9   | deferred-feature 44, defect 1, cascade 266                               |
| `fixtures`       | 17         | 17     | 100.0  | none                                                                     |
| `wasmtime`       | 469        | 431    | 91.9   | deferred-feature 2, substrate 8, cascade 28                              |
| `wasmtime/async` | 387        | 124    | 32.0   | deferred-feature 81, validation 1, defect 1, cascade 180                 |
| total            | 2392       | 1692   | 70.7   | deferred-feature 129, substrate 12, validation 21, defect 2, cascade 536 |

The browser's summary differs by the nine lines of
`expected-failures.web.txt`, which move nine passing directives into
`substrate`: `cm/async` passes 81 (20.6%), `wasmtime` 425 (90.6%),
`wasmtime/async` 122 (31.5%), and the total is 1683 (70.4%) with
substrate 21. Every other cell is the same.

The `async` rows still hold the pass rate down, though the asynchronous
lower of a call between two components moved 97 directives into the
passing column. The polyfill runs a host call into a callback export,
the task built-ins that export uses, both lowers of a call between two
components, and either lower of a host `async` function, but the six
reasons above cover most of what those directories still exercise. Each
component those directories define that the polyfill rejects is a
`deferred-feature` failure, and every later directive in the same file
that names it is a `cascade` one, which is why the two async rows
together hold 446 of the 536 cascade lines, while `cm` and `wasmtime`
alone pass at 92.2% and 91.9%. `subtask.drop` moved three directives of
`wasmtime/async`: the whole of `drop-host.wast`, and the component of
`task-builtins.wast` that declares the built-in. The asynchronous start
moved whole files: `cm/async/cross-abi-calls.wast`,
`cm/async/deadlock.wast`, `cm/async/drop-subtask.wast`,
`cm/async/drop-waitable-set.wast`,
`wasmtime/async/backpressure-deadlock.wast`,
`wasmtime/async/context-in-compositions.wast`,
`wasmtime/async/exceptions.wast`, `wasmtime/async/fused.wast`,
`wasmtime/async/subtask-wait.wast`, and
`wasmtime/async/wait-forever2.wast`. Neither lower of a host function
moved a directive of the corpora, and every other file that would
exercise one is held up by a built-in the polyfill does not serve yet.
