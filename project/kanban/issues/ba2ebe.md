---
id: ba2ebe
title: "bindgen ownership: Borrowing, with borrowed derived types"
type: feature
blocked_by: [3200e7]
labels: [bindgen, bindgen-deferred]
created: 2026-10-16T23:39:03Z
---

## What to build
Support `ownership: Borrowing { duplicate_if_necessary }` in `wcmp::bindgen!`, as Wasmtime's macro does (`crates/wit-bindgen/src/lib.rs:110-127` at `v49.0.0-rc.1`). Types used as parameters of guest exports borrow deeply (`&str`, `&[T]`, records with lifetime parameters). With `duplicate_if_necessary: true`, a type used in both directions gets an owned and a borrowed definition.

PDD028 refuses this option today. It needs the PDD027 derive to accept lifetime parameters on records and variants (a `Lower`-only borrowed form). Saves a host-side clone on calls into exports.

## Acceptance criteria
- [ ] The derive accepts lifetime parameters on records and variants and lowers borrowed fields with no copy on the host side.
- [ ] `ownership: Borrowing { duplicate_if_necessary: false | true }` generates Wasmtime's shapes; the PDD028 ledger row moves to matched.
- [ ] The codegen corpus set with `Borrowing` compiles on both targets.
- [ ] `lint` passes and `tests all` is green.

