# Resource Disposal

[PDD009] gave the host a way to mint a resource handle and hand it to a guest.
This PDD gives the host a way to release a handle it still holds, and it states
what happens to live handles when a store or an instance goes away. The model
follows [Wasmtime]: release is explicit, a dropped store runs no destructor, and
an instance lives as long as its store.

## Goals

- A host releases a handle it holds through one entry point on `Store`. The
  destructor of the resource runs exactly once, and the table slot is free for
  reuse.
- A dropped `Store` has a stated behavior for live handles, and a test observes
  it on both targets.
- An `Instance` has a stated lifetime relative to its `Store`, and a test
  observes it.
- No runtime layer type appears in the release entry point.

## Non-goals

- Running destructors when a `Store` drops. The host releases the handles it
  still holds before it drops the store, or accepts that their destructors do
  not run.
- Freeing an instance's memory before its store drops. The runtime layer's
  backends keep instance memory for the lifetime of the store on both targets.
- Releasing a borrow. A borrow the host received from a guest belongs to the
  call it came from and returns with it.

## Host Release

`Store` gains a release entry point. It takes an owned handle by value, so the
host cannot use the handle after the call. The entry point removes the handle's
entry from the host's table for the resource type. An entry that is not live
surfaces the invalid-handle ABI cause of [PDD009]. An entry that is lent out as
a borrow surfaces the same cause with Wasmtime's wording, because a borrowed
resource cannot be removed.

The entry point then runs the destructor of the resource type. For a resource a
host registered, the destructor is the closure of that registration. For a
resource a component defines, the destructor is the core function the component
named, and it runs against the instance that defined the resource. The store
learns each destructor when an instance is created: instantiation records the
destructor of every resource type of the instance in the store, keyed by the
resource type identity. A resource type that no instance in the store introduced
has no destructor, and release frees the slot and runs nothing.

A destructor runs once per release. A handle the host released cannot be
released again, because its entry is gone.

## Store Drop

A dropped `Store` frees the substrate's memories and every table. It runs no
destructor. This is the behavior of Wasmtime, where an undropped resource is
leaked. A host that wants a destructor to run releases the handle first.

The reason is control. A destructor is host code or guest code, and it can fail.
Running it from `Drop` would hide the failure and would run guest code at a time
the host did not choose. The explicit release entry point runs it where the host
can see the result.

## Instance Lifetime

An `Instance` is a handle onto state the `Store` owns. Dropping an `Instance`
releases nothing: the substrate keeps the instance's memory and tables until the
store drops, on both targets. The host can drop the `Instance` value at any time
and keep using the store and other instances. Nothing observable changes.

A handle minted by an instance that the host has dropped stays valid, because
its table and its destructor live in the store.

## User Stories

A host program keeps a pool of resources it hands to a guest and wants to retire
one it never gave away.

> The host mints a handle for the resource. The guest never asks for it. The
> host releases the handle through the store. The destructor runs once, and the
> next handle the host mints reuses the slot.

A host program shuts down and wants to know what it must do.

> The host releases every handle it still holds, then drops the store. Handles
> it forgot are leaked, and their destructors do not run. The host reads this in
> the store's documentation before it decides.

A host program creates instances in a loop and wants to know what it pays.

> The host drops each `Instance` after use. The memory stays in the store until
> the store drops. The host bounds memory by bounding the store, not the
> instance.

## Test Cases

A host release runs the destructor once. A host registers a resource type whose
destructor records the rep, mints a handle, and releases it through the store.
The destructor ran once with that rep, and the next mint reuses the freed slot.

A double release is refused. Releasing a handle that was released already fails
with the invalid-handle ABI cause, and the destructor does not run again.

A locally-defined resource releases through the store. The host lifts an owned
handle out of a component that defines its resource, releases it through the
store, and the component's in-binary destructor runs once.

A dropped store runs no destructor. A host mints a handle, drops the store, and
observes that the destructor did not run.

An instance drops before its store. The host drops an `Instance`, then calls an
export of another instance in the same store and releases a handle the dropped
instance minted. Both succeed.

## References

- [PDD009], resources.
- [PDD012], locally-defined resources.
- [Wasmtime], whose `ResourceAny::resource_drop` and store semantics this design
  follows.
- [CanonicalABI – runtime state], the handle table rules.

[PDD009]: ./PDD009%20Resources.md
[PDD012]: ./PDD012%20Locally-Defined%20Resources.md
[Wasmtime]:
  https://docs.rs/wasmtime/latest/wasmtime/component/struct.ResourceAny.html#method.resource_drop
[CanonicalABI – runtime state]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#runtime-state
