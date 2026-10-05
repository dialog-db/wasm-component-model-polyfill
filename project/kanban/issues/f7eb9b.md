---
id: f7eb9b
title: Speed up and prove the browser memory accessor fallback paths
type: chore
blocked_by: [215523]
labels: [runtime-layer, backlog-burndown-001-q3]
created: 2026-09-29T13:36:41Z
---

## What to build

Non-blocking findings from the independent review of 215523 (browser memory access, landed `da9de8ebb`) in `rust/wcmp-wasm-core-web`:

- **Shared memory without `multi_memory` (Safari's main path).** Shared copies make one indirect-call pair per byte (`store.rs` `read`/`write`/`memory_copy`). Batch through `load64`/`store64` in the accessor, still atomic per byte inside one call, for about 8x fewer calls.
- **Isolation claim.** wbg-pool serves every page with COOP/COEP, so no lane proves that a page makes a shared memory without cross-origin isolation. Add a wbg-pool page without those headers, or state the limit. This matters most for Safari.
- **Missing tests.** Two stores where one drops and a third reuses the freed slots while the survivor keeps reading; a second-thread (worker) dispatcher install; the fallback 64-bit view error past 4 GiB; a host function using memory on the path without `multi_memory` (the new host-function test asserts `multi_memory`); count accessor instantiations to prove reuse (`tests/web.rs:1179-1181` claims it, but zero `set` calls cannot prove it under `multi_memory`).
- **Hygiene.** `Error.stackTraceLimit` is set globally and never restored (`accessor.rs:520`); the `Uint8Array.prototype.set` spy is not restored if the body panics.

## Acceptance criteria
- [ ] Shared copies without `multi_memory` batch at least eight bytes per call.
- [ ] A lane shows the non-isolated case, or the crate docs state the limit.
- [ ] The missing tests exist and run in the web lanes.
- [ ] Global test state is restored on every path.
- [ ] `tests all` and `lint` pass.

## Review notes

