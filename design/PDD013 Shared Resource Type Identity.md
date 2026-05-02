# Shared Resource Type Identity

The polyfill's host-resource registration today binds a fresh
identity to each call against [PDD009]'s registration entry: calling
`resource("name", dtor)` twice — even with the same name under two
different interfaces — mints two distinct identities. That model is
correct for two genuinely-different resource types that happen to
share a label, but it forecloses the case a WIT-conformant component
routinely produces: one resource type imported through two
interfaces, with handles minted under one interface flowing through
the other. This PDD introduces a host-side resource-type value with
engine-issued identity that multiple registrations can reference.

## Goals

- A polyfill-owned `ResourceType` value carries an opaque
  engine-issued identity. Constructing one yields a fresh
  identity; cloning preserves it; equality is identity-based.
- A host registers the same `ResourceType` value against two (or
  more) registration entries' labels. Handles minted under one
  registration lower correctly through the other; the per-store
  handle table sees a single live entry per (identity, index)
  pair.
- The polyfill's link-time resolver continues to reject
  registrations whose declared resource type identity disagrees
  with the import's expected identity, surfacing [PDD008]'s
  `Error::TypeMismatch` with a position that names both the
  interface and the item.
- The existing single-interface registration shape stays
  ergonomic: a developer who only needs one interface keeps the
  current call shape via a polyfill-provided convenience that
  mints the identity inline.

## Non-goals

- Strongly-typed `Resource<T>` developer surface. Identity here is
  a runtime value, not a Rust-type-keyed `TypeId`. Typed handles
  are a separate concern.
- Locally-defined resource types declared by the component itself.
  Their identity is per-instantiation and is governed by the
  guest-defined-resource path, not by host registration.
- Asynchronous destructors. The destructor remains synchronous;
  identity sharing applies symmetrically to whatever destructor a
  registration carries.

## The Resource Type Value

The polyfill exposes a `ResourceType` value at the public API.
Construction takes the destructor closure and produces a value
that carries a fresh, engine-issued identity. The identity is
opaque to consumers; equality on `ResourceType` reduces to
identity equality. Cloning a `ResourceType` preserves the identity
— two clones compare equal — so the same value can be passed to
multiple registrations without re-keying.

This PDD revises [PDD009]'s registration entry shape on
`LinkerInstance`. Where [PDD009] specified a single-call form
that took a label and a destructor and minted a fresh identity per
call, this PDD widens the call to take a `ResourceType` value
alongside the label, with the destructor carried inside the
`ResourceType` itself. The single-call form [PDD009] documented is
preserved as a thin convenience that mints a fresh `ResourceType`
inline and forwards to the widened call.

## Multi-Interface Registration

A `ResourceType` value passed to two different registration
entries' resource calls binds the same identity to both entries.
The resolver's link-time check sees the same resource-type identity
on each side and accepts the registration; the trampoline path's
lookup of the destructor reaches the same underlying closure
regardless of which interface the import was addressed through.

A handle minted via [PDD009]'s mint entry point on `Store` against
the shared identity is live across both interfaces' lift / lower
paths. The per-store handle table is keyed by identity, not by
interface, so no migration step is required when a handle crosses
the boundary between the two interfaces.

## Error Model Growth

No new error variants are introduced. [PDD008]'s `Error::TypeMismatch`
covers the case where two registrations against a shared identity
disagree on the destructor's declared shape; the position payload
names the registration site (interface-named or plain-named) where
the mismatch was observed. The unregistered-resource-type ABI
cause [PDD009]'s error growth introduced on `Error::Abi` continues
to surface when a handle's identity is not present in the store.

The convenience inline-mint entry surfaces the same failure modes
the explicit shape does — under the hood it is just the explicit
shape with the polyfill calling the constructor.

## User Stories

**As a developer integrating a component that imports a single
resource type via two interfaces**, I want to register the
resource once and reference it from each interface, so that
handles flow naturally between the two interfaces without per-
interface re-registration of an identity.

> The developer constructs a `ResourceType` for their host-side
> entity, then calls `resource("foo", ty.clone(), dtor)` on both
> interfaces' registration entries. The component instantiates;
> the guest receives a handle from one interface's function and
> passes it to the other; the polyfill's lift accepts it.

**As a developer registering against a single interface**, I want
the existing inline-mint shape preserved, so that the common case
stays a one-liner.

> The developer calls `instance.resource("name", |state, rep| {
> ... })` exactly as before; the polyfill mints a fresh identity
> inline and returns its identity for the developer to use with
> the per-store mint entry point.

## References

- [PDD000] — product overview.
- [PDD001] — development environment.
- [PDD002] — ecosystem foundation.
- [PDD003] — compatibility outlook. This PDD closes the resource-
  type-sharing row of "Resources".
- [PDD004] — test macros.
- [PDD005] — library foundations.
- [PDD007] — linking and instantiation; the PDD whose link-time
  type check this one threads identity through.
- [PDD008] — canonical ABI and host functions; the PDD that
  introduced `Error::TypeMismatch`.
- [PDD009] — resources; the PDD whose registration entry shape
  this PDD revises and whose handle-table identity model it
  extends with multi-registration sharing.
- [`wasm_runtime_layer`] — the runtime substrate.
- [Wasmtime] — the reference runtime, whose host-side resource-
  type values shape this PDD's design.
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
[`wasm_runtime_layer`]: https://github.com/DouglasDwyer/wasm_runtime_layer
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
[Explainer]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md
