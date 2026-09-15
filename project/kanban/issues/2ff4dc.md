---
id: 2ff4dc
title: Reject an import whose registered kind does not match
type: bug
blocked_by: []
labels: [parity, conformance]
created: 2026-09-15T16:37:10Z
---

## What to build
`wasmtime/import.wast:7` expects a link failure `expected instance found func` when a host registers a function under a name the component imports as an instance, and the polyfill links anyway. Resolution matches names but not item kinds. Add the kind check to import resolution for every kind the linker registers (function, instance, resource, and any added later), with a `LinkError` naming the expected and found kinds in Wasmtime's wording.

Parity: resolution is shared code; add a cross-target test.

## Acceptance criteria
- [ ] Registering a function where an instance is imported fails to link with `expected instance found func`, on both targets, and the reverse case fails the same way.
- [ ] The `wasmtime/import.wast:7` line is removed from `expected-failures.txt`, and the corpus is regenerated.
- [ ] `tests all` passes on both targets.

