# Component Composition

A component can contain other components. Tools such as `wac` and
`wasm-tools compose` produce such a component by wiring the exports of one inner
component to the imports of another. Dynamic linking and code re-use, the reason
[PDD000] gives for the polyfill, depend on this shape. This PDD brings composed
components into the synchronous baseline of [PDD006]: a composed component
links, instantiates, and runs on every supported target, and a call from one
inner component to another crosses the boundary as the Canonical ABI specifies.

## Goals

- A component composed from two or more components instantiates through the
  existing `Linker::instantiate`, with no composition-specific API.
- A call from one inner component to another passes through an adapter module.
  The adapter lifts from the caller's memory and lowers into the callee's
  memory, including string transcoding between encodings.
- The `may_leave` flag that the Canonical ABI requires exists per inner instance
  and is enforced.
- A call from an inner instance into a function that the same instance lifted
  traps with Wasmtime's message. A call between two different inner instances
  does not trap for reentrance.
- An `own<T>` or `borrow<T>` handle passes from one inner component to another.
  The handle table of the source removes or lends the entry, and the handle
  table of the destination receives it.
- Instance-typed imports and exports nest to any depth.
- The `linking` directory of the Component Model test corpus and the adapter,
  aliasing, linking, nested, and string-transcode tests of the Wasmtime
  component tests pass, except for cases the polyfill records as expected
  failures.

## Non-goals

- Asynchronous adapters. An adapter between an `async` export and an `async`
  import belongs to the concurrency tier.
- Module-typed imports and exports. A component that imports a core module from
  the host is out of scope.
- Value imports, value exports, and `start`.
- A composition tool. The polyfill consumes composed components. It does not
  produce them.

## Adapter Modules

The translator of [PDD002] emits one adapter module per group of cross-component
calls. Wasmtime calls this mechanism FACT, the fused adapter compiler. An
adapter is an ordinary core module. Its functions implement one lift from the
caller followed by one lower into the callee, in core Wasm, without a round trip
through the host. The adapter imports the memories, `cabi_realloc` functions,
and `post-return` functions of both sides, and imports the intrinsics it needs
from the polyfill.

The plan of [PDD006] lists adapter modules alongside the component's own core
modules and schedules their instantiation. The polyfill compiles and
instantiates them like any other core module. The plan's import sources name the
adapter's functions where an inner component's import is satisfied by another
inner component's export.

## Instance Flags

Every component instance carries a flags global with the `may_leave` flag that
the [Canonical ABI runtime state][CanonicalABI – runtime state] defines. An
adapter traps when the caller's `may_leave` flag is clear. The adapter clears
the flag on an instance while it copies values into or out of the memory of that
instance, because a `cabi_realloc` call can run there. A host trampoline
consults the caller's flag before a guest leaves its component through it.

An adapter whose caller and callee are the same component instance copies no
value. The [fused adapter compiler][Wasmtime same instance] compiles it to a
trap, `CannotEnterComponent`, with the message
`cannot enter component instance`. The Canonical ABI allows the call. Wasmtime
traps because the guest can otherwise observe the order of the adapter's lifts
and lowers. The polyfill takes its adapters from that compiler, so it raises the
same trap. A call between two different inner instances does not trap for
reentrance, in any direction and at any depth.

The polyfill materializes each flags global as a core global that the plan
names. The global is an import source like a memory or a function. The flag is
per instance, so two instantiations of one component carry separate flags.

## Intrinsics

An adapter imports a small set of functions from the polyfill:

- String transcoders. When the caller and the callee declare different string
  encodings, the adapter calls a transcoder that converts between UTF-8, UTF-16,
  and Latin-1 with UTF-16 fallback, in the direction and with the bounds the
  [Canonical ABI storing rules][CanonicalABI – storing] require.
- Resource transfer. When a handle crosses a boundary, the adapter calls a
  transfer intrinsic. For `own<T>` the intrinsic removes the entry from the
  source table and inserts it into the destination table. For `borrow<T>` the
  intrinsic lends the entry for the duration of the call, using the scope
  machinery of [PDD014].
- A trap intrinsic, which the adapter calls when a flag check or a bounds check
  fails, or when its caller and callee are the same instance. The trap carries
  the message Wasmtime uses.

Each intrinsic is a host trampoline the polyfill builds at instantiation. The
plan names which intrinsic an adapter import refers to.

## Nested Instances

