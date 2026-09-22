---
id: c41ebb
title: The public-api check sees derived impls, caches its listing, and its last internal item is settled
type: chore
blocked_by: []
labels: [api]
created: 2026-09-22T17:00:12Z
---

## What to build
The three review rounds of card `49c524` accepted the closed public surface and its snapshot check, and left five notes. First, `flake.nix:307` passes `-sss` to cargo-public-api, which drops auto-derived impls, so removing a `#[derive(Clone)]` from a public type would be a breaking API change with an empty diff: `#[derive(Clone)]` on `Component` (`src/component/component_interface.rs:80`) produces no line in the snapshot while the hand-written `impl Clone for HostResource` (`src/linker/host_resource.rs:128`) and for `Accessor` (`src/concurrency/accessor.rs:94`) do. Either move to `-ss` and accept the larger snapshot, or soften the comment at `flake.nix:300-302`, which calls derived impls "not a decision anyone makes". Second, `publicApiSource` (`flake.nix:277-283`) includes all of `rust`, which includes `public-api.txt` itself, so every `api update` invalidates a listing derivation that never reads it; excluding the snapshot from the filter keeps the listing cached. Third, the nightly pin (`flake.nix:271`) and `pkgs.cargo-public-api` (`:307`) must move together: on a rustdoc-JSON `format_version` mismatch the derivation fails and `lint` goes red with a message unrelated to the contributor's change. Both are lock-pinned, so this follows only a deliberate `flake.lock` bump — acceptable degradation, but the coupling is undocumented; one line in the comment block saves the next person. Fourth, the motivation at `src/component/component_interface.rs:34-40` says the private field stops "a component whose declared interface belongs to one binary and whose plan belongs to another"; the struct literal is indeed inexpressible, but the same mismatch stays reachable by assigning to the still-public `imports`/`exports` on an owned `Component`. The consequence is bounded — `registration_and_item` bounds-checks (`src/executor/instantiate.rs:653-656`) and `resolve_imports` zips bindings to imports (`src/linker/resolve.rs:163-213`), so the worst outcome is `Error::Internal` or a wrong-but-safe link, never a panic — but the sentence implies more than the compile-fail block proves. Fifth, `ResourceType::indexed` (`src/types/resource_type.rs:37`) is the one item left on the surface that reads workspace-internal: its `index` is a component resource-table position, its only callers are `src/component/project.rs:106` and `:367`, and a host has no way to know a right value. It reaches no internal type, equality ignores the index (`resource_type.rs:56-58`), and both consumers bounds-check (`src/abi/lift.rs:431-441`, `src/abi/lower.rs:337-347`, with `lower.rs:348` also comparing `type_id`), so a forged index yields `AbiError::InvalidHandle` and never a cross-type handle — decide whether it stays.

## Acceptance criteria
- [ ] Removing a derived trait implementation from a public type changes the snapshot, or the comment states that derived impls are outside what the check sees.
- [ ] `api update` does not invalidate the listing derivation.
- [ ] The comment block says the nightly pin and cargo-public-api move together and what a mismatch looks like.
- [ ] The private-field motivation states what the block proves, naming what stays reachable through `imports`/`exports` and why it is bounded.
- [ ] `ResourceType::indexed` is behind the internal seam or its public presence is justified in its doc.
- [ ] `lint` passes (the `public-api` check included) and `tests all` is green on both targets with the conformance summary unchanged.

