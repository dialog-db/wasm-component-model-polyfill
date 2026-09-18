---
id: 21accc
title: Close the task.return review's notes
type: chore
blocked_by: []
labels: [PDD019, concurrency]
created: 2026-09-18T18:20:51Z
---

## What to build
The review of card `741912` accepted `task.return` and left a list of small things it did not block on, in `src/executor/task_return.rs` at `4641730` unless said otherwise. First, the three return-mismatch kinds (result type, string encoding, memory) trap with one message, so a test can only prove that a mismatch fired, not which; give `TaskCause::ReturnMismatch` — or its rendering — a way to say which comparison failed without changing the Wasmtime-matched prefix the corpus matches by substring, and make the three mismatch tests assert the kind. Second, `outstanding_borrows` fabricates `ValueType::Bool` when the task has no result, copying `instance/func.rs:237`; give `AbiCause::OutstandingBorrows` an honest shape for the no-result case at both sites. Third, `TypeProjector::function` and `TypeProjector::result_tuple` in `src/component/project.rs` are literal duplicates; have `function` call `result_tuple`. Fourth, `BoundaryInstance::with_id` in `src/abi/instance.rs` overrides the id but leaves `resource_tables` from the declaring instance; the two coincide today because only the declaring instance's core modules can import the built-in, so either make `with_id` re-resolve the tables from the id or say in its doc why the tables stay. Fifth, the comment at `task_return.rs` around line 207 says "the translator interns a slot on the core export" — name `wasmtime-environ`'s translator so a reader does not look for it in `translate.rs`. Add the tests the review found missing: a `task.return` with no result against a task whose function has one and the reverse, the `result_tuple` refusal of more than one result, and options naming no memory with a result that needs one.

## Acceptance criteria
- [ ] The three mismatch tests each assert which comparison failed, and the corpus directives that match the mismatch message by substring still pass.
- [ ] No fabricated value type reaches `AbiCause::OutstandingBorrows`.
- [ ] The four missing cases have tests.
- [ ] `lint` passes and `tests all` is green on both targets with the conformance summary unchanged.

