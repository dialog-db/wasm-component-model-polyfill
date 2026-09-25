---
id: c73ebd
title: Close the stream and future corpus lines and refresh the async baseline
type: chore
blocked_by: [b71981, ad5dd3, b07a9c, a31891, 9d86f4, 3fdd33, 88e273, f65b25, 9fc0f4]
labels: [PDD021, concurrency, conformance]
created: 2026-09-23T06:00:42Z
---

## What to build
With every piece of the feature in place, audit `tests/corpus/expected-failures.txt` and `expected-failures.web.txt` against the design's Corpus section. The list loses every line the owned files and directives account for, and no other line. In the Component Model corpus: `builtin-trap-poisons-instance.wast` except its two poisoning directives, `cancel-stream.wast`, `closed-stream.wast`, `cross-task-future.wast`, `drop-stream.wast`, `empty-wait.wast`, `futures-must-write.wast`, `partial-stream-copies.wast`, `same-component-stream-future.wast`, `trap-if-done.wast`, `trap-if-transfer-in-waitable-set.wast`, `wait-during-callback.wast`, `zero-length.wast`, `drop-cross-task-borrow.wast`, and `passing-resources.wast`. In the Wasmtime corpus: `async-builtins.wast`, `futures.wast`, `streams.wast`, the first four components of `cancel-sync-and-waitable.wast`, `future-cancel-read-dropped.wast`, `future-cancel-write-completed.wast`, `future-cancel-write-dropped.wast`, `future-drop-writable-after-notified-drop.wast`, `futures-must-write.wast`, `futures-must-write2.wast`, `intra-futures.wast`, `intra-streams.wast`, `partial-stream-copies.wast`, `stream-cancel-finished-op.wast`, `stream-zero-ops.wast`, `sync-and-async-waitable.wast`, `trap-if-done.wast`, `trap-if-transfer-in-waitable-set.wast`, `waitable-set-stale-entry.wast`, `future-read.wast`, `stream-big-read-and-writes.wast`, `streams-massive-send.wast` through the copy budget, and the stream and future case and the `subtask.cancel` component of `task-builtins.wast`. Each remaining line of a deferred file carries the design's reason from a live run: a stack switch (`sync-streams.wast` in both corpora and `async-calls-sync.wast`), the stackful lift, a thread built-in, cancellation, error contexts, or the trap rules. Refresh `tests/corpus/README.md`: the baseline table's `cm/async` and `wasmtime/async` rows and the prose that says what the polyfill runs now. Both targets report the same results for every named file and every repository test of the feature, with the V8 wording differences the `substrate` lines already record.

## Notes from earlier cards
- The Corpus section above predates owner decisions made while the thread was built. These files are **accepted as deferred**, not owed: `stream-zero-ops.wast:201`, `streams-massive-send.wast` (the budget is proved by a repository test), the seven stack-switch lines of `wasmtime/async/trap-if-done.wast`, and `sync-streams.wast` in both corpora — all need a stack switch, and their hand notes name the missing suspend provider and the stackful design; keep those notes true. `cm/async/trap-if-done.wast` passes through the harness's mirror of Wasmtime's wast-runner wording rule (`wast.rs:551-554`, `cm/` only). The two `wasi-http` fixtures' `drain` lines stay cascades (no `wasi:http/types` host in the harness); the `wasi-http-same-instance` fixture is a deliberate spec tripwire whose hand note names the spec's temporary same-instance rule. `cancel-starting-subtask-does-not-leak.wast:9` needs a harness host item (`set-max-table-capacity`) the design forbids adding. Record every file the Corpus section names that still does not pass, with its live reason, rather than forcing it.
- The `tests/corpus/README.md` baseline table is dated 2026-09-23 and stale in several rows; refresh the whole table from a live `tests conformance` run on both targets, not only the async rows.
- Push your commits before the long gate runs (the host rebooted once during this thread).
- Do not edit `project/design/`; list every place the design's Corpus section disagrees with the live result, so the orchestrator can reconcile the design.

## Acceptance criteria
- [ ] The expected-failure list loses every line the owned files and directives account for, and no other line, and each remaining line of a deferred file states the design's reason from a live run.
- [ ] `streams-massive-send.wast` passes whole through the copy budget.
- [ ] The progress summary shows the two `async` rows with the new counts, the same on both targets, and the README's baseline table and prose match a live run.
- [ ] The native run and the browser run report the same pass and failure results for every named file and every repository test of the feature.
- [ ] `lint` passes and `tests all` is green on both targets.

