# Canonical ABI and Host Functions

[PDD006] introduced `Component` and the type-system data shapes. [PDD007]
introduced `Linker<T>`, `LinkerInstance<'_, T>`, and `Instance`, with
primitive-only export invocation. This document introduces the host-side value
family, `Val`, implements the Canonical ABI for every compound value type in the
synchronous baseline except `own<T>` and `borrow<T>`, and adds typed and untyped
host function registration on `LinkerInstance`.

This PDD inherits the synchronous baseline of [PDD006] and the implementation
posture of [PDD005].

## Goals

- A developer registers a host function, typed or untyped, against a
  `LinkerInstance<'_, T>`. The linker makes sure at link time that the
  registration's type satisfies the import's type.
- A developer calls an export whose signature uses any baseline value type
  except `own<T>` and `borrow<T>`. Arguments and results round-trip through the
  Canonical ABI: lift, lower, `cabi_realloc`, the parameter and result spill to
  memory, and synchronous `post-return`.
- The tests `it_defines_an_untyped_host_function`,
  `it_defines_a_typed_host_function`, and
  `it_invokes_an_exported_component_function` pass on every supported target.
- `Val` lives at the crate root (`wcmp::Val`) and carries polyfill-owned data.
- `wcmp::Error` grows with type-mismatch and ABI variants.
- The Canonical ABI surface accepts the handle types later without reshaping.

## Non-goals

- `own<T>` and `borrow<T>` lift and lower, the handle table, and host resource
  registration. The lift and lower machinery accepts the handle types additively
  but does not process them.
- Specialized list fast paths, for example `list<u8>`. They are permitted but
  not required. Observable behavior is the contract.
- Asynchronous lift and lower, per-task context, and the extension of the handle
  table to `future<T>` and `stream<T>`.

## The Canonical ABI Surface

Calling an export, typed or untyped, exercises the Canonical ABI. This PDD
covers, on every supported target:

- Lift and lower for every value type in the synchronous baseline except
  `own<T>` and `borrow<T>`, in argument and result position.
- The flat calling convention of the [Canonical ABI][CanonicalABI]. A parameter
  list with more than `MAX_FLAT_PARAMS` (16) flat values is passed through one
  pointer into memory. A result with more than `MAX_FLAT_RESULTS` (1) flat value
  is returned through a pointer. Both rules apply in both directions: when the
  host calls an export and when the guest calls a host function.
- Invocation of the guest's `cabi_realloc` when a lower needs guest memory, with
  the alignment and size rules the Canonical ABI specifies.
- The three string encodings a `canonopt` can declare: UTF-8, UTF-16, and
  Latin-1 with UTF-16 fallback.
- `post-return` after a synchronous lift, run after the caller observes the
  result.

The implementation reads and writes the runtime layer's `Memory` through the
workspace-internal accessors of [PDD005]. No upstream type is exposed. The call
entry points are `async fn`, as [PDD005] requires.

## The Host Value Surface

`Val` is the host-side value. It is the value-bearing counterpart to [PDD006]'s
`ValueType`. Every shape in `ValueType` has a case in `Val` whose payload is the
host-readable Rust representation. `Val` is the carrier for every untyped host
function call (`&[Val]` in, `&mut [Val]` out), the result of every untyped
export call, and the bridge between a typed host function and the Canonical ABI.

`Val` carries owned, polyfill-typed data for compound types, for example
`Val::Record` and `Val::List`. The cases for `own<T>` and `borrow<T>` exist so
that the enum is closed. Their semantics are out of scope.

## Host Function Registration

`LinkerInstance<'_, T>` gains two registration modes:

- Untyped registration takes a closure over `Val` slices. The developer supplies
  the declared function type. Lift and lower happen at call time against the
  import's declared signature.
- Typed registration takes a closure with statically typed arguments and result,
  mirroring [Wasmtime]'s `LinkerInstance::func_wrap`. The linker makes sure at
  link time that the Rust types satisfy the import's signature. A mismatch
  surfaces as `wcmp::Error::TypeMismatch` before the linker accepts the
  registration.

The closure of either mode is synchronous. It receives the store's host data and
returns a `Result`. A typed registration whose signature names a handle type is
rejected at link time with `wcmp::Error::TypeMismatch`. An untyped registration
against a handle-typed import is rejected at call time.

A host function that returns `Err(_)` traps the guest. The error reaches the
host's call as a `wcmp::Error` on every supported target.

## Error Model Growth

`wcmp::Error` grows with two variants:

- A type-mismatch variant for a host registration whose declared type does not
  unify with the component's import, and for a typed export call whose declared
  signature does not unify with the export's type.
- An ABI variant for a failure during lift or lower of one value. It carries the
  value position (an argument index or the result), the value type, and a
  structured cause: an out-of-bounds memory access, a missing `cabi_realloc`, a
  failed `cabi_realloc` call, an invalid encoding, or a host value that does not
  match the declared type.

Each variant carries a `#[source]` cause where one exists. [PDD005]'s note about
`anyhow::Error` applies.

## User Stories

A developer adopting the polyfill wants to register a typed host function and
call an export that uses strings, lists, and records.

> The developer builds a `Linker<T>`, registers a typed host function against
> the interface the component imports, awaits `instantiate`, awaits the export
> call, and observes the round trip through the polyfill's API.

A contributor opening a PDD that builds on this work wants the Canonical ABI in
place except for handle types.

> The contributor extends the lift and lower paths and the registration modes
> without reshaping either.

A reviewer evaluating a PDD wants a readable scope.

> The reviewer makes sure that the PDD lands only the three named tests, on both
> targets, and that no handle-type lift or lower and no resource registration
> appears.

## References

- [PDD000], the product overview.
- [PDD002], the ecosystem foundation.
- [PDD003], the compatibility outlook. This PDD covers the Canonical ABI rows
  and the host function rows.
- [PDD005], the foundations and posture.
- [PDD006], component parsing and the synchronous baseline.
- [PDD007], linking and instantiation.
- [Wasmtime], the reference implementation.
- [CanonicalABI], the rules this PDD implements for the baseline value types.

[PDD000]: ./PDD000%20Wasm%20Component%20Model%20Polyfill.md
[PDD002]: ./PDD002%20Ecosystem%20Foundation.md
[PDD003]: ./PDD003%20Compatibility%20Outlook.md
[PDD005]: ./PDD005%20Library%20Foundations.md
[PDD006]: ./PDD006%20Component%20Parsing.md
[PDD007]: ./PDD007%20Linking%20and%20Instantiation.md
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
[CanonicalABI]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md
