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

## Acceptance criteria
- [ ] The expected-failure list loses every line the owned files and directives account for, and no other line, and each remaining line of a deferred file states the design's reason from a live run.
- [ ] `streams-massive-send.wast` passes whole through the copy budget.
- [ ] The progress summary shows the two `async` rows with the new counts, the same on both targets, and the README's baseline table and prose match a live run.
- [ ] The native run and the browser run report the same pass and failure results for every named file and every repository test of the feature.
- [ ] `lint` passes and `tests all` is green on both targets.

