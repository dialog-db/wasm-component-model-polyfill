# Pre-wasip3 Public API

[PDD006] through [PDD009] bring the synchronous baseline to completion on the
wire. Components parse, link, instantiate, round-trip every baseline value type
through the Canonical ABI, and host-owned resources run their destructors. What
that work does not finish is the shape of the public API at the developer's eye
level. This PDD closes that gap before the concurrency tier adds surface on top
of it.

Two surfaces are introduced. The first is export navigation: a developer who
holds an `Instance` walks the export tree, reaches an instance-typed export by
its `InterfaceIdentifier`, and obtains a function handle inside it. The second
is the typed call surface: a developer converts an untyped function handle into
one whose Rust parameter tuple and return type are checked against the export's
signature at acquisition.

## Goals

- `Instance` exposes an export navigator that reaches root-level function
  exports and function exports nested inside an instance-typed export.
- A function handle exposes a typed conversion that produces a handle whose Rust
  parameter tuple and return type are checked at acquisition.
- The flat-name `Instance::get_func` accessor is preserved unchanged.

## Non-goals

- Asynchronous export invocation at the Component Model level. The typed call is
  an `async fn`, as every call is, but it targets synchronous exports.
- A host binding code generator.
- Convenience defaults that hide the engine, such as `Linker::default()`.
- Navigation beyond instance-typed and function-typed exports.

## The Export Navigator

`Instance` gains an `exports` accessor that returns a polyfill-owned navigator.
The navigator exposes two lookups. One is keyed by `InterfaceIdentifier` and
returns a view onto one instance-typed export, for example the
`(export "test:guest/foo" (instance …))` form. The other is keyed by function
name and returns the polyfill's `Func`. The function lookup exists at the root
of the export tree and on the instance view. A nested function export is visible
only through the instance view.

The navigator is a polyfill type. No upstream type appears at the navigation
boundary. The instance lookup uses the polyfill's `InterfaceIdentifier`.

`Instance::get_func` stays as the shorthand for root-level function exports. The
navigator adds the instance-typed traversal that `get_func` cannot see.

## The Typed Call Surface

`Func` gains a typed conversion that consumes the untyped handle and produces a
typed handle. A polyfill-defined trait constrains the parameter tuple type and
the return type to the value types of the synchronous baseline. The conversion
makes sure at acquisition that the export's declared signature satisfies the
requested Rust types. A mismatch surfaces as `wcmp::Error::TypeMismatch`.

Calling the typed handle is an `async fn`. It takes a tuple of native Rust
values and returns the native Rust result. The Canonical ABI round trip (lift,
lower, `cabi_realloc`, `post-return`) happens behind the typed surface and
surfaces the same `wcmp::Error::Abi` failures as the untyped path.

The trait mirrors the lift, lower, and typed-descriptor triple that
`wasmtime::component` exposes, but it is named in the polyfill's surface and
covers only the synchronous baseline. `own<T>` and `borrow<T>` are baseline
value types per [PDD009] and inhabit the typed surface.

## Error Model Growth

`wcmp::Error::TypeMismatch` gains the acquisition-time failure modes of the
typed conversion. The payload names the export, the declared signature, and the
Rust parameter tuple and return type the caller asked for. [PDD005]'s note about
`anyhow::Error` applies.

## User Stories

A developer adopting the polyfill wants to call an export with native Rust
values and reach into a component whose exports are organized under WIT
interfaces.

> The developer awaits `instantiate`, navigates to a named export instance,
> looks up a function, asks for a typed handle of
> `(Vec<String>, u32) -> String`, and awaits the call with native values.

A contributor opening the first concurrency PDD wants the synchronous surface
complete and stable.

> The contributor writes the concurrency surface against the navigator and the
> typed call surface additively.

## Test Cases

The export navigator reaches nested exports. A host navigates from an `Instance`
to an instance-typed export and to a function inside it, and the flat-name
accessor still finds a root-level function.

The typed call surface checks the signature at acquisition. A typed handle for a
parameter tuple and return type that match the export succeeds, and one that
does not match fails with the type-mismatch error before any call.

A typed call passes native values. A call through a typed handle with native
Rust arguments returns a native Rust result equal to the value the guest
computed.

## References

- [PDD000], the product overview.
- [PDD003], the compatibility outlook. This PDD closes the developer-facing API
  rows for the synchronous baseline.
- [PDD005], the foundations and posture.
- [PDD006], component parsing.
- [PDD007], linking and instantiation. This PDD extends its navigation surface.
- [PDD008], the Canonical ABI and host functions. This PDD extends its typed
  surface and its type-mismatch variant.
- [PDD009], resources.
- [Wasmtime], the reference implementation.

[PDD000]: ./PDD000%20Wasm%20Component%20Model%20Polyfill.md
[PDD003]: ./PDD003%20Compatibility%20Outlook.md
[PDD005]: ./PDD005%20Library%20Foundations.md
[PDD006]: ./PDD006%20Component%20Parsing.md
[PDD007]: ./PDD007%20Linking%20and%20Instantiation.md
[PDD008]: ./PDD008%20Canonical%20ABI%20and%20Host%20Functions.md
[PDD009]: ./PDD009%20Resources.md
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
