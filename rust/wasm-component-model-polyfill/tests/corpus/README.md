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

The polyfill runs the callback form of an asynchronous export, the task
built-ins that come with it, and both lowers of a call out through an
import, so part of `cm/async/` and `wasmtime/async/` passes: a host
call into such an export, `task.return`, backpressure, the waitable set
built-ins, `thread.yield`, and the context slots. A call between two
components crosses in all four combinations of lift and lower, and the
readable end of a stream or a future crosses with it. A read and a
write on the two ends of a stream pair up as the reference's stream
state pairs them, with partial and zero-length copies, and a copy
that does not finish at once completes through an event on its end.
A future's read and write are the same copy of one value, and each
end of a future copies once. A cancel ends the copy in progress on
one end and reports the progress it made, as Wasmtime reports it.
Owned handles cross a stream as its payload. A payload of a number
type copies between two ends one instance holds, and any other payload
traps there with Wasmtime's refusal. The translator accepts
`task.cancel` and `subtask.cancel`, so a component that links either
instantiates, but a call to either fails as unsupported, because the
cancellation of a task is not built. The asynchronous lower answers
with the status word, the subtask enters the caller's handle table
when the call does not resolve at once, and the callee's start and
resolution reach the caller as subtask events; the synchronous lower
returns at the callee's `task.return` and leaves the callee's late
exit to the next turn. A guest that lowers a host `async` function
asynchronously reaches it too: the call answers with the
status word, a future that is still running becomes a subtask the guest
waits on, the result crosses back when the future completes, and
`subtask.drop` takes the subtask's entry away once the guest has taken
delivery of it. A guest that lowers one synchronously reaches it as
well: the call blocks the guest thread where it stands until the future
resolves. No call traps for reentrance; the instance's entry gate is
the only serialization, and a callee the gate holds starts when the
gate opens. A call that blocks where nothing can make progress fails
with the message the reference gives it: the cannot-block message when
a synchronous call is in progress, the stack-switch message when only a
real suspension could wait, and the deadlock message otherwise. A real
suspension could wait when a host task is pending, or when the blocked
callee runs above a caller that would go on under a stack switch: a
caller that lowered the call asynchronously, or synchronously once the
callee returned. An exception thrown in a callee reaches the host as the
trap the synchronous baseline gives it.

The directive that first meets what is missing is an expected failure
of category `deferred-feature`, for one of six reasons: a call whose
callee can be released only by a caller that is on the stack, which
needs a stack switch, the stackful lift, a thread built-in other than
`thread.yield`, the cancellation of a task or a subtask, an error
context, or the rules that decide which trap poisons an instance.
Three definitions in the same category fail at link instead, on a
host item the harness does not provide: the two WASI 0.3 handler
fixtures import `wasi:http/types`, and
`wasmtime/async/cancel-starting-subtask-does-not-leak.wast` imports
`set-max-table-capacity` from the `wasmtime` instance. Most of the rest
is `cascade`: a component definition that fails leaves its name
unbound and no instance current, so every later directive in the file
that names the definition or invokes the instance fails as
bookkeeping rather than on its own merits. Several files of
`cm/async` define a component once and then drive it over dozens of
directives, so that row carries far more `cascade` lines than
`deferred-feature` ones.

`expected-failures.txt` lists every directive the polyfill does not pass
yet, one per line, as `<path>:<line> <category> <reason>`. The harness
fails when a listed directive passes, an unlisted directive fails, or a
line has no category, so the list stays current. The categories are
listed at the top of the file. `cm/nyi.txt` is the upstream list of
files that fail in the reference implementation itself.

The `tests regenerate` menu command rewrites the list from a native
run, in place of a hand loop over the failures a run prints. It runs
the progress test with `WCMP_REGENERATE_EXPECTATIONS` naming a copy of
the list, which the harness rewrites, prints the difference, and
installs the copy. `tests regenerate --dry-run` stops at the
difference and leaves the list alone. The dry run is also how to read
what a listed directive fails with today, because the harness compares
an expectation by file and line only: a reason in the list is never
checked against a run, so nothing else prints it.

