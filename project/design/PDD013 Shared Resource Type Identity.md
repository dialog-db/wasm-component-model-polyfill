# Shared Resource Type Identity

One resource type can reach a component through two interfaces. WIT-conformant
components produce this routinely: a handle minted under one interface flows
through the other. [PDD009]'s registration entry binds an identity to each
registration. This PDD makes that identity a host-side value that several
registrations can share.

## Goals

- A polyfill-owned `ResourceType` value is the identity of a resource type. It
  is `Copy`. Equality is identity equality.
- `ResourceType::host::<T>()` gives the identity that the Rust type `T` stands
  for. Each call with the same `T` gives the same identity.
- A host registers one `ResourceType` under the labels of two or more
  registration entries. A handle minted under one registration lowers through
  the other. The handle table holds one live entry per identity and index.
- The link-time resolver rejects a registration whose resource type identity
  disagrees with the import's expected identity, with [PDD008]'s
  `Error::TypeMismatch` and a position that names the interface and the item.

## Non-goals

- Locally-defined resource types. Their identity is per instantiation and is
  governed by [PDD012].
- Asynchronous destructors.

## The Resource Type Value

The polyfill exposes a `ResourceType` value in its public API. It is the
identity of a resource type and nothing else. It has two host sources, as in
[Wasmtime]:

- `ResourceType::host::<T>()` keys the identity on the Rust type `T`.
- `ResourceType::host_dynamic(payload)` keys the identity on a `u32`, for a host
  that makes resource types at run time.

A registration entry takes a label, a `ResourceType`, and a destructor. The
destructor belongs to the registration, not to the value.

## Multi-Interface Registration

The same `ResourceType` passed to two registration entries binds the same
identity to both. The resolver sees the same identity on each side and accepts
the registration. A host that writes `ResourceType::host::<T>()` in two places
gets the same identity in both, so no value needs to travel between the two
registrations.

A handle minted against the shared identity is live across both interfaces' lift
and lower paths. The handle table is keyed by identity, not by interface, so a
handle needs no migration when it crosses from one interface to the other.

## Error Model Growth

No new variants are introduced. [PDD008]'s `Error::TypeMismatch` covers a
registration whose identity differs from the identity the import expects. The
unregistered-resource-type ABI cause of [PDD009] covers a handle whose identity
is absent from the store.

## User Stories

A developer integrates a component that imports one resource type through two
interfaces and wants to register the resource once per interface without
plumbing.

> The developer registers `ResourceType::host::<File>()` under each interface's
> registration entry. The component instantiates. The guest receives a handle
> from one interface's function and passes it to the other. The lift accepts it.

## Test Cases

A resource type value is an identity. Two calls of `ResourceType::host` with the
same Rust type compare equal. Calls with different Rust types, and two different
dynamic payloads, compare unequal.

One resource type serves two interfaces. A handle minted under one interface's
registration lowers through the other interface's registration, and the handle
table holds one live entry for it.

A mismatched identity fails at link time. A registration whose resource type
identity differs from the import's expected identity is rejected with the
type-mismatch error and a position that names the interface and the item.

## References

- [PDD000], the product overview.
- [PDD003], the compatibility outlook.
- [PDD005], the foundations and posture.
- [PDD007], linking and instantiation. This PDD threads identity through its
  link-time check.
- [PDD008], the Canonical ABI and host functions, which introduced
  `Error::TypeMismatch`.
- [PDD009], resources. This PDD revises its registration entry.
- [PDD012], locally-defined resources.
- [Wasmtime], whose host-side resource type values shape this design.

[PDD000]: ./PDD000%20Wasm%20Component%20Model%20Polyfill.md
[PDD003]: ./PDD003%20Compatibility%20Outlook.md
[PDD005]: ./PDD005%20Library%20Foundations.md
[PDD007]: ./PDD007%20Linking%20and%20Instantiation.md
[PDD008]: ./PDD008%20Canonical%20ABI%20and%20Host%20Functions.md
[PDD009]: ./PDD009%20Resources.md
[PDD012]: ./PDD012%20Locally-Defined%20Resources.md
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
