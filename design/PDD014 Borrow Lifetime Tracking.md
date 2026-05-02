# Borrow Lifetime Tracking

The canonical ABI's runtime-state rules require that every
`borrow<T>` handle a host call hands across the boundary be
released before the call returns, and that any owning handle the
host *lent* as a borrow during the call be returned to its
pre-call state on exit. The polyfill's lift and lower paths
recognise the borrow shape — `Val::Borrow` round-trips through the
per-store handle table [PDD009] introduced — but no per-call state
records how many borrows the call has handed out, so a host that
fails to release one returns silently. The reserved
outstanding-borrows ABI cause [PDD008]'s error growth introduced on
`Error::Abi` exists for exactly this case but is never produced.
This PDD adds the per-call state, the increments and decrements,
and the scope-exit check that surfaces the failure, and (with the
same machinery) reverts the lend operations the canonical ABI
requires be undone on call return.

## Goals

- Every host call enters and exits a per-call scope. Scope entry
  is the trampoline's first action; scope exit is the last action
  on the success path before the trampoline writes results back to
  the substrate.
- Each `borrow<T>` *lower* into the guest inside a scope
  increments a per-scope borrow counter; each guest-side
  `resource.drop` of a borrow handle decrements it. A scope
  exiting with a non-zero counter surfaces the
  outstanding-borrows ABI cause with the residual count carried.
- Each `borrow<T>` *lift* from an owning handle records the
  lender so that scope exit can undo the lend, returning the
  owning handle to its pre-call state.
- The polyfill's existing imported-resource lift / lower paths
  pick up the scope hooks transparently — call sites that already
  drive lift and lower do not change.

## Non-goals

- Asynchronous calls. The scope model is synchronous; one
  trampoline call corresponds to one scope entry / exit pair.
- Cross-call borrow tracking. Borrows that escape one scope and
  are dropped in another are not modelled; per the canonical ABI
  the borrow's lifetime is the call's lifetime.
- Static borrow checking at the polyfill's typed surface. The
  enforcement here is dynamic; the typed surface remains
  unchanged.
- Locally-defined-resource borrow tracking. Local resources have
  their own identity model; the borrow accounting introduced here
  applies symmetrically once a local resource's `borrow<T>` lift
  reaches the same code path.

## The Per-Call Scope

This PDD revises [PDD008]'s host-trampoline contract. Where
[PDD008] specified the trampoline's body as a lift–dispatch–lower
sequence with no per-call state, this PDD widens the contract:
trampoline entry pushes a scope, trampoline exit (on the success
path) pops it after running a validation step.

The scope is a polyfill-owned shape held on a per-store stack of
call contexts. Its state mirrors the per-call shape [Wasmtime]'s
runtime maintains:

```text
struct CallContext {
    borrow_count: u32,
    lenders: Vec<HandleIndex>,
}
```

The four operations the canonical ABI's runtime-state rules name
update this state:

- `lower_borrow` (host hands a borrow to the guest) increments
  `borrow_count` and inserts the borrow into the guest-facing
  table for the current scope.
- `lift_borrow` (host receives a borrow lifted from an owning
  handle) leaves `borrow_count` unchanged but, when the source is
  a host-owned handle, pushes the owning handle's index onto
  `lenders` so the lend can be undone on scope exit.
- `resource.drop` of a borrow handle decrements `borrow_count`;
  the table entry is removed.
- Scope exit checks `borrow_count == 0`, walks `lenders` to
  reverse the per-lender table mutations, then pops the scope.

