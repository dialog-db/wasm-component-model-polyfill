---
id: ae5bc0
title: Close the task.return notes review's follow-ups
type: chore
blocked_by: []
labels: [PDD019, concurrency]
created: 2026-09-19T01:14:53Z
---

## What to build
The review of card `21accc` accepted the change and left follow-ups, all in the tree at `9403209`. First, the optional `AbiError.valtype` has no test in the `None` direction: `it_traps_when_the_task_still_owes_a_borrow` in `src/executor/task_return.rs` (around line 727) builds its task with `Some(u32)`, and `src/error.rs` has no `AbiError` rendering test, so `type_label`'s empty arm is unproved. Flip that test to a task with no result and add one `AbiError` row without a valtype to the rendering table in `error.rs`. Second, four sites still fabricate `ValueType::Primitive(PrimitiveType::Bool)` where the new `AbiError` doc says a failure that processes no value type carries none: `src/instance/func.rs:134` (argument-count mismatch), `src/abi/context.rs:339` (post-return substrate failure), `src/linker/component_value.rs:277`, `:309`, `:317` (host value mismatch, missing result, unexpected result — `value_mismatch` even discards the value it is handed), and, landed after that review, `src/executor/callback_task.rs` `outstanding_borrows` (`Bool`) and `src/store/store_context.rs` `waitable_set_at` (`U32` for a status-word decode failure). Pass `None` at each, and make any test that matched on the fabricated type match on the cause. Third, the comment on `it_fails_the_lift_when_options_that_name_no_memory_carry_a_result_that_needs_one` in `task_return.rs` reads as if Wasmtime were matched; say plainly that the reference traps at the memory comparison, Wasmtime lifts through the task's own memory and succeeds, and the polyfill faults out of bounds, and that the shape is validation-illegal so nothing observes the difference. Fourth, `src/abi/instance.rs:41` and `:113` still say "every resource table of the component instance" where the `with_id` doc correctly says the tables are the instantiation's; align them. Fifth, reflow `task_return.rs:233` to the file's width.

For the owner, recorded here rather than fixed: `AbiError` is a public struct with public fields and no `#[non_exhaustive]`, so the valtype change breaks downstream constructors and readers, and `TaskCause::ReturnMismatch` going from a unit to a struct variant breaks matchers; the crate is 0.1.0. PDD019's Error Model Growth section says each cause's message is the trap Wasmtime prints; the mismatch message is now the trap plus a suffix naming the kind, and `ReturnMismatchKind` is not in the corpus.

## Acceptance criteria
- [ ] An `AbiError` with no valtype renders without a type label, proved by a test, and the outstanding-borrows test exercises the no-result task.
- [ ] No site constructs an `AbiError` with a fabricated value type.
- [ ] The three doc comments named above say what is true at the tip.
- [ ] `lint` passes and `tests all` is green on both targets with the conformance summary unchanged.


## Dispatch log
- 2026-09-19T01:51:48Z dispatched implementor `card-ae5bc0-c38e7aaf` (implement session, PDD019 thread, budget 3; seed at 562fe7f)
- 2026-09-19T04:05:07Z implementor reported done at `25557a2` (one commit; no-valtype rendering test, outstanding-borrows test on a no-result task, `None` at all seven fabricated sites, three doc comments; `tests all` green (native 552, web 539), conformance unchanged, lint green). Owner notes: three sites still carry the placeholder `ValueType::Own(ResourceType::new("resource"))` (`store_data.rs:181`, `host_call.rs:102`, `store_context.rs:201`); `value_mismatch` now ignores its `&Val` argument. Fetched, moved to needs-review, paused the implementor.
- 2026-09-19T04:05:51Z launched reviewer `review-ae5bc0-ab56a23b`; delivered `sandbox-guest/card-ae5bc0-c38e7aaf` (tip 25557a2) as `delivered/card-ae5bc0-c38e7aaf`.

## Review notes
- 2026-09-18 reviewer `review-ae5bc0-ab56a23b` on tip `25557a2`: **accept** (follow-ups, none blocking). Gates in the reviewer's VM: `tests all` four lanes green (native 552, web 539), `lint` 9/9, `tests conformance` at the tip and the seed byte-identical (2392/1574/65.8, 0 unexpected / 0 stale). All five items delivered and verified against source (the three-behaviour comment checked against `definitions.py:2278,583` and Wasmtime `concurrent.rs:3533-3547`). Findings: (A) AC 2 as worded is not met — three sites still construct `ValueType::Own(ResourceType::new("resource"))` (`store_context.rs:201`, a substrate failure on a `u32` rep like the `context.rs:339` site the card listed; `host_call.rs:102`, an unregistered type, so naming one is self-contradictory; `store_data.rs:181`, which does process an own handle but should use the handle's real type via `handle.type_id`); the card's enumerated list is done, so scope boundary, not defect. (B) `AbiError`'s doc says the outstanding-borrows rule carries no valtype, yet `task_return.rs:278` and `func.rs:421` still pass the result type; the `None` flip also leaves the `Some` branch at :278 uncovered — owner to decide which gives way. (C) `abi/instance.rs:8` and `:49` still say "instance" where `:40` and `:113` now say "instantiation". (D) `value_mismatch(_val)` was already vestigial at the seed; every call site has `Self::value_type()` in scope, so the honest label was one argument away. No message loses information.
- 2026-09-19T05:08:55Z landed as `e2aef28` (clean squash, 8 files); reviewer paused; card stays needs-review until the composed-tip gate returns.
- 2026-09-19T05:28:00Z host gate on the composed tip `445bb7c` green (`tests all` four lanes, `lint` 9/9, native 2392/1574/65.8, 0 unexpected / 0 stale); card to ready; removed `card-ae5bc0-c38e7aaf` and `review-ae5bc0-ab56a23b`. Revert artifact: `sandbox-guest/card-ae5bc0-c38e7aaf`.
