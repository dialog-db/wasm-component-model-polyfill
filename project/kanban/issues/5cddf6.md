---
id: 5cddf6
title: The record cap stays internal, and its full-table cause moves out of the scheduler
type: bug
blocked_by: []
labels: [concurrency, backlog-burndown-001-q3]
created: 2026-09-27T03:16:16Z
---

## What to build
Close the findings the review of card 2170bc left open.

1. **The cap setter must not be reachable from a downstream crate.** The conformance harness reaches the internal record-cap setter through `wast-runner`, an ordinary Cargo feature (`Cargo.toml:18`), and a `#[doc(hidden)]` re-export `set_max_table_capacity` (`src/lib.rs:292`). Any crate can turn the feature on, so "no public method changes the cap" holds only for default builds, and the `api` gate cannot see it because rustdoc JSON drops hidden items. Gate the item on something a downstream crate cannot set by accident — for example a `--cfg` that the test derivation passes — or state in the review notes why the feature is acceptable.
2. **Move `TableFull` out of `SchedulerCause`.** The `Error::Scheduler` doc (`src/error.rs:132`) says the driver could not be carried through a turn, and the `SchedulerCause` doc (`src/error.rs:858-864`) names the scheduler, `run_concurrent` or the suspend seam as the raisers. The record tables raise `TableFull` when a record is created, with no turn involved. Give it a store- or resource-level home and keep Wasmtime's message "resource table has no free keys". Record the public-surface change with `api update`.
3. **Admit a host call before its body runs.** A host call is admitted only after its first poll returns `Pending` (`src/store/store_context.rs:636`, `:713`), so a call refused at the cap has already run the first poll of its body. Wasmtime pushes the `HostTask` before the call (`crates/wasmtime/src/runtime/component/concurrent.rs:1962` at `v49.0.0-rc.1`). Admit first, or say in the review notes why not.
4. **Missing tests.** The host-task count and its release (the only path counted by hand, in `host_task_set.rs`); the three-record atomicity of `prepare_call`; refusal of a stream or future and of `thread.new-indirect` at the cap; and an end-to-end check that the harness's `wasmtime.set-max-table-capacity` really lowers the cap.

## Acceptance criteria
- [ ] A crate outside this workspace cannot change the record cap in any feature combination, or the review notes state why the chosen gate is acceptable.
- [ ] The full-table failure is a store- or resource-level structured cause with Wasmtime's message, and `api` records the move.
- [ ] Item 3 is done or the review notes say why not, and item 4's tests exist on both targets.
- [ ] `lint` passes and `tests all` is green on both targets, any directive that starts passing has its line removed from the expected-failure list, and every line whose reason this card changes is refreshed from a live run.

