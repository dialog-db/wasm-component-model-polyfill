# Locally-Defined Resources

[PDD009] models resources that a component imports and that the host satisfies
through a registration entry. A component can also declare its own resource type
with an in-binary destructor: `(type (resource (rep i32) (dtor (func $f))))`.
Every guest that exports a resource does this. This PDD adds the locally-defined
resource path, so that a component owns its own resource type end to end without
a host registration.

## Goals

- A component that declares a locally-defined resource instantiates. The
  destructor runs synchronously when the guest drops the last live handle.
- The polyfill mints a fresh resource type identity per instantiation. Two
  instantiations of one component carry distinct resource types. A handle minted
  by one instance cannot be lifted by the other.
- The locally-defined and host-imported paths share the handle table machinery
  of [PDD009], including the mint entry point on `Store` and the handle lift and
  lower paths.

## Non-goals

- Asynchronous destructors. The destructor is a synchronous core function.
- Sharing of a locally-defined resource type across components. The identity is
  per instantiation.
- Host registration against a locally-defined resource.
- A strongly typed `Resource<T>` developer surface. Reps stay `u32` and opaque
  to the polyfill.

## Translation

The plan that [PDD006]'s translation produces carries one entry per
locally-defined resource. The entry names the destructor's source, a core
function of the defining instance, and the defining component instance. A
resource without a destructor is admitted. Its drop trampoline removes the table
entry and invokes nothing.

Whether the polyfill widens its imported-resource entry with an origin (imported
or local) or introduces a parallel entry shape is an implementation detail. The
public surface keeps the handle and resource type identity shapes of [PDD009]
and the handle-specific ABI causes on `Error::Abi`.

## Instantiation

At instantiation the polyfill resolves each local resource's destructor to a
core function of the instance that the earlier instantiation steps produced. The
`resource.drop` trampoline invokes the destructor synchronously with the dropped
entry's rep as its single `i32` argument, which matches the destructor's
declared signature.

A fresh resource type identity is minted per instantiation. An attempt to lower
a handle from one instance into another surfaces the unregistered-resource-type
ABI cause of [PDD009].

## Error Model Growth

No new variants are introduced. The locally-defined path reaches the
handle-specific ABI causes of [PDD009] (invalid handle, unregistered resource
type) and the instantiation variant of [PDD007] when the runtime layer refuses
the destructor, for example because its signature is not `(func (param i32))`.

## User Stories

A developer authors a component that owns its own resource type and wants the
polyfill to honor the in-binary destructor.

> The developer compiles a component that declares
> `(type (resource (rep i32) (dtor (func $f))))` and exports a function that
> takes ownership of a handle. Instantiation succeeds. Calling the export with a
> polyfill-minted handle drives the destructor exactly once. The host code never
> names the destructor.

A contributor needs two instances of one component to stay isolated under
guest-defined resources.

> The contributor instantiates the component twice, mints a handle through one
> instance, and attempts to lift it through the other. The lift surfaces the
> unregistered-resource-type ABI cause.

## References

- [PDD000], the product overview.
- [PDD003], the compatibility outlook.
- [PDD005], the foundations and posture.
- [PDD006], component parsing. This PDD extends its translation plan.
- [PDD007], linking and instantiation.
- [PDD008], the Canonical ABI and host functions.
- [PDD009], resources. This PDD mirrors its imported-resource path and shares
  its handle table.
- [Wasmtime], whose resource initialization step the polyfill's plan projects
  from.
- [Explainer], the Component Model design document.

[PDD000]: ./PDD000%20Wasm%20Component%20Model%20Polyfill.md
[PDD003]: ./PDD003%20Compatibility%20Outlook.md
[PDD005]: ./PDD005%20Library%20Foundations.md
[PDD006]: ./PDD006%20Component%20Parsing.md
[PDD007]: ./PDD007%20Linking%20and%20Instantiation.md
[PDD008]: ./PDD008%20Canonical%20ABI%20and%20Host%20Functions.md
[PDD009]: ./PDD009%20Resources.md
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
[Explainer]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md
