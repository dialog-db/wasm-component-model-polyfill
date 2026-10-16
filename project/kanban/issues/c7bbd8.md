---
id: c7bbd8
title: Error::Host carries a host's own error and its source chain
type: feature
blocked_by: []
labels: [PDD028, bindgen]
created: 2026-10-17T02:55:52Z
---

## What to build

PDD028, "Errors", with the revisions it makes to PDD005 and PDD008. A host reports an error of its own with `Error::Host`, and the error reaches the host's call with its source chain. No macro work.

- **`Error::Host`.** A new variant whose source is an `anyhow::Error`. `Error` implements `From<anyhow::Error>`. When the `anyhow::Error` already holds a polyfill `Error`, the conversion takes that `Error` out instead of wrapping it, so a variant passed on with `?` comes back unchanged.
- **Travel.** A host function that returns `Err` traps the guest, and the trap poisons the store (PDD023). The host's call returns the same `Error`. An `Error::Host` keeps its source chain, and the host can downcast the source to its own type.
- **Browser.** A `JsValue` is not `Send`. Document on the variant that a browser host turns a JavaScript exception into a message before it wraps it in an `anyhow::Error`.
- Check the existing host-error paths (`HostCall` results, host task results, `Error::Trap` and `Error::Scheduler`) and route a host's own error through `Error::Host` where today it is flattened to a message.

## Acceptance criteria
- [ ] A host function returns `Error::Host` with a source of a host type. The guest traps. The host's call returns an error whose source downcasts to the host type. The store is poisoned. On both targets.
- [ ] A host function that returns `Error::ResourceTable` through `?` (directly and through an `anyhow::Error` that holds it) makes the host's call return `Error::ResourceTable`.
- [ ] `anyhow::Context` on a host error keeps the whole chain, and each layer is visible from the host's call.
- [ ] `lint` passes, and `tests all` is green on both targets with the conformance summary unchanged.

