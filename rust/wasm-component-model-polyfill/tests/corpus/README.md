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

The polyfill runs both forms of an asynchronous export, the callback
form and the stackful form, the task built-ins that come with them, and
both lowers of a call out through an import, so part of `cm/async/` and
`wasmtime/async/` passes: a host call into such an export,
`task.return`, backpressure, the waitable set built-ins, `thread.yield`,
the other eight thread built-ins, and the context slots. A stackful
export's core function runs as its task's implicit thread, and it does
not take its instance exclusively. Natively on x86_64 Linux every
thread entry runs on a stack of its own through the stack-switching
provider, and in the browser through the JSPI provider: a blocking
built-in suspends the thread, the five thread built-ins that suspend
or switch suspend it the same way, and a switch starts or resumes the
thread it names there. A thread of a sync-typed call suspends its
stack only to switch, and only when it runs on a stack of its own, as
the thread of a host call does, or when a block of its own instance
started or resumed it. Otherwise it cannot suspend its stack, and
without a provider, in the `no-provider` lanes, no thread can. Such a
thread waits in a nested turn: a suspension waits in turns run from inside the built-in,
a switch to a thread that never ran starts that thread above the
built-in, and a switch to a thread suspended below the current frame
fails with the stack-switch message. Under a provider the nested turn
of a sync-typed call runs the ready threads of the call's own instance,
its threads suspended in the provider included, and a thread of that
instance that the turn or a switch of the call started or resumed
suspends back into it, so it can switch back to the call's thread
below. A call between two components crosses in all
four combinations of lift and lower, and the readable end of a stream
or a future crosses with it. A read and a
write on the two ends of a stream pair up as the reference's stream
state pairs them, with partial and zero-length copies, and a copy
that does not finish at once completes through an event on its end.
A future's read and write are the same copy of one value, and each
end of a future copies once. A cancel ends the copy in progress on
one end and reports the progress it made, as Wasmtime reports it.
Owned handles cross a stream as its payload. A payload of a number
type copies between two ends one instance holds, and any other payload
traps there with Wasmtime's refusal. A caller cancels a call into
another component with `subtask.cancel`: a callee the entry gate holds
never runs, a callback callee takes the request as the task-cancelled
event and confirms it with `task.cancel` or returns all the same, and a
stackful callee is never told, so the cancel waits for its return. A
call into a host function is cancelled by dropping its future: the
cancel marks the call's host task as aborted, the next turn that polls
the host tasks drops the future, and the call resolves as cancelled
before it returned, or as returned when its future completed first. The
asynchronous lower answers
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
a synchronous call into the blocked thread's own instance is in
progress, or into the instance of a caller below that waits for it,
the stack-switch message when only a real suspension could wait, and
the deadlock message otherwise. A real
suspension could wait when a host task is pending, or when the blocked
callee runs above a caller that would go on under a stack switch: a
caller that lowered the call asynchronously, or synchronously once the
callee returned. An exception thrown in a callee reaches the host as the
trap the synchronous baseline gives it.

The directive that first meets what is missing is an expected failure
of category `deferred-feature`, for one reason: a block or a
switch that only a stack switch can serve, such as a callee that can
be released only by a caller that is on the stack or a thread that
suspends after its task has resolved. Two definitions in the same
category fail at link instead, on a
host item the harness does not provide: the two WASI 0.3 handler
fixtures import `wasi:http/types`. Most of the rest
is `cascade`: a component definition that fails leaves its name
unbound and no instance current, so every later directive in the file
that names the definition or invokes the instance fails as
bookkeeping rather than on its own merits. A trap poisons the store the
instance runs in, as in Wasmtime, so a later directive that invokes an
instance a trap ended fails with `cannot enter component instance`, and
is a `cascade` line too. Several files of
`cm/async` define a component once and then drive it over dozens of
directives, so a failure early in one of them cascades over the rest;
natively that row has no `cascade` line today, and without a provider
it has ten.

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
same group, which is how a runtime wording that ends in a group of its
own is told apart from a note. When the runtime's wording drops such a group, the
rewrite reads the old group as a note and restores it after the new
reason, so that line needs a person to strike the group. A reason
that ends in the run's cause behind other leading text is a sentence
a person wrote over that cause, and it is kept whole: the nineteen
such lines say in one clause what the substrate
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
differences, eleven lines today, and each one is written by hand from
the failure a `tests web debug` run prints.

