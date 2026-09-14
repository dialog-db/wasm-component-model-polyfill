# Host Registration: Root Namespace and Store Context

[PDD008] introduced host registration against interface-typed imports. A host
attaches functions and resources to a view onto one interface's registration
entry, and the linker resolves interface-named imports against those entries.
Two shapes that WIT-conformant components produce fall outside that surface. The
first is a component that imports a plain-named function at the root of its
import list. The second is a host function that mints a fresh resource handle
inside a guest call. This PDD revises [PDD008]'s registration shape on both
fronts. It adds a root namespace that is addressed without an interface
identifier, and it widens the host function context so that a registered closure
can read and mutate the per-store handle tables of [PDD009].

## Goals

- `Linker` exposes a root-namespace registration entry point that returns the
  same registration view as the interface entry point. Plain-named imports
  resolve through it.
- A registered host function can mint a fresh `own<T>` handle in the per-store
  table. The handle is live when the guest reads it.
- The typed and untyped registration entry points pass the same host call
  context. Neither is more capable than the other.
- The link-time and call-time error variants of [PDD007], [PDD008], and [PDD009]
  cover the new failure modes without parallel hierarchies.

## Non-goals

- Asynchronous host functions and asynchronous destructors.
- A host binding code generator. The registration surface is what a hand-written
  host calls. A generator consumes the same surface.
- Borrow lifetime tracking inside a host call.
- Registrations keyed by Rust type identity. Reps stay `u32` and opaque to the
  polyfill.

## The Root Namespace

`Linker` gains a root accessor. It returns a registration view whose entries are
addressed by the root namespace rather than by an interface identifier. Calling
the accessor twice returns a view onto the same entry, as [PDD007]'s interface
accessor does. The return type is the existing registration view. There is no
parallel root-only view type, because the registration operations are the same
wherever the items resolve.

The resolver's plain-named branch consults the root registration. A plain-named
function import whose item name matches a root entry resolves. One without a
match surfaces the unresolved-import link error of [PDD007], with the import's
name carried through. The link-time signature check of interface-typed imports
applies unchanged to root entries.

## The Host Call Context

This PDD revises the host function closure contract of [PDD008]. Where [PDD008]
gave the closure a `&mut T` (the host data of the `Store<T>`), this PDD gives it
a polyfill-owned context value named `HostCall`.

`HostCall<'_, T>` is the host's view, scoped to one call, of the state the
trampoline invokes the closure against. Two surfaces are reachable on it. The
first is the host data of type `T`, through accessors that mirror `Store::data`
and `Store::data_mut`. The second is a mint accessor that creates a fresh
resource handle for a registered resource type identity. The mint accessor has
the same spelling as the mint entry point on `Store`, because the host call
context is a borrowed view onto the store. No upstream `StoreContextMut` or
other runtime layer type appears at the boundary.

The typed entry point follows the same contract. The closure receives the same
`HostCall<'_, T>` context, with the typed argument tuple and return type erased
into or projected out of `Val` slots by the traits [PDD010] introduced.

This is a revision of [PDD008]'s contract, not an extension. Every call site
takes a `HostCall<'_, T>` as the closure's first parameter, and host data reads
route through its accessor.

## Error Model Growth

No new variants are introduced. A plain-named import with no root registration
surfaces [PDD007]'s unresolved-import link error. A host function that mints a
handle against a resource type identity the store does not recognize surfaces
[PDD009]'s unregistered-resource-type ABI cause. A typed registration whose Rust
signature does not satisfy a plain-named import surfaces [PDD008]'s
`Error::TypeMismatch`, with a position that names the plain-named registration.

## User Stories

A developer adopting the polyfill against a WIT-conformant component wants to
register a top-level host function that the component imports under a plain
name, for example `(import "log" (func …))`.

> The developer reaches for the root registration entry point and registers a
> typed `log` closure that takes a `String`. Instantiation succeeds. The guest's
> call to `log` reaches the closure, which observes the lifted string.

A developer builds a host that mints resources during guest calls and wants a
host function to return a live handle.

> The developer registers a host function whose declared return type is
> `own<thing>`. Inside the closure, `HostCall<'_, T>` mints a handle for a
> host-side rep. The closure returns it through the typed surface. The guest
> reads a live handle.

## References

- [PDD000], the product overview.
- [PDD003], the compatibility outlook. This PDD closes the registration surface
  rows of "Linking, Instantiation, and Host Integration".
- [PDD005], the foundations and posture.
- [PDD007], linking and instantiation. This PDD's root accessor parallels its
  interface accessor.
- [PDD008], the Canonical ABI and host functions. This PDD revises its closure
  contract.
- [PDD009], resources. This PDD exposes its handle table through the host call
  context.
- [PDD010], the typed call surface.
- [Wasmtime], whose `Linker::root()` and store-context callbacks shape this
  design.

[PDD000]: ./PDD000%20Wasm%20Component%20Model%20Polyfill.md
[PDD003]: ./PDD003%20Compatibility%20Outlook.md
[PDD005]: ./PDD005%20Library%20Foundations.md
[PDD007]: ./PDD007%20Linking%20and%20Instantiation.md
[PDD008]: ./PDD008%20Canonical%20ABI%20and%20Host%20Functions.md
[PDD009]: ./PDD009%20Resources.md
[PDD010]: ./PDD010%20Pre-wasip3%20Public%20API.md
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
