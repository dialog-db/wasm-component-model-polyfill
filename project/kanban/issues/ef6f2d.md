---
id: ef6f2d
title: A synthetic borrow converts to ResourceAny with no call in flight, as in Wasmtime 50
type: feature
blocked_by: []
labels: [crates-io, wasmtime-50]
created: 2026-10-20T19:11:12Z
---

## What to build

In Wasmtime 50, `Resource::new_borrow(..).try_into_resource_any` works outside
a call (`4ac4c8cbf`). The result needs no table entry, and `resource_drop` on
it does nothing. Lowering it into an `own` fails with "cannot lower a `borrow`
resource into an `own`". The polyfill refuses the conversion with
`NoCallInFlight` when no call is in flight
(`resource/resource_any.rs:208-214`).

## Acceptance criteria

- [ ] The conversion works with no call in flight, and `resource_drop` on the
      result does nothing.
- [ ] Lowering the result into an `own` fails with Wasmtime's message, and
      lowering it into a `borrow` passes the rep.

## Review notes

