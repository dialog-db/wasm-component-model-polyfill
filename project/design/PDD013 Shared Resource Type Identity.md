# Shared Resource Type Identity

[PDD009]'s registration entry binds a fresh identity to each call. Calling
`resource("name", dtor)` twice, even with the same name under two interfaces,
mints two identities. That is correct for two different resource types that
share a label. It forecloses a case that WIT-conformant components produce
routinely: one resource type imported through two interfaces, with handles
minted under one interface flowing through the other. This PDD introduces a
host-side resource type value with an engine-issued identity that several
registrations can reference.

## Goals

- A polyfill-owned `ResourceType` value carries an opaque identity. Construction
  yields a fresh identity. Cloning preserves it. Equality is identity equality.
- A host registers one `ResourceType` value against the labels of two or more
  registration entries. A handle minted under one registration lowers through
  the other. The handle table holds one live entry per identity and index.
- The link-time resolver rejects a registration whose resource type identity
  disagrees with the import's expected identity, with [PDD008]'s
  `Error::TypeMismatch` and a position that names the interface and the item.
- The single-interface registration stays a one-liner through a convenience that
  mints the identity inline.

## Non-goals

- A strongly typed `Resource<T>` developer surface. Identity is a run-time
  value, not a Rust `TypeId`.
- Locally-defined resource types. Their identity is per instantiation and is
  governed by [PDD012].
- Asynchronous destructors.

## The Resource Type Value

The polyfill exposes a `ResourceType` value in its public API. Construction
takes the destructor closure and produces a value with a fresh identity. The
identity is opaque. Cloning preserves it, so one value can be passed to several
registrations without re-keying.

This PDD revises [PDD009]'s registration entry. Where [PDD009] took a label and
a destructor and minted a fresh identity per call, this PDD takes a
`ResourceType` value with the label, and the destructor lives inside the value.
The single-call form of [PDD009] stays as a convenience that mints a fresh
`ResourceType` inline and forwards to the widened call.

## Multi-Interface Registration

A `ResourceType` value passed to two registration entries binds the same
identity to both. The resolver sees the same identity on each side and accepts
the registration. The trampoline's lookup of the destructor reaches the same
closure through either interface.

A handle minted through the store's mint entry point against the shared identity
is live across both interfaces' lift and lower paths. The handle table is keyed
by identity, not by interface, so a handle needs no migration when it crosses
from one interface to the other.

## Error Model Growth

No new variants are introduced. [PDD008]'s `Error::TypeMismatch` covers two
registrations against one identity that disagree on the destructor's shape. The
unregistered-resource-type ABI cause of [PDD009] covers a handle whose identity
is absent from the store. The inline convenience surfaces the same failures as
the explicit form.

## User Stories

A developer integrates a component that imports one resource type through two
interfaces and wants to register the resource once.

> The developer constructs a `ResourceType` for their host entity and passes a
> clone to the resource call of each interface's registration entry. The
> component instantiates. The guest receives a handle from one interface's
> function and passes it to the other. The lift accepts it.

A developer registers against one interface and wants the one-liner to stay.

> The developer calls `instance.resource("name", |state, rep| { … })`. The
> polyfill mints a fresh identity inline and returns it.

## Test Cases

A resource type carries an opaque identity. Two constructions yield two
identities, a clone compares equal to its source, and equality is identity
equality only.

One resource type serves two interfaces. A handle minted under one interface's
registration lowers through the other interface's registration, and the handle
table holds one live entry for it.

A mismatched identity fails at link time. A registration whose resource type
identity differs from the import's expected identity is rejected with the
type-mismatch error and a position that names the interface and the item.

The single-interface registration stays a one-liner. A resource call with a
destructor and no explicit type mints a fresh identity inline.

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
