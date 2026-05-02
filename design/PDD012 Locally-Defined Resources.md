# Locally-Defined Resources

The polyfill's resource subsystem today only models *imported*
resources — the ones a component pulls in through an interface
import and the host satisfies via the registration entry [PDD009]
introduced. A component that declares its own resource with an
in-binary destructor (`(type (resource (rep i32) (dtor (func $f))))`)
is rejected at translation time: the executor's
`GlobalInitializer::Resource` arm is `todo!()`. This PDD adds the
locally-defined-resource path so a component can own its own
resource type end-to-end without a host registration.

## Goals

- A component declaring a locally-defined resource with an
  in-binary destructor instantiates without rejection. The
  destructor runs synchronously when the guest issues
  `resource.drop` against the last live handle.
- The polyfill mints a fresh resource-type identity per
  *instantiation*: two instantiations of the same component carry
  distinct resource types, so a handle minted by one instance
  cannot be lifted by another.
- The locally-defined and host-imported paths share the same
  per-store handle-table machinery [PDD009] established, including
  the mint entry point on `Store` and the lift / lower handle
  paths.

## Non-goals

- Asynchronous destructors. The destructor is a synchronous core
  function, and is invoked synchronously on drop.
- Cross-component sharing of locally-defined resource types. The
  identity is per-instantiation; sharing across components is the
  imported-resource path's contract.
- Host-side registration against a locally-defined resource. The
  host has no entry point to add items to a guest-defined resource
  type; that mixed shape is out of scope.
- The strongly-typed `Resource<T>` developer surface. Locally-
  defined resources surface through the polyfill's existing handle
  and resource-type-identity data shapes — reps remain `u32`
  opaque to the polyfill.

## Translation

The executor's translator gains a non-`todo!()` projection for
`GlobalInitializer::Resource`. Each locally-defined resource emits
a polyfill-side spec that carries the destructor's source — a
`CoreDef` the executor's existing `lift_core_def` projection turns
into the polyfill's `ImportSource` — alongside the polyfill index
of the defining component instance. A resource without a declared
destructor is still admitted; its drop trampoline runs the table
removal but invokes no destructor.

The spec lives alongside the existing `ResourceSpec` entries in the
executor's IR. Whether the polyfill widens `ResourceSpec` with an
additional `origin: { Imported(import_index, item_name) |
Local(dtor_source) }` field, or introduces a parallel
`LocalResourceSpec` shape, is an implementation detail; the public
surface preserves the existing handle and resource-type-identity
shapes [PDD009] established and the handle-specific ABI causes
[PDD009] grew on `Error::Abi`.

## Instantiation

At instantiate time, the executor resolves each local resource's
`dtor_source` to a runtime-layer function against the core
instances the prior `InstantiateModule` directives produced. The
destructor is invoked synchronously by the resource's
`resource.drop` trampoline, with the dropped entry's rep as its
single i32 argument — mirroring the in-binary destructor's
declared signature.

A fresh resource-type identity is minted per instantiation. Two
instantiations of the same component produce distinct identities,
so a handle minted by one instance cannot be lowered into the
other; attempting to do so surfaces the unregistered-resource-type
ABI cause [PDD009]'s error growth introduced on `Error::Abi`.

## Error Model Growth

No new error variants are introduced. The locally-defined path
reaches the existing handle-specific ABI causes [PDD009] grew on
`Error::Abi` (invalid handle, unregistered resource type) and
[PDD007]'s instantiation error variant when the substrate refuses
the destructor (for example, a destructor whose declared signature
does not match the canonical `(func (param i32))` shape).

## User Stories

**As a developer authoring a component that owns its own resource
type**, I want the polyfill to honour the component's in-binary
destructor without forcing me to register a host-side dtor, so
that components self-contained on the wire are also self-contained
at runtime.

> The developer compiles a component declaring
> `(type (resource (rep i32) (dtor (func $f))))` and exporting a
> function that takes ownership of a handle. Instantiation
> succeeds; calling the export with a polyfill-minted handle drives
> the in-binary destructor exactly once, and the polyfill's host
> code never names the resource's destructor surface.

**As a contributor who needs two instances of a component to
remain isolated under guest-defined resources**, I want the
polyfill to treat each instantiation's resource type as a distinct
identity, so that handle confusion across instances is impossible
by construction.

> The contributor instantiates the same component twice, mints a
> handle through one instance, and attempts to lift it through the
> other. The lift surfaces the unregistered-resource-type ABI
> cause; the polyfill's per-instance identity prevents the
> cross-instance flow.

## References

- [PDD000] — product overview.
- [PDD001] — development environment.
- [PDD002] — ecosystem foundation.
- [PDD003] — compatibility outlook. This PDD closes the locally-
  defined-resource row of "Resources".
- [PDD004] — test macros.
- [PDD005] — library foundations.
- [PDD006] — component parsing; the PDD whose IR projection this
  one extends.
- [PDD007] — linking and instantiation; the PDD whose instantiation
  error variant the destructor-resolution path may surface.
- [PDD008] — canonical ABI and host functions; the PDD that
  introduced `Error::Abi`, the variant the resource lift / lower
  paths grow causes on.
- [PDD009] — resources; the PDD whose imported-resource path this
  one mirrors and whose handle-table machinery it shares.
- [`wasm_runtime_layer`] — the runtime substrate.
- [Wasmtime] — the reference runtime, whose resource-initialization
  directive the polyfill's executor projects from.
- [Explainer] — the canonical Component Model design document.

[PDD000]: ./PDD000%20Wasm%20Component%20Model%20Polyfill.md
[PDD001]: ./PDD001%20Development%20Environment.md
[PDD002]: ./PDD002%20Ecosystem%20Foundation.md
[PDD003]: ./PDD003%20Compatibility%20Outlook.md
[PDD004]: ./PDD004%20Test%20Macros.md
[PDD005]: ./PDD005%20Library%20Foundations.md
[PDD006]: ./PDD006%20Component%20Parsing.md
[PDD007]: ./PDD007%20Linking%20and%20Instantiation.md
[PDD008]: ./PDD008%20Canonical%20ABI%20and%20Host%20Functions.md
[PDD009]: ./PDD009%20Resources.md
[`wasm_runtime_layer`]: https://github.com/DouglasDwyer/wasm_runtime_layer
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
[Explainer]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md