The shared list records the best case: the failures under a suspend
provider. `expected-failures.no-provider.txt` is the overlay of the
directives that fail beyond it without one. The harness applies it
whenever the store runs no guest thread through a provider, on either
target, because the nested turn is one code path on both: a lane that
turns the provider off, a native build on a platform without the
stack-switching proposal, and a browser without JSPI. A browser that
ships JSPI answers the JSPI provider, which runs guest threads as the
stack-switching provider does, so the overlay does not apply there. In
a browser the web delta applies beside it. Every line of the overlay
carries the stack-switch reason, the text of the scheduler's
stack-switch cause, and no line may name a directive the shared list
names; the harness fails the run otherwise.

Each corpus file is two tests. `it_passes_<file>` runs it with the
provider allowed, and `it_passes_<file>::it_passes_without_a_provider`
runs it with the provider turned off through `EngineConfig`. The
nextest profiles in `.config/nextest.toml` at the workspace root split
them: the ordinary lanes run the first, and `tests native no-provider`
and `tests web no-provider` run only the second, from the debug
archives. `tests all` runs the corpus in all four states. The native
lanes run the stack-switching provider on the x86_64 Linux host, the
web lanes run the JSPI provider in the flake's Chromium, and the
overlay holds the directives only a stack switch passes. The provider
states of both targets pass the same directives, and the web delta is
the only difference between them.

`tests regenerate` writes both lists from the native debug archive: the
shared list from the progress run with the provider allowed, then the
overlay from `it_reports_conformance_progress_without_a_provider`, which
keeps only the failures the regenerated shared list does not name
(`WCMP_REGENERATE_BASE` names that list). A directive that fails with
a provider and passes without one has no place in either list, so the
shared list names it, and `expected-passes.no-provider.txt` names it
again: the harness drops the directive from the expectations of a run
in which no provider runs the store's threads, and fails a run whose
list names a directive the shared list does not. That list is written
by hand, with the reason the directive passes without a provider, and
`tests regenerate` neither reads nor writes it.

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
beside the spectest for its own misc tests, with two items. `gc` does
nothing, because the polyfill's substrate collects its own garbage.
`set-max-table-capacity` sets the cap on the store's live records, as
the runner sets the capacity of the store's concurrent table. The cap
is set only through the store's internal API, which an integration
test cannot see, so the crate exports the one entry the item calls
under its `wast-runner` feature, and only the crate's own tests turn
the feature on, through a dev-dependency on the crate itself.

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
six — `rich`, `streams`, `stream-composition`, `sync-wait`,
`wasi-http`, and `wasi-http-same-instance` — from Rust crates that `cargo` and
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

The corpus runs in four states: each target with the suspend provider
and without one. The figures below come from one `tests all` run of
2026-09-27, and the provider-off tables from the `tests regenerate
--dry-run` of the same tree, which left both lists as they were
(`tests conformance` prints the current provider tables).

The progress summary on the native target, with the stack-switching
provider:

| Corpus           | Directives | Passed | Pass % | Expected failures by category                                          |
| ---------------- | ---------- | ------ | ------ | ---------------------------------------------------------------------- |
| `cm`             | 1126       | 1096   | 97.3   | substrate 4, validation 20, cascade 6                                  |
| `cm/async`       | 393        | 393    | 100.0  | none                                                                   |
| `fixtures`       | 59         | 53     | 89.8   | deferred-feature 2, cascade 4                                          |
| `wasmtime`       | 469        | 441    | 94.0   | substrate 8, cascade 20                                                |
| `wasmtime/async` | 387        | 380    | 98.2   | deferred-feature 6, cascade 1                                          |
| total            | 2434       | 2363   | 97.1   | deferred-feature 8, substrate 12, validation 20, cascade 31            |

The browser runs its guest threads through the JSPI provider, so its
summary is the native one, and the eleven lines of
`expected-failures.web.txt` move eleven passing directives into
`substrate`: `cm` passes 1095 (97.2%) with substrate 5, `cm/async` 392
(99.7%) with substrate 1, `wasmtime` 434 (92.5%) with substrate 15,
`wasmtime/async` 378 (97.7%) with substrate 2, and the total is 2352
(96.6%) with substrate 23. Six of the
eleven lines, among them the three in the `async` rows, are the browser
engine's wording for a trap or a validation error that Wasmtime words
differently. Two in `wasmtime/big-strings.wast` trap in the adapter
before the bounds check Wasmtime reaches, and three in
`wasmtime/memory64.wast` need allocations past 4 GiB that 32-bit code
in the browser cannot address. No line of the delta is a difference
of the polyfill.

Without a provider, the 56 lines of `expected-failures.no-provider.txt`
fail beyond the shared list, 32 in `cm/async` and 24 in
`wasmtime/async`. Each `deferred-feature` line among them carries the
stack-switch reason, and each `cascade` line follows a directive that
does. The other three rows do not change. The native summary with
the provider turned off:

