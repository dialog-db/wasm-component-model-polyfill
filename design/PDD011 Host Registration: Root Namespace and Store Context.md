# Host Registration: Root Namespace and Store Context

[PDD008] introduced the polyfill's host-registration surface against
*interface-typed* imports: a host attaches functions and resources
to a borrowed view onto an interface's registration entry, and the
linker resolves a component's interface-named imports against those
entries at link time. Two real shapes that appear in WIT-conformant
components fall outside that surface — a component that imports a
*plain-named* function at the root of its import list, and a host
function that needs to mint a fresh resource handle from within the
body of a guest call. This PDD revises [PDD008]'s registration
shape on both fronts: it adds a root namespace addressed without an
interface identifier, and it widens the host-function callback's
context so registered closures can read and mutate the per-store
handle tables [PDD009] introduced.

## Goals

- A `Linker` exposes a root-namespace registration entry point that
  produces the same registration view shape [PDD008]'s interface
  entry point returns. Plain-named imports resolve through it.
- A registered host function's body can read and mutate the per-
  store handle tables: minting a fresh `own<T>` from within a
  registered closure returns a handle whose lift back into the
  guest is observable as a live entry in the table.
- The typed and untyped registration entry points
  ([PDD008]'s `func_wrap` and its untyped sibling) agree on the
  host-function context they pass — neither is more capable than
  the other.
- The polyfill's link-time and call-time error variants
  ([PDD007]'s `Error::Link`, [PDD008]'s `Error::TypeMismatch`,
  [PDD009]'s `Error::Abi`) cover the new failure modes introduced
  here without parallel error hierarchies.

## Non-goals

- Asynchronous host functions or async resource destructors. Both
  remain out of scope.
- A host-binding code generator (a `wit-bindgen!` equivalent). The
  registration surface is what a hand-written host calls; bindgen
  consumes the same surface from generated code.
- Borrow-lifetime tracking inside a host call. The host's view of
  borrow handles remains the lift-only contract this PDD inherits.
- Type-erasing the rep parameter or keying registrations by Rust
  type identity. Reps remain `u32` opaque to the polyfill.

## The Root Namespace

`Linker` gains a root accessor that returns a registration view
whose entry map carries items addressed by the polyfill's root
namespace rather than by an interface identifier. Calling the
accessor twice returns a view onto the same registration entry,
exactly as [PDD007]'s interface accessor does for interface
identifiers. The accessor's return type is the polyfill's existing
registration view — there is no parallel root-only view type,
because the registration operations are identical regardless of
where the items eventually get resolved against.

The resolver's plain-named import branch consults the root
registration before raising the unsupported-registration link
error [PDD007] rejects with today. A plain-named function import
whose item-name matches a root registration resolves; one without
a match still surfaces as the polyfill's unresolved-import link
error with the import's name carried through. The link-time
signature check the resolver applies to interface-typed imports
applies unchanged to root entries.

## The Host-Call Context

This PDD revises [PDD008]'s host-function callback contract. Where
[PDD008] specified the closure's first parameter as `&mut T` (the
host-data slot of the [PDD007]-introduced `Store<T>`), this PDD
replaces it with a polyfill-owned context value named `HostCall`.

`HostCall<'_, T>` is the host's view, scoped to one call, of the
state the trampoline is invoking the closure against. Two
surfaces are reachable on it: the host-data slot of type `T`
(read via a polyfill-named accessor that mirrors [PDD007]'s
`Store::data`/`data_mut` shape), and a polyfill accessor that
mints a fresh resource handle for a registered resource-type
identity ([PDD009]). The mint accessor mirrors the spelling
[PDD009] established for `Store`'s own mint entry point — the
host-call context is, structurally, a borrowed view onto the
store the trampoline runs against. No upstream `StoreContextMut`
or other substrate type appears at the boundary; the polyfill
owns its host-context surface end-to-end.

The typed entry point ([PDD008]'s `func_wrap`) follows
symmetrically: the closure receives the same `HostCall<'_, T>`
context with the typed argument tuple and return type erased into
or projected out of the polyfill's `Val` slots by the
implementation traits [PDD010]'s typed call surface introduced.

This is a revision to the contract [PDD008] documented, not an
extension of it: existing call sites are mechanically updated so
the closure's first parameter takes a `HostCall<'_, T>` and host-
data reads route through its accessor. The polyfill's tests
exercise both shapes of update.

## Error Model Growth

The new failure modes are additive variants on the existing error
hierarchy. A plain-named import that finds no root registration
falls under [PDD007]'s unresolved-import link error with the
import's name carried unchanged. A host function that mints a
handle against a resource-type identity the store's tables do not
recognise surfaces [PDD009]'s unregistered-resource-type ABI cause
— the existing variant captures the failure shape without growth.

A typed registration whose Rust signature does not satisfy a
plain-named import surfaces [PDD008]'s `Error::TypeMismatch`. The
position payload distinguishes the plain-named registration from
the interface-named one [PDD008] documented; this PDD claims the
plain-named position variant as its own, formalising what the
codebase has reserved since [PDD008] landed.

## User Stories

**As a developer adopting the polyfill against a WIT-conformant
component**, I want to register a top-level host function the
component imports under a plain name (for example, `(import "log"
(func ...))`), so that the registration surface mirrors the
component's actual import shape rather than forcing every host item
to live inside an interface.

> The developer constructs a `Linker`, reaches for the root
> registration entry point, and registers a typed `log` closure
> that takes a `String` and returns nothing. Instantiating the
> component succeeds; the guest's call to `log` reaches the
> registered closure, which observes the lifted Rust string.

**As a developer building a host that mints resources during guest
calls**, I want a host function to be able to allocate a fresh
`own<T>` handle in the per-store table from within its body, so
that constructor-shaped imports and factory-shaped helpers can
return live handles without leaving the host context.

> The developer registers a host function whose declared return
> type is `own<thing>`. Inside the closure, the `HostCall<'_, T>`
> context exposes the per-store handle table; the closure mints a
> handle for a host-side rep, returns it through the typed
> surface, and the lifted handle is live in the table when the
> guest reads it back.

## References

- [PDD000] — product overview.
- [PDD001] — development environment.
- [PDD002] — ecosystem foundation.
- [PDD003] — compatibility outlook. This PDD closes the
  registration-surface rows of "Linking, Instantiation, and Host
  Integration".
- [PDD004] — test macros.
- [PDD005] — library foundations.
- [PDD007] — linking and instantiation; the PDD whose interface
  registration entry point this PDD's root accessor parallels and
  whose `Store<T>` the host-call context borrows from.
- [PDD008] — canonical ABI and host functions; the PDD whose host-
  function callback contract this PDD revises.
- [PDD009] — resources; the PDD whose handle-table surface the
  host-call context exposes through a polyfill accessor.
- [PDD010] — pre-wasip3 public API; the PDD whose typed call
  surface erases through the same context.
- [`wasm_runtime_layer`] — the runtime substrate.
- [`wasm_component_layer`] — prior art only.
- [Wasmtime] — the reference runtime, whose `Linker::root()` and
  `StoreContextMut`-flavoured callbacks shape this PDD's design.
- [Explainer] — the canonical Component Model design document.

[PDD000]: ./PDD000%20Wasm%20Component%20Model%20Polyfill.md
[PDD001]: ./PDD001%20Development%20Environment.md
[PDD002]: ./PDD002%20Ecosystem%20Foundation.md
[PDD003]: ./PDD003%20Compatibility%20Outlook.md
[PDD004]: ./PDD004%20Test%20Macros.md
[PDD005]: ./PDD005%20Library%20Foundations.md
[PDD007]: ./PDD007%20Linking%20and%20Instantiation.md
[PDD008]: ./PDD008%20Canonical%20ABI%20and%20Host%20Functions.md
[PDD009]: ./PDD009%20Resources.md
[PDD010]: ./PDD010%20Pre-wasip3%20Public%20API.md
[`wasm_runtime_layer`]: https://github.com/DouglasDwyer/wasm_runtime_layer
[`wasm_component_layer`]: https://github.com/DouglasDwyer/wasm_component_layer
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
[Explainer]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md
