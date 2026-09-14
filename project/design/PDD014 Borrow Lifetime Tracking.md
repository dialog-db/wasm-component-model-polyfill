# Borrow Lifetime Tracking

The runtime-state rules of the Canonical ABI require that every `borrow<T>`
handle a host call hands across the boundary is released before the call
returns. They also require that an owning handle the host lent as a borrow
during the call returns to its pre-call state on exit. This PDD adds the
per-call state that records those facts, the increments and decrements, and the
scope-exit check that surfaces a leaked borrow. With the same machinery it
reverts the lends that the Canonical ABI requires be undone on return.

## Goals

- Every host call enters and exits a per-call scope. Scope entry is the
  trampoline's first action. Scope exit is the last action on the success path
  before the trampoline writes results back.
- Each `borrow<T>` lowered into the guest inside a scope increments a per-scope
  borrow counter. Each guest-side `resource.drop` of a borrow handle decrements
  it. A scope that exits with a non-zero counter surfaces the
  outstanding-borrows ABI cause with the residual count.
- Each `borrow<T>` lifted from an owning handle records the lender, so that
  scope exit can undo the lend.
- The lift and lower paths pick up the scope hooks transparently. Call sites
  that drive lift and lower do not change.

## Non-goals

- Asynchronous calls. One trampoline call is one scope.
- Cross-call borrow tracking. A borrow's lifetime is the call's lifetime.
- Static borrow checking at the typed surface. Enforcement is dynamic.
- Locally-defined resource borrow tracking beyond what the shared lift and lower
  paths give.

## The Per-Call Scope

This PDD revises the trampoline contract of [PDD008]. Where [PDD008] specified a
lift, dispatch, lower sequence with no per-call state, this PDD pushes a scope
at entry and pops it at exit after a validation step.

The scope is a polyfill-owned shape on a per-store stack of call contexts. Its
state mirrors the per-call shape [Wasmtime] maintains:

```text
struct CallContext {
    borrow_count: u32,
    lenders: Vec<HandleIndex>,
}
```

Four operations update the state:

- `lower_borrow` (the host hands a borrow to the guest) increments
  `borrow_count` and inserts the borrow into the guest-facing table for the
  current scope.
- `lift_borrow` (the host receives a borrow lifted from an owning handle) leaves
  `borrow_count` unchanged. When the source is a host-owned handle, it pushes
  the owning handle's index onto `lenders`.
- `resource.drop` of a borrow handle decrements `borrow_count` and removes the
  table entry.
- Scope exit makes sure that `borrow_count` is zero, walks `lenders` to reverse
  each lend, and pops the scope.

The drop trampoline distinguishes an own-drop from a borrow-drop by the kind
recorded on the table entry. A borrow-drop that finds no live scope, for example
a stale handle from an earlier call, surfaces the invalid-handle ABI cause of
[PDD009].

## Scope Exit

Before the trampoline writes results back on the success path, the polyfill
validates the current scope. The check mirrors Wasmtime's `validate_scope_exit`:

```text
fn validate_scope_exit(scope) -> Result<()>:
    if scope.borrow_count > 0:
        return Err(OutstandingBorrows { count: scope.borrow_count })
    for lender in scope.lenders.drain():
        table.undo_lend(lender)
    Ok(())
```

A non-zero counter surfaces `Error::Abi` with the outstanding-borrows cause and
the residual count. The lender list is then walked, and each recorded owning
handle returns to its pre-call state.

A trampoline whose host closure returns `Err(_)` propagates the error without
the validation step. This matches Wasmtime. The error path already signals a
failure, and a borrow-leak report on top adds no information. The scope is still
popped, so the per-store stack stays consistent for the next host call.

## Error Model Growth

No new variants are introduced. The outstanding-borrows ABI cause on
`Error::Abi` carries the residual count. A validation failure inside a typed
host function surfaces the same `Error::Abi`. The host's typed return is not
consulted, because the failure happens after the closure returns.

## User Stories

A developer registers a host function that takes `borrow<T>` and wants the
polyfill to refuse the call's return when a borrow leaks.

> The guest hands a borrow into the host, which increments the scope's counter.
> The closure runs. The guest never drops the handle. The validation step
> surfaces `Error::Abi` with the outstanding-borrows cause and a count of one.

A contributor extends the resource subsystem and wants the bookkeeping to live
behind the lift and lower paths.

> The contributor adds a new resource shape and threads it through the existing
> paths. Borrow tracking applies without further wiring.

## References

- [PDD000], the product overview.
- [PDD003], the compatibility outlook.
- [PDD005], the foundations and posture.
- [PDD007], linking and instantiation.
- [PDD008], the Canonical ABI and host functions. This PDD revises its
  trampoline contract and produces its outstanding-borrows cause.
- [PDD009], resources, whose handle table the scope state shadows.
- [Wasmtime], whose per-call `CallContext` and `validate_scope_exit` this PDD
  mirrors.
- [CanonicalABI], the runtime-state rules this PDD enforces.

[PDD000]: ./PDD000%20Wasm%20Component%20Model%20Polyfill.md
[PDD003]: ./PDD003%20Compatibility%20Outlook.md
[PDD005]: ./PDD005%20Library%20Foundations.md
[PDD007]: ./PDD007%20Linking%20and%20Instantiation.md
[PDD008]: ./PDD008%20Canonical%20ABI%20and%20Host%20Functions.md
[PDD009]: ./PDD009%20Resources.md
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
[CanonicalABI]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md
