---
id: aabfc1
title: A concrete heap type knows its kind in the runtime layer type model
type: feature
blocked_by: [127919]
labels: [runtime-layer]
created: 2026-09-29T01:52:11Z
---

## What to build

In `wcmp-wasm-core`, `HeapType::Concrete(TypeHandle)` carries no hierarchy. So `Val::null` and `default_for_ty` on a concrete type always give a null function reference, even for a struct, array, continuation or exception type (`rust/wcmp-wasm-core/src/values/val.rs`), and the capability check for a concrete reference asks only for `function_references` (`src/checks.rs`). Wasmtime splits its concrete heap types by kind (`ConcreteFunc`, `ConcreteStruct`, `ConcreteArray`, `ConcreteCont`, `ConcreteExn`).

The PDD's type model leaves this open, so the owner decides the shape first: for example, a `TypeHandle` that reports its top type, or concrete variants by kind as in Wasmtime. Then make `Val::null`, `default_for_ty` and the capability check follow the kind (a concrete struct or array needs `gc`).

Found by the independent review of 127919 (rounds 1 and 2).

## Acceptance criteria
- [ ] The owner has chosen the shape, and the choice is recorded in the design document.
- [ ] A null or default value of a concrete struct, array, continuation or exception type has the matching reference kind.
- [ ] A concrete reference asks for the capability its kind needs.
- [ ] `tests/type_model.rs` covers each kind, and `tests all` and `lint` pass.

## Review notes

- 2026-09-29: from the review of 3164da: a `TypeHandle` carries no engine id, so a handle from another engine names a wrong type without an error (the Wasmtime backend's `type_registry.rs:315-318` scans linearly). Consider it with the shape chosen here.
