---
id: dd5efd
title: The link rule refuses a mismatched registration kind
type: feature
blocked_by: [4305c8, 76d25c]
labels: [PDD020, concurrency]
created: 2026-09-19T06:40:54Z
---

## What to build
In `check_function_item` in `src/linker/resolve.rs`, enforce Wasmtime 49's rule on the registration kind: an async-typed import (`FunctionType.async_` true) satisfied by `func_new` or `func_wrap` fails to link, and a sync-typed import satisfied by `func_new_concurrent` or `func_wrap_concurrent` fails to link. `LinkError` in `src/error.rs` gains two causes, one per mismatch, each rendering Wasmtime's message for it; take the text from Wasmtime 49's `crates/wasmtime/src/runtime/component/func/host.rs` and `linker.rs` at `v49.0.0-rc.1`, and pin it in the tests. A synchronous host function would serve an async-typed import correctly, since it resolves at once, so the rule is Wasmtime's choice rather than the reference's; the polyfill follows it so that a host's registrations move between the two unchanged, and the doc on the two causes says so. Both concurrent entries register and link for an async-typed import, and both synchronous entries keep linking for a sync-typed one.

## Acceptance criteria
- [ ] An async-typed import satisfied by `func_wrap` fails to link with Wasmtime's message, proved by a repository test.
- [ ] A sync-typed import satisfied by `func_wrap_concurrent` fails to link with Wasmtime's other message, proved by a repository test.
- [ ] Both `func_wrap_concurrent` and `func_new_concurrent` register and link for an async-typed import, proved by repository tests.
- [ ] `lint` passes and `tests all` is green on both targets with the conformance summary unchanged.

## Dispatch log
- 2026-09-20T01:43:42Z dispatched implementor `card-dd5efd-aa08410f` (implement session, PDD020 thread, budget 3, seed `9d36b33` with 76d25c ready and 708ab2 landed but not yet tip-gated)

## Review notes
- 2026-09-20T00:40:16Z from the 76d25c review, for this card's implementor: (a) `linker/mod.rs:14-16`, `host_func_kind.rs:22-28`, `host_func.rs:51-55` already state this card's rule in the present tense — make them true rather than restating; (b) `function_type_for` hard-codes `async_: false`, so a typed concurrent registration's signature reads sync and `HostFuncKind` is the only record that it is concurrent — branch the rule on the kind, never on the registration's own `async_`; (c) the `Error::Unsupported` arm at `trampoline.rs:370` (sync lower of a concurrent registration) is untested and reachable only while the rule is absent — once the rule holds it becomes unreachable from a linked component, so decide whether a test or a doc note is the right closure.
