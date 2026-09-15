---
id: ff8c3b
title: 64-bit memories in adapter modules
type: feature
blocked_by: []
labels: [parity, conformance]
created: 2026-09-15T16:37:10Z
---

## What to build
A fused adapter between two components whose linear memories are 64-bit passes i64 pointers and lengths to the string transcoders. The translator rejects a transcoder with `from64` or `to64` set at `src/executor/translate.rs` ("64-bit memories in adapter modules"). Wasmtime supports memory64 components end to end. Give the transcoder trampolines i64-aware signatures and pointer arithmetic, and check that lift and lower of the host boundary already handle a 64-bit memory (a `canon lift` with a memory64 option) or extend them too.

Parity: the runtime layer exposes 64-bit memories on both targets (the browser through the JS API's memory64 support, which the outlook lists as shipped everywhere); the test runs in both lanes.

Corpus: 1 `deferred-feature` line in `wasmtime/memory64.wast` and its 3 cascades.

## Acceptance criteria
- [ ] The `wasmtime/memory64.wast` lines are removed from `expected-failures.txt` (or re-categorized as `substrate` with the exact runtime-layer reason), and the corpus is regenerated.
- [ ] A string crosses a composed component with 64-bit memories on both targets.
- [ ] `tests all` passes on both targets.

