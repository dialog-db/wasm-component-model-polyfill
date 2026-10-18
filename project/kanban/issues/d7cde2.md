---
id: d7cde2
title: A layer on the linker can reach the definition it shadows
type: feature
blocked_by: []
labels: [webgpu-spike, linker]
created: 2026-10-18T07:21:10Z
---

## What to build

A host that layers a definition over another on the linker can reach the one it
replaces.

`Linker::allow_shadowing` lets a later registration replace an earlier one, and
a host can use it to put a layer around an imported interface: refuse some
calls, change some results, and pass the rest on. But the layer cannot reach
the definition it replaced, because the linker gives back nothing it holds.

The `wasi:webgpu` spike (`~/Git/github.com/cdata/wasi-webgpu-test`) built every
extension of its port this way: a profile layer over `request-device` and the
adapter's limits and features, a canvas-context layer, `lost`, and a fix for
`on-submitted-work-done` (`rust/spike-webgpu/src/`). To call through, each layer
calls the port's trait methods directly, so the port had to make a private type
public (`HasWasiWebGpuCtx`), an edit it would not otherwise need.

Decide the shape, for example one of:
- a lookup of the item a linker instance holds under a name, which a layer
  captures before it shadows it;
- a wrapping registration that hands the layer the definition it wraps, to call
  or not.

Wasmtime's component `Linker` has neither, so this is an extension, and the
design should say so.

## Acceptance criteria
- [ ] A layer registered over an import can call the definition it replaced,
      synchronous and concurrent (`func_wrap_concurrent`) alike, on both targets.
- [ ] A layer over a typed `bindgen!` import works without reaching into the
      generated trait's types.
- [ ] The owner has recorded whether this extends Wasmtime's API, and how it is
      named.
- [ ] `lint` passes, and `tests all` is green on both targets with the
      conformance summary unchanged.

## Review notes