The drop trampoline distinguishes own-drop from borrow-drop by the
table entry's recorded kind. A borrow-drop that finds no live
scope (for example, a stale handle from a prior call that escaped
the call's lifetime) surfaces the invalid-handle ABI cause
[PDD009]'s error growth introduced on `Error::Abi`, with a
description of the mismatch.

## Scope Exit

Before the trampoline writes results back to the substrate on the
success path, the polyfill runs a validation step against the
current scope. The check mirrors [Wasmtime]'s `validate_scope_exit`
shape:

```text
fn validate_scope_exit(scope) -> Result<()>:
    if scope.borrow_count > 0:
        return Err(OutstandingBorrows { count: scope.borrow_count })
    for lender in scope.lenders.drain():
        table.undo_lend(lender)
    Ok(())
```

A non-zero borrow counter surfaces `Error::Abi` carrying the
outstanding-borrows ABI cause; the variant's existing `count`
payload carries the residual count unchanged. The lender list is
then walked: each recorded owning handle has its borrow mutation
reverted in the per-store table, restoring its pre-call state.

A trampoline whose host closure returns `Err(_)` propagates the
error without running the validation step. This matches
[Wasmtime]'s behaviour: the success-path lower-and-exit sequence
is the only call site for `validate_scope_exit`. The error path is
the caller's signal that something already failed; reporting a
borrow leak on top would compound diagnostics without adding
information. The scope is still popped on exit so the per-store
stack stays consistent for any subsequent host call.

## Error Model Growth

No new error variants are introduced. The outstanding-borrows
ABI cause [PDD008]'s error growth introduced on `Error::Abi` —
reserved at the time the canonical-ABI error enum was designed —
gains a producer. The variant's `count: u32` payload is the
residual count the validation step observes.

A validation step that fires inside a typed host function (the
typed registration entry [PDD008] introduced on `LinkerInstance`,
or the untyped sibling) surfaces the same `Error::Abi`; the host's
Rust-typed return is not consulted, since the failure happens
after the closure returns and before the trampoline writes
results.

## User Stories

**As a developer registering a host function that takes
`borrow<T>`**, I want the polyfill to refuse the call's return
when I forget to drop the borrow, so that the canonical-ABI
runtime-state rule is enforced for me rather than left as a
silent bug.

> The developer registers a host function that lowers
> `borrow<thing>` to the guest and intentionally never has the
> guest drop the handle. The guest's call hands the borrow into
> the host (incrementing the scope's `borrow_count`), runs the
> closure, and reaches the trampoline's validation step. The
> polyfill surfaces `Error::Abi` carrying the outstanding-borrows
> ABI cause; the count is `1`.

**As a contributor extending the resource subsystem**, I want
the lend / undo-lend bookkeeping to live behind the existing lift
and lower paths, so that future work on resource shapes does not
need to reach into trampoline internals.

> The contributor adds a new resource shape and threads it
> through the existing lift / lower paths. The polyfill's borrow
> tracking applies without further wiring; the new shape's
> tests inherit the validation guarantee.

## References

- [PDD000] — product overview.
- [PDD001] — development environment.
- [PDD002] — ecosystem foundation.
- [PDD003] — compatibility outlook. This PDD closes the borrow-
  lifetime row of "Resources".
- [PDD004] — test macros.
- [PDD005] — library foundations.
- [PDD007] — linking and instantiation.
- [PDD008] — canonical ABI and host functions; the PDD whose
  trampoline contract this PDD revises and whose `Error::Abi`
  outstanding-borrows cause this PDD's validation step produces.
- [PDD009] — resources; the PDD whose handle-table machinery the
  scope state shadows and whose invalid-handle ABI cause the
  drop trampoline may surface.
- [`wasm_runtime_layer`] — the runtime substrate.
- [Wasmtime] — the reference runtime, whose per-call `CallContext`
  shape and `validate_scope_exit` discipline this PDD mirrors.
- [Explainer] — the canonical Component Model design document.
- [Canonical ABI] — the runtime-state rules this PDD enforces at
  scope exit.

[PDD000]: ./PDD000%20Wasm%20Component%20Model%20Polyfill.md
[PDD001]: ./PDD001%20Development%20Environment.md
[PDD002]: ./PDD002%20Ecosystem%20Foundation.md
[PDD003]: ./PDD003%20Compatibility%20Outlook.md
[PDD004]: ./PDD004%20Test%20Macros.md
[PDD005]: ./PDD005%20Library%20Foundations.md
[PDD007]: ./PDD007%20Linking%20and%20Instantiation.md
[PDD008]: ./PDD008%20Canonical%20ABI%20and%20Host%20Functions.md
[PDD009]: ./PDD009%20Resources.md
[`wasm_runtime_layer`]: https://github.com/DouglasDwyer/wasm_runtime_layer
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
[Explainer]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md
[Canonical ABI]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md
