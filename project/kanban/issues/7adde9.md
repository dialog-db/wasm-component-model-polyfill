---
id: 7adde9
title: Cover the other sorts an interface-named import can carry
type: chore
blocked_by: []
labels: [PDD020, concurrency]
created: 2026-09-20T13:32:31Z
---

## What to build
The root re-routing of non-instance interface-named imports (`resolve_imports` in `src/linker/resolve.rs`) covers function, resource, module, type, component, and value sorts, but only the function shape is tested. Add tests for a resource import and a module import under an interface name (positive and unresolved each), and for a type import under an interface name failing with `UnsupportedRegistration`. While there: the comment at `resolve.rs:404-408` misdescribes the live `(Interface, _)` arm and the `(Plain, _)` arm around `:431` is dead — fix the comment and remove the dead arm; and `TypeMismatchPosition::HostFunctionRegistrationPlain` in `src/error.rs` (around line 459) now carries interface names, so either rename it (a public, `non_exhaustive` enum) or state in its doc that the name is historical.

## Acceptance criteria
- [ ] Tests prove a resource and a module import under an interface name resolve through the root and fail with `UnresolvedImport` when nothing is registered, and a type import under an interface name fails with `UnsupportedRegistration`.
- [ ] The dead `(Plain, _)` arm is gone and the dispatch comment describes the live arms.
- [ ] `lint` passes and `tests all` is green on both targets with the conformance summary unchanged.


