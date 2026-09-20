---
id: dacf25
title: An unresolved import inside an instance names the missing item
type: chore
blocked_by: []
labels: [PDD020, concurrency]
created: 2026-09-20T15:40:21Z
---

## What to build
`LinkError::UnresolvedImport` (`src/error.rs` around line 224) carries no item, so a missing function inside an instance import renders `no registered linker instance satisfies import <name>` without saying which item was missing, although the four construction sites in `src/linker/resolve.rs` (around lines 446, 471, 508, 535) have the item in hand. The sibling causes now render `instance export <item> has the wrong type` between the import context and the cause; Wasmtime names the missing item there too. Add an `item: Option<String>` to `UnresolvedImport`, fill it at every site that knows it, render it through the same `ItemContext` clause when present, keep the item-less rendering byte-identical, and pin both renderings in tests. Also tighten the `error.rs` unit test so both registration causes are pinned with a full-string equality with and without an item.

## Acceptance criteria
- [ ] An unresolved item inside an instance import renders the item name, proved by a test that pins the full string; the root-level rendering is unchanged, proved by a test.
- [ ] Both registration-kind causes are pinned with a full string with and without an item.
- [ ] `lint` passes and `tests all` is green on both targets with the conformance summary unchanged.


