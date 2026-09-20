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
- 2026-09-20T04:02:53Z the session paused on a usage limit at ~02:56Z and every running VM stopped with it; resumed implementor `card-dd5efd-aa08410f` (branch at `767984e`, gates unfinished) with a self-contained prompt.
- 2026-09-20T05:13:26Z implementor reported blocked: on resume the harness rebased the branch onto the fresher base `2bed59c` (local tip `1bf914e`, origin still `767984e` on `9d36b33`; the three commits patch-identical by `git patch-id`), and the gated tip cannot fast-forward. Work complete and gate green on `1bf914e` (`lint` 9/9, `tests all` native 629 / web 617, conformance unchanged). Orchestrator authorized `git push --force-with-lease` since no reviewer has read the branch and nothing is lost.
- 2026-09-20T05:16:44Z implementor reported done at `1bf914e` (three commits on seed `9bb3950`: `check_registration_kind` runs before the signature comparison and branches on `HostFuncKind` only; two `LinkError` causes rendering Wasmtime's `typecheck_async` messages verbatim, documented as Wasmtime's choice; seven tests in `baseline_async_import.rs` incl. wrong-`async_`-flag, interface-item naming, and the `trampoline.rs:370` arm reached from a linked component; conformance harness registers `host-echo-u32` through the concurrent entry, README prose updated; `lint` 9/9, `tests all` native 629 / web 617, conformance unchanged). No overlap with the 1b6304 landing. Fetched, moved to needs-review, paused the implementor.
- 2026-09-20T05:16:44Z launched reviewer `review-dd5efd-f94a018d`; delivered `sandbox-guest/card-dd5efd-aa08410f` (tip 1bf914e) as `delivered/card-dd5efd-aa08410f`.

## Review notes
- 2026-09-20T00:40:16Z from the 76d25c review, for this card's implementor: (a) `linker/mod.rs:14-16`, `host_func_kind.rs:22-28`, `host_func.rs:51-55` already state this card's rule in the present tense — make them true rather than restating; (b) `function_type_for` hard-codes `async_: false`, so a typed concurrent registration's signature reads sync and `HostFuncKind` is the only record that it is concurrent — branch the rule on the kind, never on the registration's own `async_`; (c) the `Error::Unsupported` arm at `trampoline.rs:370` (sync lower of a concurrent registration) is untested and reachable only while the rule is absent — once the rule holds it becomes unreachable from a linked component, so decide whether a test or a doc note is the right closure.