| Corpus           | Directives | Passed | Pass % | Expected failures by category                                          |
| ---------------- | ---------- | ------ | ------ | ---------------------------------------------------------------------- |
| `cm`             | 1126       | 1096   | 97.3   | substrate 4, validation 20, cascade 6                                  |
| `cm/async`       | 393        | 361    | 91.9   | deferred-feature 22, cascade 10                                        |
| `fixtures`       | 59         | 53     | 89.8   | deferred-feature 2, cascade 4                                          |
| `wasmtime`       | 469        | 441    | 94.0   | substrate 8, cascade 20                                                |
| `wasmtime/async` | 387        | 356    | 92.0   | deferred-feature 22, cascade 9                                         |
| total            | 2434       | 2307   | 94.8   | deferred-feature 46, substrate 12, validation 20, cascade 49           |

No line of the web delta names a directive of the overlay, so the
browser without a provider moves the same eleven directives into
`substrate`: `cm` passes 1095 (97.2%), `cm/async` 360 (91.6%),
`wasmtime` 434 (92.5%), `wasmtime/async` 354 (91.5%), and the total is
2296 (94.3%) with substrate 23.

The `async` rows still hold the pass rate down, and the two reasons
above cover what those directories still exercise. A directive that
fails on one of them can leave a later directive of the same file
without its instance, or with a thread or a callee the failure left
behind in the same instance, and each such later directive is a
`cascade` line. Each component instance runs in a store of its own,
as under Wasmtime's wast runner, so what an earlier instance left
behind never runs during a later instance's call. The two async rows
hold 1 of the 31 cascade lines natively, in `wasmtime/async`, and
without a provider `cm/async` adds 10, nine in
`cm/async/during-sync-scheduling-candidates.wast` and one in
`cm/async/async-calls-sync.wast`, and `wasmtime/async` adds 8, all in
`wasmtime/async/task-deletion.wast`, each after a directive whose
stack-switch failure poisoned the store.

Of the 38 files of `cm/async`, all 38 pass whole natively and 37 in
the browser, where `builtin-trap-poisons-instance.wast` holds one line
of the browser's delta, and 25 natively without a provider, where
thirteen hold lines that only a provider serves, and 24 in the browser
without one. Of the 54 files of `wasmtime/async`, 52 pass whole
natively and 50 in the browser, where `subtask-wait.wast` and
`sync-call-context-trap.wast` each hold one line of the browser's delta,
and 43 natively without a provider, where nine more hold lines that
only a provider serves, and 41 in the browser without one. Eight of the ten fixtures pass whole.

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
from Wasmtime's runner. `cm/async/builtin-trap-poisons-instance.wast`
passes whole natively: each of its two traps poisons the store, and the
call after it fails with the cannot-enter trap.
`wasmtime/async/cancel-sync-and-waitable.wast` passes whole, its fifth
component trapping in `subtask.cancel` of a subtask in a waitable set as
it expects. The `subtask.cancel` component
of `wasmtime/async/task-builtins.wast` instantiates.

Nine files that exercise a stream or a future keep lines that pass
under a provider and fail without one, in the `no-provider` lanes,
where they wait on a stack switch. Both
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
that fails without a provider has a callee that blocks in
`waitable-set.wait` until its caller writes a future; under the
provider it passes. `cm/async/cancel-delivery.wast` and
`cm/async/cancel-subtask.wast` each have a callee that blocks the same
way, on a future read or a wait, until its caller goes on to cancel it
and write the future. `cm/async/async-calls-sync.wast` and
`wasmtime/async/reenter-during-yield.wast` wait on a stack switch as
well, for a callee that only an outer caller can release. So do
`cm/async/sync-barges-in.wast` and
`wasmtime/async/drop-waitable-set-stackful.wast`, whose stackful
callee blocks in `waitable-set.wait` inside its core function until
its caller, below it on the stack, goes on.

