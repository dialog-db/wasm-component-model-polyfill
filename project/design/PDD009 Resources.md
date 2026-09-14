# Resources

[PDD006], [PDD007], and [PDD008] bring the synchronous baseline to within one
feature of completion. Components parse, link, instantiate, and round-trip
values through the Canonical ABI for every baseline value type except the handle
types. This document closes that gap for host-imported resources. It introduces
the handle table, host resource registration with synchronous destructors, and
lift and lower for `own<T>` and `borrow<T>`.

This PDD inherits the synchronous baseline of [PDD006] and the implementation
posture of [PDD005].

## Goals

- A developer declares a host-owned resource type with a synchronous destructor
  against a `LinkerInstance<'_, T>`, hands handles to guest code, and observes
  the destructor run exactly once when the guest drops its handle.
- A developer calls an export whose signature uses `own<T>` or `borrow<T>`. The
  round trip follows the runtime-state rules of the Canonical ABI: no aliasing
  while live, deterministic index reuse.
- The test `it_defines_a_host_resource_with_a_sync_destructor` passes on every
  supported target.
- The handle representation does not expose a runtime layer type.
- `wcmp::Error::Abi` grows with handle-specific causes. No new top-level variant
  is added.

## Non-goals

- Asynchronous destructors.
- Resources that a component defines itself.
- Transfer of a handle between two components.
- The extension of the handle table to `future<T>` and `stream<T>`.

## The Resource Surface

`LinkerInstance<'_, T>` gains a third registration mode: host resource
registration. The host declares a resource type under a label and supplies a
synchronous destructor closure. The destructor receives the store's host data
and the host's representation of the resource. The representation (the "rep") is
a `u32` that the host chooses, typically an index into a host-managed table. The
destructor runs exactly once when a guest-held handle is dropped.

The registration returns a polyfill-owned resource type identity. The identity
threads through [PDD006]'s `ValueType` shapes and through [PDD008]'s `Val`
family. No upstream type appears.

## The Handle Table

The polyfill keeps one handle table per resource type per `Store<T>`. Index
allocation and reuse follow the [runtime-state
rules][CanonicalABI – runtime state] of the Canonical ABI. Indices are
non-aliasing while live. Reuse of a freed index is deterministic within one
store. The table is not exposed. Consumers meet handles only through `Val` and
the typed call surface.

The store exposes one entry point that mints a fresh `own<T>` handle for a
registered resource type and a rep. The host uses it to hand a resource to the
guest through an export call.

## Lift and Lower for Handle Types

Lifting a guest-produced handle into a host `Val` resolves the index to the host
representation. Lowering a host `Val` that carries a handle produces the index
the guest sees. A borrow is valid for the duration of the host call and is
released at return. Ownership transfer on `own<T>` follows the Canonical ABI. A
guest access to a transferred handle traps.

The guest reaches the table through the `resource.new`, `resource.rep`, and
`resource.drop` built-ins. Each built-in is a host trampoline that the polyfill
wires into the core instance.

## Error Model Growth

`wcmp::Error::Abi` gains handle-specific causes: an out-of-range or stale handle
index, a handle whose resource type identity does not match the declared type,
and a transfer of an `own<T>` to a host that registered no matching resource
type. [PDD005]'s note about `anyhow::Error` applies.

## User Stories

A developer adopting the polyfill wants to hand a guest a handle to a host-owned
resource and see the destructor run when the guest drops it.

> The developer registers a resource type with a destructor, awaits
> `instantiate`, mints a handle, passes it to an export that takes ownership,
> lets the guest drop it, and observes the destructor run exactly once.

A contributor opening a PDD that builds on this work wants the resource surface
complete on every target.

> The contributor extends the handle table and the registration mode without
> reshaping either.

A reviewer evaluating a PDD wants a readable scope.

> The reviewer makes sure that the PDD lands only the named test, on both
> targets, and that no asynchronous destructor surface appears.

## References

- [PDD000], the product overview.
- [PDD002], the ecosystem foundation.
- [PDD003], the compatibility outlook. This PDD covers the handle rows of the
  type system and the Canonical ABI, and the host resource row.
- [PDD005], the foundations and posture.
- [PDD006], component parsing and the synchronous baseline.
- [PDD007], linking and instantiation.
- [PDD008], the Canonical ABI and host functions.
- [Wasmtime], the reference implementation.
- [CanonicalABI], including the [runtime-state
  rules][CanonicalABI – runtime state].

[PDD000]: ./PDD000%20Wasm%20Component%20Model%20Polyfill.md
[PDD002]: ./PDD002%20Ecosystem%20Foundation.md
[PDD003]: ./PDD003%20Compatibility%20Outlook.md
[PDD005]: ./PDD005%20Library%20Foundations.md
[PDD006]: ./PDD006%20Component%20Parsing.md
[PDD007]: ./PDD007%20Linking%20and%20Instantiation.md
[PDD008]: ./PDD008%20Canonical%20ABI%20and%20Host%20Functions.md
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
[CanonicalABI]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md
[CanonicalABI – runtime state]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#runtime-state
