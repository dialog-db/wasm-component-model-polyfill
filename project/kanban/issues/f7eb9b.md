---
id: f7eb9b
title: Speed up and prove the browser memory accessor fallback paths
type: chore
blocked_by: [215523]
labels: [runtime-layer, backlog-burndown-001-q3]
created: 2026-09-29T13:36:41Z
disposition: accepted
disposition_at: 2026-10-16T06:30:36Z
---

## What to build

Non-blocking findings from the independent review of 215523 (browser memory access, landed `da9de8ebb`) in `rust/wcmp-wasm-core-web`:

- **Shared memory without `multi_memory` (Safari's main path).** Shared copies make one indirect-call pair per byte (`store.rs` `read`/`write`/`memory_copy`). Batch through `load64`/`store64` in the accessor, still atomic per byte inside one call, for about 8x fewer calls.
- **Isolation claim.** wbg-pool serves every page with COOP/COEP, so no lane proves that a page makes a shared memory without cross-origin isolation. Add a wbg-pool page without those headers, or state the limit. This matters most for Safari.
- **Missing tests.** Two stores where one drops and a third reuses the freed slots while the survivor keeps reading; a second-thread (worker) dispatcher install; the fallback 64-bit view error past 4 GiB; a host function using memory on the path without `multi_memory` (the new host-function test asserts `multi_memory`); count accessor instantiations to prove reuse (`tests/web.rs:1179-1181` claims it, but zero `set` calls cannot prove it under `multi_memory`).
- **Hygiene.** `Error.stackTraceLimit` is set globally and never restored (`accessor.rs:520`); the `Uint8Array.prototype.set` spy is not restored if the body panics.

## Acceptance criteria
- [x] Shared copies without `multi_memory` batch at least eight bytes per call.
- [x] A lane shows the non-isolated case, or the crate docs state the limit.
- [ ] The missing tests exist and run in the web lanes.
- [x] Global test state is restored on every path.
- [ ] `tests all` and `lint` pass.

## Review notes
- 2026-10-07 delivered in `720f5fe8` (host session, backlog-burndown-001-q3).
  - Batching: `Accessor::read_atomic`, `write_atomic` and `copy_atomic` move
    eight bytes per `load64`/`store64` call, then the tail byte by byte. A
    copy still runs in the direction that reads each byte before it is
    overwritten. `it_copies_a_shared_memory_eight_bytes_in_each_call_without_bulk_copies`
    counts calls through a test-only counter (35 bytes: 4 words + 3 bytes),
    with overlaps of fewer bytes than a word in both directions.
  - Isolation: the crate docs now state the limit. The old claim that a page
    makes a shared memory without cross-origin isolation is gone; no lane
    shows it, because wbg-pool serves every page with COOP/COEP.
  - Tests: survivor plus third accessor taking freed slots (unit test, and a
    store-level test that counts `WebAssembly.Table.prototype.grow` = 0); a
    host function without `multi_memory` (2 `TypedArray.set`, 0
    instantiations on the second call); the host-function test now counts
    `WebAssembly.Instance` constructions (>= 1, then 0); the fallback view
    error past 4 GiB on a 65 537-page memory64 (`write`, `read`,
    `with_bytes`, `copy`).
  - Not done: a worker dispatcher install. The polyfill builds without the
    `atomics` target feature, so a worker runs its own instance with its own
    table and dispatcher. The crate docs now say so, and that no test runs the
    polyfill in a worker.
  - Hygiene: one `Setting` guard (tests/web.rs, accessor.rs unit tests) puts
    back `Error.stackTraceLimit`, the `Uint8Array.prototype.set`,
    `Promise.prototype.then` and `Table.prototype.grow` spies, and the
    `WebAssembly.*` overrides on every unwinding path. A panic aborts the
    page, and wbg-pool gives each test a fresh page, so nothing leaks.
  - Gate: web lane for the crate 105/105, `lint clippy-wasm32 rustfmt` green.
    `tests all` and full `lint` run on the composed series tip.
- 2026-10-07 context-free review: the "missing tests" box is unticked because the worker dispatcher
  install test is not written. The reason is above: the build has no
  `atomics`, so a worker runs its own instance. The owner decides whether
  the stated limit closes it.
