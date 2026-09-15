---
id: ec6ed9
title: Per-instance resource tables and handle transfer between inner components
type: bug
blocked_by: [852821]
labels: [PDD015, PDD012, wave-2]
created: 2026-09-15T07:41:01Z
---

## What to build
The polyfill keeps one handle table per resource *type* (the translator's resource index), shared by every component instance that uses the type. The Canonical ABI keeps one table per component instance per resource (Wasmtime's `TypeResourceTableIndex`), and the adapter's `resource.transfer-own` / `resource.transfer-borrow` intrinsics move an entry from the caller's table to the callee's. The polyfill's transfer intrinsics are identity maps over the index, which is wrong as soon as two inner instances hold different entries at the same index. `cm/linking/unit.wast:931-1023` shows it: a user component that calls `make`, `get`, and `drop` on a resource defined by another inner instance reads rep 0 and counts one drop too few. Model tables per (component instance, resource) as the translator does, resolve the transfer intrinsics to a real move between the source and destination tables, and keep the host-facing handle identity per resource type.

## Acceptance criteria
- [ ] `cm/linking/unit.wast:931-1023` pass and leave the expectation list.
- [ ] A test proves an `own<T>` returned by one inner component and dropped by another runs the first component's destructor exactly once (the PDD015 story).
- [ ] The smoke test's composition step still passes on both targets.