The run's failures decide every line. A directive that still fails
keeps its line's category and takes the run's reason. A directive that
passes now loses its line. A directive the list does not name arrives
with the category `triage`, which is deliberately not a category: the
harness rejects the list, and every gate keeps failing, until a person
reads the failure and writes the category that names its cause. The
entries are sorted by path and then by directive line, so a run
against an unchanged tree rewrites the list byte for byte.

Two kinds of hand-written text survive the rewrite. A trailing
parenthetical is a note a person appended, and the rewrite restores it
after the run's reason, unless the run's reason already ends with the
same group, which is how the runtime's own wording — `unsupported
component feature: thread built-ins (table extraction)` — is told
apart from a note. A reason that ends in the run's cause behind other
leading text is a sentence a person wrote over that cause, and it is
kept whole: the nineteen such lines say in one clause what the substrate
says in a nested error and a backtrace. A line refreshes when the
cause at the end of the run's reason changes, which is what a change
to the failure text means. A line the rule reads the other way is
visible in the difference the command prints, which is where a person
decides.

Two things about running it. Nix builds the test archive from the
tracked tree, so commit a code change before regenerating: the reasons
come from the archive's code, while the merge reads the list on disk.
And a line the command wrote with `triage` needs its category before
the command runs again, because the harness parses the list before it
runs the corpus and refuses the placeholder.

`expected-failures.web.txt` is the browser-only delta: the harness
applies it on top of the shared list on `wasm32-unknown-unknown`. It
holds only differences of the substrate (the browser's engine against
Wasmtime), so a polyfill gap is recorded once, in the shared list, and
counts on both targets. `tests regenerate` does not touch it. Its
reasons are the browser engine's wording, which a native run cannot
produce and must not invent; the delta holds only substrate
differences, ten lines today, and each one is written by hand from
the failure a `tests web debug` run prints.

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
beside the spectest for its own misc tests, with one item: `gc`, whose
function does nothing, because the polyfill's substrate collects its
own garbage. A file of the corpus also imports
`set-max-table-capacity` from it, which the harness does not register,
so that file stops at link.

An `assert_trap` passes when the trap's message contains the expected
text, as in Wasmtime's runner. In a file of `cm/` it also passes when
the expected text and the message both contain `cannot write`, or both
contain `cannot read`. Wasmtime's runner accepts those pairs in every
file (`crates/wast/src/wast.rs:551-554` at `cb091c33cece`), because the
Component Model's suite words the traps of a copy on a done end
differently from Wasmtime, and the spec fixes no wording for them. The
polyfill raises Wasmtime's wording, which the `wasmtime` corpus
expects as written, so the harness relaxes only `cm/`. The rule reaches
every expected text with either phrase, not only a copy on a done end:
the refusal of a read and a write from one instance in
`same-component-stream-future.wast` expects `cannot read from and write
to intra-component future`, which any `cannot read` trap would satisfy.
The polyfill's refusal contains that text as written, so those lines
pass without the relaxation.

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
than written as `.wast` by hand: four from a core module in WAT, and
five — `rich`, `streams`, `stream-composition`, `wasi-http`, and
`wasi-http-same-instance` — from Rust crates that `cargo` and
wit-bindgen build, so the binding layer is the one a real guest
carries. `wasi-http-same-instance` is the WASI 0.3
handler as first written. Its `drain` copies a non-number payload
between two ends one instance holds, which the spec traps under a
rule it marks as temporary. The fixture is kept so that lifting the
rule shows up in the tests. The `fixtures` menu command runs
`fixtures/build.sh` with the flake's pinned tools and regenerates every
output byte for byte, including the harness manifest. Each fixture
directory holds the sources; the `.wast` next to it is generated and
runs under the harness like the vendored corpora. `fixtures/README.md`
documents each fixture, the pinned tools, and why both WASI 0.3
handlers stop at link under the harness.

`wast` has no syntax for a `map` value or a fixed-length list value.
A directive spells a map as a list of two-element tuples, the map's
canonical-ABI layout, and a fixed-length list as a list; the harness
turns them into the polyfill's values wherever the invoked function
declares a `map` or a `list<T, N>`, for arguments and expected results
alike.

## Baseline

The progress summary on the native target, as of 2026-09-25 (`tests
conformance` prints the current one):

| Corpus           | Directives | Passed | Pass % | Expected failures by category                                 |
| ---------------- | ---------- | ------ | ------ | ------------------------------------------------------------- |
| `cm`             | 1126       | 1038   | 92.2   | deferred-feature 2, substrate 4, validation 20, cascade 62    |
| `cm/async`       | 393        | 214    | 54.5   | deferred-feature 38, cascade 141                              |
| `fixtures`       | 56         | 50     | 89.3   | deferred-feature 2, cascade 4                                 |
| `wasmtime`       | 469        | 431    | 91.9   | deferred-feature 2, substrate 8, cascade 28                   |
| `wasmtime/async` | 387        | 326    | 84.2   | deferred-feature 39, cascade 22                               |
| total            | 2431       | 2059   | 84.7   | deferred-feature 83, substrate 12, validation 20, cascade 257 |

The browser's summary differs by the ten lines of
`expected-failures.web.txt`, which move ten passing directives into
`substrate`: `cm` passes 1037 (92.1%) with substrate 5, `cm/async` 213
(54.2%) with substrate 1, `wasmtime` 425 (90.6%) with substrate 14,
`wasmtime/async` 324 (83.7%) with substrate 2, and the total is 2049
(84.3%) with substrate 22. Every other cell is the same. Five of the
ten lines, among them the three in the `async` rows, are the browser
engine's wording for a trap or a validation error that Wasmtime words
differently. Two in `wasmtime/big-strings.wast` trap in the adapter
before the bounds check Wasmtime reaches, and three in
`wasmtime/memory64.wast` need allocations past 4 GiB that 32-bit code
in the browser cannot address. No line of the delta is a difference
of the polyfill.

The `async` rows still hold the pass rate down, `cm/async` far more
than `wasmtime/async`, and the six reasons above cover what those
directories still exercise. Each component those directories define
that the polyfill rejects is a `deferred-feature` failure, and every
later directive in the same file that names it is a `cascade` one, so
the two async rows together hold 163 of the 257 cascade lines. Four
files of `cm/async` whose components need a thread built-in hold 116
of them: `trap-if-block-and-sync.wast` 47,
`trap-if-sync-and-waitable-set.wast` 28,
`during-sync-scheduling-candidates.wast` 25, and
`switch-to-ready-callback.wast` 16.

Of the 38 files of `cm/async`, 21 pass whole on both targets. Of the
54 files of `wasmtime/async`, 37 pass whole natively and 35 in the
browser, where `subtask-wait.wast` and `sync-call-context-trap.wast`
each hold one line of the browser's delta. Seven of the nine fixtures
pass whole.

The streams and futures account for 33 of the files that pass whole.
In `cm/async`: `cancel-stream.wast`, `closed-stream.wast`,
`cross-task-future.wast`, `drop-cross-task-borrow.wast`,
`drop-stream.wast`, `empty-wait.wast`, `futures-must-write.wast`,
`partial-stream-copies.wast`, `passing-resources.wast`,
`same-component-stream-future.wast`, `trap-if-done.wast`,
`trap-if-transfer-in-waitable-set.wast`, `validate-no-stream-char.wast`,
`wait-during-callback.wast`, and `zero-length.wast`. In
`wasmtime/async`: `async-builtins.wast`,
`future-cancel-read-dropped.wast`,
`future-cancel-write-completed.wast`,
`future-cancel-write-dropped.wast`,
`future-drop-writable-after-notified-drop.wast`, `future-read.wast`,
`futures-must-write.wast`, `futures-must-write2.wast`, `futures.wast`,
`intra-futures.wast`, `intra-streams.wast`,
`partial-stream-copies.wast`, `stream-big-read-and-writes.wast`,
`stream-cancel-finished-op.wast`, `streams.wast`,
`sync-and-async-waitable.wast`, `trap-if-transfer-in-waitable-set.wast`,
and `waitable-set-stale-entry.wast`. `cm/async/trap-if-done.wast`
passes five of its directives by the wording rule the harness takes
from Wasmtime's runner. The busy-drop directive of
`cm/async/builtin-trap-poisons-instance.wast` passes too, and its two
directives that expect the poisoning trap stay deferred on the trap
rules. The first four components of
`wasmtime/async/cancel-sync-and-waitable.wast` pass, and its fifth
fails at its call to `subtask.cancel`. The `subtask.cancel` component
of `wasmtime/async/task-builtins.wast` instantiates.

Seven files that exercise a stream or a future keep lines deferred on
a stack switch, which the suspend capability serves and which has no
provider on either target until the stackful design fills it. Both
`sync-streams.wast` and `wasmtime/async/streams-massive-send.wast`
have a callee that writes synchronously after `task.return` for its
caller below it to read, the massive send once with a stream and once
with a future. The repository's tests of the copy budget stand in for
the massive send's stream write and future write.
`wasmtime/async/stream-zero-ops.wast` passes every directive but one,
whose synchronously lifted callee, reached through an asynchronous
lower, blocks in `waitable-set.wait` until its caller, below it on the
stack, writes. `wasmtime/async/trap-if-done.wast` passes every
directive but seven, which have a synchronously lifted callee, reached
through an asynchronous lower, that writes its future synchronously
for its caller below it to read. The stream and future case of
`wasmtime/async/task-builtins.wast` has a callee that reads
synchronously in its first core function what only its caller writes,
and the one directive of `cm/async/cancel-and-exclusive-lock.wast`
that fails has a callee that blocks in `waitable-set.wait` until its
caller writes a future. `cm/async/async-calls-sync.wast` and
`wasmtime/async/reenter-during-yield.wast` wait on a stack switch as
well, for a callee that only an outer caller can release.

The rest of the async rows wait on the other reasons. The stackful
lift holds `cm/async/sync-barges-in.wast`,
`wasmtime/async/stackful.wast`,
`wasmtime/async/drop-waitable-set-stackful.wast`,
`wasmtime/async/task-deletion.wast`, four components of
`wasmtime/async/task-return-traps.wast`, and the directives of
`cm/async/big-interleaving-test.wast` that do not reach a cancel, as
well as one directive of `cm/values/variants.wast`. A thread built-in
holds the four `cm/async/during-sync-*.wast` files,
`cm/async/self-switch-traps.wast`,
`cm/async/switch-to-ready-callback.wast`,
`cm/async/trap-if-block-and-sync.wast`,
`cm/async/trap-if-sync-and-waitable-set.wast`,
`wasmtime/async/join-during-sync-read.wast`, the other two components
of `wasmtime/async/task-return-traps.wast`,
`cm/values/post-return.wast`, and
`wasmtime/thread-transparency/reentrancy.wast`.
Cancellation holds `cm/async/cancel-delivery.wast`,
`cm/async/cancel-subtask.wast`, `wasmtime/async/cancel-host.wast`,
`wasmtime/async/cancel-sibling-subtask.wast`,
`wasmtime/async/yield-when-cancelled.wast`, and two directives of
`cm/async/big-interleaving-test.wast`, each of which instantiates and
fails at its call to `subtask.cancel`.
`wasmtime/async/cancel-starting-subtask-does-not-leak.wast` stops at
link, before its cancel, on the host item the harness does not
provide. Eight of the twelve cases of `cm/async/reentrance.wast` pass.
Of the four that remain, two need a thread built-in, one fails at its
call to `subtask.cancel`, and one traps before its cancel because the
deadlocked call of an earlier case leaves its callee waiting and a trap
does not poison that callee's instance yet. Error contexts hold
`wasmtime/async/error-context.wast` and
`wasmtime/error-context-trap-in-post-return.wast`. Of the three corpus
files that exercise a host `async` item, `wasmtime/async/lower.wast`
and `wasmtime/async/drop-host.wast` pass whole, and
`wasmtime/async/cancel-host.wast` is held up by cancellation.
