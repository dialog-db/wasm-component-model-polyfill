# Resources

[PDD006], [PDD007], and [PDD008] together brought the polyfill's
synchronous baseline to within one feature of completion: components
parse, link, instantiate, and round-trip values through the canonical
ABI for every baseline valtype except the handle valtypes. This
document closes the gap. It introduces the polyfill's handle table,
host-resource registration with sync destructors, and lift/lower for
`own<T>` and `borrow<T>`, completing the synchronous baseline.

This PDD inherits both boundaries [PDD006] established: the
synchronous baseline (defined in
[PDD006 §The Synchronous Baseline][pdd006-synchronous-baseline]) and
the parity-per-PDD discipline (expressed in
[PDD006 §Web Parity Per PDD][pdd006-web-parity-per-pdd]). Neither is
restated here.

## Goals

- A developer can declare a host-owned resource type with a sync
  destructor against a `LinkerInstance<'_, T>`, hand handles to guest
  code, and observe the destructor run exactly once when the guest
  drops its handle.
- A developer can call a component export whose signature uses
  `own<T>` or `borrow<T>` and the canonical-ABI round-trip preserves
  the canonical ABI's runtime-state rules — no aliasing, deterministic
  reuse — sufficient to support the resource-handle observations the
  baseline tests make.
- The corresponding test in `tests/baseline_linking.rs` executes under
  both `test:native:*` and `test:web:*` without the `#[ignore]`
  attribute it currently carries:
  `it_defines_a_host_resource_with_a_sync_destructor`.
- The un-stubbed test runs unconditionally on every supported target
  per [PDD006 §Web Parity Per PDD][pdd006-web-parity-per-pdd] — no
  `#[ignore]`, no target gate.
- The polyfill's resource-handle representation does not directly
  expose [`wasm_runtime_layer`] or any upstream component-layer type.
- The single `wcmp::Error` enum grows additively as needed; in
  practice this PDD extends [PDD008]'s ABI variant with handle-
  specific failure modes rather than introducing a new top-level
  variant.

## Non-goals

- Async resource destructors. Per
  [PDD006 §The Synchronous Baseline][pdd006-synchronous-baseline]
  and [PDD003]'s checklist, async destructors are async-tier work.
- Cross-component resource handle transfer trampolines. Async-tier.
- The generalised handle-table extension to `future<T>` and
  `stream<T>`. Async-tier.

## The Resource Surface

`LinkerInstance<'_, T>` gains a third registration mode (in addition
to the two [PDD008] introduced): host-resource registration. The host
declares a resource type — a polyfill-owned representation carrying
the host's Rust type identity — and a sync destructor closure. The
destructor takes a borrow of the store's host data and the host's
representation of the resource, and runs exactly once when a
guest-held handle to the resource is dropped.

The resource type is exposed through the polyfill's own surface; it
does not name any upstream type. Its identity threads through
[PDD006]'s `ValueType` shapes (which reserve a slot for resource type
identity) and through [PDD008]'s `Val` family (whose handle-bearing
cases this PDD's lift/lower fills in).

## The Handle Table

The polyfill maintains a handle table behind its resource
representation. The table's index allocation and reuse semantics
follow the canonical ABI's runtime-state rules: indices are
non-aliasing while live, and reuse of a freed index is deterministic
within a single store. The table is per-store: every `Store<T>`
carries one, threaded through the runtime substrate via the
crate-private accessors [PDD005] established. The table is not
exposed in the public API; consumers interact with handles only
through the `Val` family and the typed export-call surface.

## Lift and Lower for Handle Valtypes

`own<T>` and `borrow<T>` lift and lower against the handle table:
lifting a guest-produced handle into a host `Val` resolves the index
to the host representation; lowering a host `Val` carrying a handle
produces the index the guest sees. Borrows are tracked for the
duration of the host call and released at return; ownership transfer
on `own<T>` follows the canonical ABI's discipline, including the
guest-trap that subsequent guest access to a transferred handle
incurs.

The lift/lower implementation extends [PDD008]'s; the public surface
of [PDD008]'s `Val` and lift/lower entry points is preserved
additively.

## Error Model Growth

