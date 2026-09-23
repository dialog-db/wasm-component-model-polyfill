---
id: 7adde9
title: Cover the other sorts an interface-named import can carry
type: chore
blocked_by: [c7c8a5]
labels: [PDD020, concurrency]
created: 2026-09-20T13:32:31Z
---

## What to build
The root re-routing of non-instance interface-named imports (`resolve_imports` in `src/linker/resolve.rs`) covers function, resource, module, type, component, and value sorts, but only the function shape is tested. Add tests for a resource import and a module import under an interface name (positive and unresolved each), and for a type import under an interface name failing with `UnsupportedRegistration`. While there: the comment at `resolve.rs:404-408` misdescribes the live `(Interface, _)` arm and the `(Plain, _)` arm around `:431` is dead — fix the comment and remove the dead arm; and `TypeMismatchPosition::HostFunctionRegistrationPlain` in `src/error.rs` (around line 459) now carries interface names, so either rename it (a public, `non_exhaustive` enum) or state in its doc that the name is historical.

## Acceptance criteria
- [ ] Tests prove a resource and a module import under an interface name resolve through the root and fail with `UnresolvedImport` when nothing is registered, and a type import under an interface name fails with `UnsupportedRegistration`.
- [ ] The dead `(Plain, _)` arm is gone and the dispatch comment describes the live arms.
- [ ] `lint` passes and `tests all` is green on both targets with the conformance summary unchanged.

## Dispatch log
- 2026-09-20T20:43:05Z dispatched implementor `card-7adde9-2f3112d2` (implement session, PDD020 thread, budget 3, seed `ba84743` with 639a90, 4587d2, 5345f4 landed pending gate)
- 2026-09-20T22:34:37Z implementor reported done at `b83affa` (refactor `b6853fb`, doc `e9ab51f`, tests `b83affa` on seed `2433b7d`; resource and module tests positive and negative; **the type-import criterion cannot be met as written** — `wasmtime-environ` erases non-resource type imports before translation (`inline.rs:110-114`) and bails on component/value imports, so the `UnsupportedRegistration` arm is defensive; an end-to-end erasure test and a `resolve_root` unit test with hand-built imports delivered instead; dead `(Plain, _)` arm removed, `resolve_one` takes destructured arguments; `HostFunctionRegistrationPlain` keeps its name with a historical note; `lint` green, `tests all` native 684 / web 674, conformance unchanged). No overlap with other landings. Fetched, moved to needs-review, paused the implementor.
- 2026-09-20T22:34:37Z launched reviewer `review-7adde9-01991eea`; delivered `sandbox-guest/card-7adde9-2f3112d2` (tip b83affa) as `delivered/card-7adde9-2f3112d2`.

## Review notes
- 2026-09-20T23:47:41Z reviewer `review-7adde9-01991eea` on tip `b83affa`: **accept**, no blocking findings. Gates in the reviewer's VM: `lint` 14/14, `tests all` native 684 / web 674 both profiles, conformance byte-identical to the README on both targets. Type-import claim holds, verified against the pinned `wasmtime-environ 49.0.0-rc.1` source (`inline.rs:111-116` drops interface-typed imports, `:1906` bails on root component imports, `types_builder.rs:285` on values) and, independently, `ExternType` has no variant for a non-resource type import at all, so the criterion is inexpressible by any route; the `_` arm is required for exhaustiveness. `resolve_one` refactor verified case by case (one call site; all four outcomes reproduced incl. vacuous-with-out-of-range candidate). Tests high quality (destructor observes rep 7; host module's `f` returns 101). Non-blocking: (1) `baseline_linking.rs:2313-2314` claims the unit test presents the arm a type, but it presents Component and Value — one sentence to fix; the reason string names a sort that can never arrive (pre-existing); (2) the same-identifier-linker-instance halves assert `UnresolvedImport` without re-pinning the name (matches the pre-existing function test); `UnresolvedImport` is the right cause under PDD011's two namespaces; (3) `HostFunctionRegistrationPlain`'s `name` field doc still says "as the component writes it" while nested items format `{name}.{qualified}` (pre-existing).
- 2026-09-20T23:47:41Z landed as `6068e3e` (clean squash; three files byte-identical to the branch tip). Landing commit made with `jj commit`. Card stays needs-review until the composed tip is gated. Pair removed.
- 2026-09-21T01:16:01Z composed-tip gate at `190aae8` (sandbox `verify-tip-6795c82b`): **green** — four lanes (native 687, web 677), `lint` all checks, conformance identical to the README on both targets including category counts (native 1595, web 1585). Moved to ready.
