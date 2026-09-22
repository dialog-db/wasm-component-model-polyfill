---
id: dced20
title: A host borrow lifted out of a guest lowers back as Wasmtime does, and a reused index is refused
type: chore
blocked_by: []
labels: [resource]
created: 2026-09-22T06:53:00Z
---

## What to build
The two review rounds of card `dda886` accepted the validated and lent host borrow and left findings that need one card. First, ABA reuse: `Store::resource_drop` frees the slot and the free list hands the index back (`src/resource/table.rs:88-98`), so a stale `ResourceHandle` whose slot was re-minted passes the lookup; the `rep` cross-check at `src/abi/lower.rs:397-405` catches it unless the new resource has the same rep. Wasmtime blocks this with a generation in `HostResourceIndex` (`crates/wasmtime/src/runtime/component/resources/host_tables.rs:159-166`, `validate_host_index`, `new_host_index`). Decide whether the host table carries a generation. Second, a `Val::Borrow` the host lifted out of a guest carries the guest table's index; the polyfill cannot tell it from a host-minted handle and refuses to re-lower it, where Wasmtime's `ResourceState::Borrow` passes the rep through. The reviewer's shape with no public-surface change: at lift time put the host-side borrow in the host table as a `HandleKind::Borrow` entry (Wasmtime `host_resource_lower_borrow`, `host_tables.rs:152-157`) and let the lower pass a `Borrow` entry's rep through with no lend, which also makes the branch's `Borrow`-entry arm at `lower.rs:366-376` reachable. Third, two wording notes: the inline comment at `lower.rs:398-405` says the reps disagree "exactly when the index came from somewhere else", which two host entries holding the same rep disprove — say it is a narrowing check; and the rep-mismatch refusal reuses `HandleLookupError::Unknown`, so the message reads "handle index 1 is not live in the host's resource table" for the one case the check exists to catch, where the index is live with another resource — give it its own reason.

## Acceptance criteria
- [ ] A stale `ResourceHandle` whose slot was re-minted with the same rep is refused, proved by a test; or the design states why the polyfill accepts that window.
- [ ] A borrow the host lifted out of a guest can be lowered back into a guest and hands the guest the rep, with no lend, matching Wasmtime; a test proves it on both targets and the guest-index collision test of `dda886` still passes.
- [ ] The rep-mismatch refusal carries its own reason naming the mismatch, pinned by a test, and the inline comment states the check's limit.
- [ ] `lint` passes and `tests all` is green on both targets with the conformance summary unchanged.