`wcmp::Error::Abi` ([PDD008]) gains handle-specific failure modes:
attempting to lift an out-of-range handle, transferring an `own<T>`
to a host that has not registered the corresponding resource type, or
any other failure the canonical ABI's runtime-state rules surface as
a trap. The variant's payload grows additively to identify the
failure mode.

[PDD005]'s note about [`anyhow::Error`] in `#[source]` fields applies
unchanged.

## User Stories

**As a developer adopting the polyfill**, I want to hand a guest a
handle to a host-owned resource and observe my sync destructor run
when the guest drops the handle, so that the polyfill is sufficient
for backing a host that exposes resources to component guests.

> The developer registers a host resource type with a sync destructor
> against a `LinkerInstance`, instantiates a component that takes
> ownership of a handle to the resource, lets the guest drop the
> handle, and observes the destructor run exactly once.

**As a contributor opening the first async-tier PDD**, I want the
synchronous-baseline resource surface already in tree, complete on
every supported target, and stable — so that my PDD extends a
finished foundation rather than chasing residual platform divergence.

> The contributor reads this document, sees the closing of the
> synchronous baseline on both `test:native:*` and `test:web:*`, and
> starts the async-tier work knowing the synchronous handle-table
> machinery and host-resource registration mode are already in place
> and exercised on every target.

**As a reviewer evaluating an in-progress PDD**, I want the scope
of each PDD to be readable.

> The reviewer reads the goals and non-goals, confirms that the PR
> un-stubs only the named test — green on both `test:native:*` and
> `test:web:*`, with no target gate — that no async-destructor
> surface sneaks in, and that the public API additions are limited
> to the host-resource registration mode and the handle-bearing
> lift/lower paths.

## References

- [PDD000] — product overview.
- [PDD001] — development environment.
- [PDD002] — ecosystem foundation.
- [PDD003] — compatibility outlook. This PDD covers the resource
  rows of "Component Type System" and "Canonical ABI", and the
  host-resource row of "Linking, Instantiation, and Host
  Integration".
- [PDD004] — test macros.
- [PDD005] — library foundations.
- [PDD006] — component parsing.
- [PDD007] — linking and instantiation.
- [PDD008] — canonical ABI and host functions; the PDD this one
  extends.
- [`wasm_runtime_layer`] — the runtime substrate.
- [`wasm_component_layer`] — prior art only.
- [Wasmtime] — the reference runtime.
- [Explainer] — the canonical Component Model design document.
- [CanonicalABI] — the canonical ABI rules this PDD's handle-table
  semantics realise.
- [Subtyping] — structural-equality rules.
- [`thiserror`] — the derive used by the polyfill's error enum.
- [`anyhow::Error`] — the source-capture compromise [PDD005] notes.

[PDD000]: ./PDD000%20Wasm%20Component%20Model%20Polyfill.md
[PDD001]: ./PDD001%20Development%20Environment.md
[PDD002]: ./PDD002%20Ecosystem%20Foundation.md
[PDD003]: ./PDD003%20Compatibility%20Outlook.md
[PDD004]: ./PDD004%20Test%20Macros.md
[PDD005]: ./PDD005%20Library%20Foundations.md
[pdd005-implementation-posture]: ./PDD005%20Library%20Foundations.md#implementation-posture
[PDD006]: ./PDD006%20Component%20Parsing.md
[pdd006-synchronous-baseline]: ./PDD006%20Component%20Parsing.md#the-synchronous-baseline
[pdd006-web-parity-per-pdd]: ./PDD006%20Component%20Parsing.md#web-parity-per-pdd
[pdd006-implementation-posture]: ./PDD006%20Component%20Parsing.md#implementation-posture
[PDD007]: ./PDD007%20Linking%20and%20Instantiation.md
[PDD008]: ./PDD008%20Canonical%20ABI%20and%20Host%20Functions.md
[`wasm_runtime_layer`]: https://github.com/DouglasDwyer/wasm_runtime_layer
[`wasm_component_layer`]: https://github.com/DouglasDwyer/wasm_component_layer
[`thiserror`]: https://docs.rs/thiserror
[`anyhow::Error`]: https://docs.rs/anyhow
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
[Explainer]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md
[CanonicalABI]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md
[Subtyping]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Subtyping.md
