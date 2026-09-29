---
id: edf2c7
title: A forgotten instantiation future cannot leave a stale store pointer in the browser backend
type: bug
blocked_by: [5d0ebe]
labels: [runtime-layer]
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
