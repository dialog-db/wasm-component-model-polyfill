---
id: dacf25
title: An unresolved import inside an instance names the missing item
type: chore
blocked_by: [d6887f]
labels: [PDD020, concurrency]
created: 2026-09-20T15:40:21Z
---

## What to build
`LinkError::UnresolvedImport` (`src/error.rs` around line 224) carries no item, so a missing function inside an instance import renders `no registered linker instance satisfies import <name>` without saying which item was missing, although the four construction sites in `src/linker/resolve.rs` (around lines 446, 471, 508, 535) have the item in hand. The sibling causes now render `instance export <item> has the wrong type` between the import context and the cause; Wasmtime names the missing item there too. Add an `item: Option<String>` to `UnresolvedImport`, fill it at every site that knows it, render it through the same `ItemContext` clause when present, keep the item-less rendering byte-identical, and pin both renderings in tests. Also tighten the `error.rs` unit test so both registration causes are pinned with a full-string equality with and without an item.

## Acceptance criteria
- [ ] An unresolved item inside an instance import renders the item name, proved by a test that pins the full string; the root-level rendering is unchanged, proved by a test.
- [ ] Both registration-kind causes are pinned with a full string with and without an item.
- [ ] `lint` passes and `tests all` is green on both targets with the conformance summary unchanged.

## Dispatch log
- 2026-09-20T23:48:26Z dispatched implementor `card-dacf25-2bde1c97` (implement session, PDD020 thread, budget 3, seed `b488264` with 639a90, 4587d2, 5345f4, 7adde9 landed pending gate)
- 2026-09-21T01:18:31Z implementor reported done at `aefa90b` (change `484ce26` plus a reflow on seed `945c55b`; `UnresolvedImport` gains `item: Option<String>` rendered through a new `UnresolvedContext` helper — item-less byte-identical; filled at four `resolve.rs` sites and three `instantiate.rs` lookups via `item_of(path)`; unit test pins none / `f` / `inner.f`, end-to-end tests pin the nested and root strings; the registration causes' `starts_with` assertions replaced with full-string equality; `lint` all checks, `tests all` native 685 / web 675, conformance unchanged). Note: `KindMismatch`'s item is path-qualified for function items but bare for resource/module/nested-instance items, and `UnresolvedImport` inherits that split (pre-existing). No overlap with other landings. Fetched, moved to needs-review, paused the implementor.
- 2026-09-21T01:18:31Z launched reviewer `review-dacf25-7089fc72`; delivered `sandbox-guest/card-dacf25-2bde1c97` (tip aefa90b) as `delivered/card-dacf25-2bde1c97`.

## Review notes
- 2026-09-21T02:35:36Z reviewer `review-dacf25-7089fc72` on tip `aefa90b`: **accept**, none blocking. Gates in the reviewer's VM: `lint` 14/14, `tests all` four lanes exit 0 (web release observed 675/675; the native summary scrolled out of the reviewer's capture), conformance cell-for-cell with the README from `summary.json`. Rendered shape confirmed: Wasmtime applies `instance export <item> has the wrong type` unconditionally around `definition()`, whose leaf for a missing item is `function implementation is missing`; for a missing nested instance Wasmtime recurses (`a -> b -> f`) where the polyfill names only `b`. Strongest finding (pre-existing, newly visible, follow-up): with an item the leaf clause `no registered linker instance satisfies import a` is false — an instance did satisfy `a` — and names `a` twice; Wasmtime's leaf is `function implementation is missing`. Others: (1) `instantiate.rs:646` parent-walk hardcodes `item: None` though the segment is in hand (unreachable today); (2) `ItemPosition::item()` returns `None` when a function item's name coincides with its plain-named enclosing import; (3) the qualification split is more visible (three more bare-item messages) but no new test enshrines the bare form; test gap: the headline missing-function site (`resolve.rs:599`) has only a synthetic unit test — `IMPORTS_ITEM` at `baseline_linking.rs:1802` would reach it end to end; five updated matches relaxed to `..`. Public surface: `#[non_exhaustive]` is on the enum, not the variant, so the added field breaks a downstream `UnresolvedImport { import }` pattern (consistent with the sibling variants; crate 0.1.0).
- 2026-09-21T02:35:36Z landed as `f2f3eb6` (clean squash; four files byte-identical to the branch tip; `executor/instantiate.rs` auto-merged with the 3e657c landing, delta verified). Landing commit made with `jj commit`. Card stays needs-review until the composed tip is gated. Pair removed.
- 2026-09-21T10:05:02Z composed-tip gate at `d07b6b6` (sandbox `verify-tip-303180c4`, post-GC): **green** — four lanes (native 711, web 701), `lint` all checks, conformance identical to the README on both targets including category counts and the composed browser paragraph (native 1694, browser 1685). Moved to ready.