The stackful lift runs, so `wasmtime/async/stackful.wast` passes
whole, and so do the four stackful components of
`wasmtime/async/task-return-traps.wast`,
`cm/async/big-interleaving-test.wast`, and the stackful directive of
`cm/values/variants.wast`. `thread.index`, `thread.new-indirect`, and
`thread.resume-later` run too, so the other two components of
`wasmtime/async/task-return-traps.wast` pass and that file passes
whole. They pass by the reference's path: the explicit thread each
one makes ready runs after the implicit thread exits, and the
no-result failure comes as that thread, the task's last, ends.
`cm/values/post-return.wast` and, natively,
`wasmtime/thread-transparency/reentrancy.wast` pass whole too. The five
thread built-ins that suspend or switch run too, so
`cm/async/self-switch-traps.wast`,
`cm/async/during-sync-call-exclusive-resume.wast`,
`cm/async/during-sync-call-may-block-if-other-ready-threads.wast`,
`cm/async/during-sync-call-no-sibling-resume.wast`, and
`cm/async/during-sync-scheduling-candidates.wast` pass whole
natively, and so do `cm/async/trap-if-block-and-sync.wast` and
`cm/async/switch-to-ready-callback.wast`. Natively the
thread lines of `cm/async/trap-if-sync-and-waitable-set.wast` and
`wasmtime/async/join-during-sync-read.wast` pass too, and so do the
lines of `during-sync-scheduling-candidates.wast` whose thread
suspends or yields after its task resolved, because each of those
threads suspends in the provider. A thread of a sync-typed call cannot
suspend its stack even under a provider, but its block runs the ready
threads of its own instance, as the reference's `canon_lift` does: a
yield of `during-sync-scheduling-candidates.wast` resumes the thread
`thread.resume-later` made ready in the provider, and in two lines
each of `trap-if-block-and-sync.wast` and the two `during-sync-call`
files a thread that the sync-typed thread switched to suspends back
into its built-in, or switches back to it. The thread of a host call
into a sync-typed export runs on a stack of its own and suspends
through the provider when it switches, so a thread that a callee of a
synchronous call started switches back to it too, as in a line of
`cm/async/reentrance.wast`. Without a provider the other lines named here
wait on a stack switch as well: a switch back to a thread suspended
below the current frame, a thread that blocks above a thread that
yielded to it, a thread that suspends above the asynchronous lower
that started it, or a thread that suspends or yields for ever after
its task resolved, whose host call cannot return while the thread's
frame is on the stack. `cm/async/switch-to-ready-callback.wast` passes
whole natively; without a provider two of its directives fail, whose
test function suspends above the asynchronous lower of its caller
where the reference deadlocks. A task lives until its last thread
ends, so `wasmtime/async/task-deletion.wast` passes whole natively and
in the browser:
each explicit thread runs after the implicit thread of its task has
exited, one of them calls `task.return`, and the others suspend or
yield for ever in the provider after their calls returned. Without a
provider its first directive waits on a stack switch, for those
threads, which start in the nested turns of `run` and stay on the
stack above it. The rest of the async rows wait on the other reasons.
The cancellation of a call into another component runs, so
`cm/async/cancel-delivery.wast`, `cm/async/cancel-subtask.wast`,
`cm/async/cancel-and-exclusive-lock.wast`,
`wasmtime/async/cancel-sibling-subtask.wast`,
`wasmtime/async/cancel-starting-subtask-does-not-leak.wast`, and
`wasmtime/async/yield-when-cancelled.wast` pass whole natively and in
the browser, and so do the cancel directives of
`cm/async/big-interleaving-test.wast` and
`cm/async/trap-if-sync-and-waitable-set.wast`.
`wasmtime/async/cancel-starting-subtask-does-not-leak.wast` links
against the harness's `wasmtime.set-max-table-capacity`, which lowers
the cap on the store's live records to 100, and its 1,000 cancels of
a starting subtask each free the subtask's records, so the cap holds.
`cm/async/trap-if-block-and-sync.wast:352` passes under the provider
too: the callee its sync-typed export lowers asynchronously suspends,
the lower answers `STARTED`, and the export's synchronous
`subtask.cancel` fails with the cannot-block message. Without a
provider that callee blocks on the real stack above the asynchronous
lower, which a stack switch would let go on, so the block fails with
the stack-switch message before the cancel.
`wasmtime/async/cancel-host.wast` passes whole natively and in the
browser, with a provider and without one: a cancel of a call into a
host function drops the call's future in the next turn that polls the host
tasks, and the borrow the call held comes back when the resolution is
delivered.
The twelve cases of
`cm/async/reentrance.wast` pass, two of them by cancelling a callee
parked in its callback loop, which `subtask.cancel` wakes and gives
way to from inside its own frame, as Wasmtime does. The
start intrinsic clears the may-not-suspend flag of an `async`-typed
callee's instance while it runs the callee, as Wasmtime does, so a
callee that an asynchronous lower began while synchronous calls below
it are in progress suspends and hands control back to its caller.
Without a provider two of the twelve wait on a stack switch: that callee
suspends above the asynchronous lower that started it, and a thread
switches back to a thread suspended below it. The error-context
built-ins run, so `wasmtime/async/error-context.wast` and
`wasmtime/error-context-trap-in-post-return.wast` pass whole. Of the three corpus
files that exercise a host `async` item, `wasmtime/async/lower.wast`,
`wasmtime/async/drop-host.wast`, and `wasmtime/async/cancel-host.wast`
pass whole.
