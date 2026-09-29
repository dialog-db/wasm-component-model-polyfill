---
id: edf2c7
title: A forgotten instantiation future cannot leave a stale store pointer in the browser backend
type: bug
blocked_by: [5d0ebe]
labels: [runtime-layer, PDD025]
created: 2026-09-29T10:01:11Z
---

## What to build

Finding F1 from the independent review of 5d0ebe (browser host functions, landed `4c4c05641`), in `rust/wcmp-wasm-core-web`.

The raw-pointer guard (`calls.rs:212-216`) is held across an `await` in instantiation (`store.rs:256-258`), so soundness depends on its `Drop` running. If safe code `mem::forget`s a pending instantiate future, the pointer stays set with no borrow behind it. The JavaScript event loop still runs that instantiation's start function, which then takes `&mut WebStore` from the stale pointer: undefined behaviour reachable from safe code. The SAFETY comment's claim that a guest runs only inside that call is also untrue across the `await`, because other tasks run in that window. A related edge: a start function from a dropped instantiation can run in another instantiation's `await` window, and its host error can replace that instantiation's own error (`store.rs:259-263`).

Restructure so a stale access cannot be undefined behaviour (for example, a pointer the store owns and clears on drop, checked through a generation or a weak handle), and keep each instantiation's host error its own.

## Acceptance criteria
- [ ] No safe use, including `mem::forget` of a pending future, leaves a pointer that a guest can dereference after its borrow ends.
- [ ] A start function from a dropped instantiation cannot replace another instantiation's error.
- [ ] The SAFETY comments state the invariant that actually holds.
- [ ] A web test forgets a pending instantiation whose start function calls a host function, and the result is a structured error or trap, not undefined behaviour.
- [ ] `tests all` and `lint` pass.

## Review notes

- 2026-09-29: card 5b4e90 (JSPI, landed `cf3cb439a`) replaced the raw-pointer guard with an `Rc`-owned store cell, an epoch-checked `Owner`, and permit-checked flights; its reviewer confirmed the stale-pointer hole described above is closed for `instantiate` and suspension. One residual remains: after `mem::forget` of a pending resume or deferred-instantiate future and release of the guard, a host function of that still-permitted flight that reaches its own `Store` through a global or a captured `Rc<RefCell<Store>>` makes a reference beside the live `&mut WebStore` (`wcmp-wasm-core-web/src/calls.rs:552`, via `Owner::store`/`store_mut`, `owner.rs:54,62`). Reviewer's suggested fix: a "flight inside a host call" flag in `Calls`, checked by `Owner::store`/`store_mut` (an error on fallible methods, a panic in `data`/`data_mut`). Retarget this card to that residual, and add the forget tests for both resume and instantiate.

## Dispatch log

- 2026-09-29: pulled into the PDD025 implement session at the owner's request. The card text above predates 5b4e90; the last review note is the current target (the forget-plus-global residual in the `Owner`/flight design, not the old raw-pointer guard). Card 10f282 (trap kinds) is in review and touches `store.rs` and `errors.rs` in the same crate.
- 2026-09-29: implementor `card-edf2c7-6d976fda` dispatched (JSPI landed as `cf3cb439a`, Wasmi suspension as `16c966ff9`).
- 2026-09-29: implementor reported done at `5427697cd` (a count of a flight's running host-function calls in `Calls`; `Calls::claim` returns `Error::Backend` while it is non-zero, so fallible `Owner` methods give a structured error and `data`/`data_mut` panic, documented; SAFETY comments rewritten; two web forget tests, one per path, that assert `Error::Backend`); `tests all` (web 1511/1511) and `lint` green. Implementor paused. Reviewer `review-edf2c7-511cb2b8` launched; branch delivered.
- 2026-09-29: reviewer `review-edf2c7-511cb2b8` **accepted** at `5427697cd` (`tests all` and `lint` green; traced a global, a captured `Rc<RefCell<Store>>`, `ExternRef`, re-entry, nested and forgotten flights, dropped futures and a store dropped inside a host call, and found no safe path to two live references; every SAFETY comment is true; both forget tests would fail without the fix). Finding: in the refused state the panic reaches most public operations, because `checks::same_store` and the constructors go through `store.data()`/`engine()`, which conflicts with PDD025's "never a panic" (filed as fa9643). Landed as `16b227b7a`. Reconciliation: `rust/wcmp-wasm-core-web/tests/web.rs` conflicted with 10f282's appended trap tests; the host kept both sides (HEAD's file plus this card's doc-header change and its 200 appended lines, applied as its own patch). Mechanical only, so no return to review; the paused implementor was resumed to run `tests web debug` and `lint` on the composed tip. Revert artifact: `sandbox-guest/card-edf2c7-6d976fda`.
- 2026-09-29: composed tip `16b227b7a` verified by the resumed implementor: `tests web debug` 1526/1526 (forget tests and 10f282's trap tests both run), `lint` green. Implementor removed.