A composed component's imports and exports can nest: an instance-typed export
can contain another instance-typed export. The export navigator of [PDD010]
reaches a function at any depth by walking instance views. The identifier model
of [PDD006] keys each level. Resolution of [PDD007] walks a nested import path
to its leaf item.

## Error Model Growth

No new top-level variants are introduced. A trap raised by an adapter or an
intrinsic surfaces through the instantiation and ABI variants that exist, with
the trap message carried through. The ABI variant gains a cause for a handle
transfer whose destination does not accept the resource type.

## User Stories

A developer receives a component composed with `wac` from a Rust component and a
Go component, and wants to run it without knowing its structure.

> The developer awaits `Component::new` and `Linker::instantiate` and calls an
> outer export. The call passes into the Rust component, which calls the Go
> component through an adapter. Strings cross the boundary in the encoding each
> side declared.

A developer composes a component that hands a resource from one inner component
to another.

> The first inner component returns an `own<T>`. The adapter transfers the
> handle to the second component's table. The second component drops it, and the
> first component's destructor runs exactly once.

A contributor wants a call into the caller's own instance to surface as a trap
rather than as memory corruption.

> A test builds a component whose start function calls a function that the same
> component lifted. Instantiation fails with the message Wasmtime uses,
> `cannot enter component instance`, and the adapter copies no value.

## Test Cases

A composed component instantiates through the existing linker. A component built
from two inner components instantiates with no composition-specific API and its
outer export returns the value the inner components compute together.

Strings cross an adapter in each encoding. A call from a UTF-8 component into a
UTF-16 component passes a string through the adapter and the callee reads the
same text, and the reverse direction transcodes back.

The `may_leave` flag is enforced. A call out of an instance whose `may_leave`
flag is clear traps with the message Wasmtime uses.

A call into the caller's own instance traps. A component whose start function
calls a function that the same component lifted fails to instantiate with
`cannot enter component instance`. The `same-instance-call.wast` file of the
Wasmtime corpus traps with the same message for strings, lists, and calls with
no values in memory.

A handle transfers between inner components. An `own<T>` returned by one inner
component and dropped by the other runs the first component's destructor exactly
once.

The upstream linking cases pass. The `linking` directory of the Component Model
corpus and the adapter, aliasing, linking, nested, and string-transcode files of
the Wasmtime corpus pass except for the cases the expected-failure list records.

## References

- [PDD000], the product overview.
- [PDD002], the ecosystem foundation, including the translator that emits
  adapter modules.
- [PDD003], the compatibility outlook. This PDD covers the composition row and
  the string transcoding row.
- [PDD005], the foundations and posture.
- [PDD006], component parsing and the synchronous baseline.
- [PDD007], linking and instantiation.
- [PDD008], the Canonical ABI and host functions.
- [PDD009], resources.
- [PDD010], the export navigator.
- [PDD014], borrow lifetime tracking.
- [Linking], the Component Model design document on linking.
- [CanonicalABI], including the [runtime state][CanonicalABI – runtime state]
  and the [storing rules][CanonicalABI – storing].
- [Wasmtime] at `v50.0.0`, whose fused adapter compiler produces the adapter
  modules, and the compiler's [same-instance trap][Wasmtime same instance].
- The [Component Model test corpus] and the [Wasmtime component tests].

[PDD000]: ./PDD000%20Wasm%20Component%20Model%20Polyfill.md
[PDD002]: ./PDD002%20Ecosystem%20Foundation.md
[PDD003]: ./PDD003%20Compatibility%20Outlook.md
[PDD005]: ./PDD005%20Library%20Foundations.md
[PDD006]: ./PDD006%20Component%20Parsing.md
[PDD007]: ./PDD007%20Linking%20and%20Instantiation.md
[PDD008]: ./PDD008%20Canonical%20ABI%20and%20Host%20Functions.md
[PDD009]: ./PDD009%20Resources.md
[PDD010]: ./PDD010%20Pre-wasip3%20Public%20API.md
[PDD014]: ./PDD014%20Borrow%20Lifetime%20Tracking.md
[Linking]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Linking.md
[CanonicalABI]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md
[CanonicalABI – runtime state]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#runtime-state
[CanonicalABI – storing]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#storing
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
[Wasmtime same instance]:
  https://github.com/bytecodealliance/wasmtime/blob/v50.0.0/crates/environ/src/fact/trampoline.rs#L117-L127
[Component Model test corpus]:
  https://github.com/WebAssembly/component-model/tree/main/test
[Wasmtime component tests]:
  https://github.com/bytecodealliance/wasmtime/tree/main/tests/misc_testsuite/component-model
