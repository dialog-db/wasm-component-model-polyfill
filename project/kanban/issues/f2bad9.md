---
id: f2bad9
title: bindgen with maps interfaces, packages, and resources
type: feature
blocked_by: [1cc7ae]
labels: [PDD028, bindgen]
created: 2026-10-17T02:55:52Z
disposition: accepted
disposition_at: 2026-10-21T06:27:09Z
---

## What to build

PDD028, "Resources and `with`". The `with` option at each of Wasmtime's granularities (`lib.rs:1128-1360` and `1862-1873` at `v49.0.0-rc.1`).

- **Keys.** An interface, a package, a namespace, a version of a package, or one type. Lookup goes from the most specific key to the least. A key that matches nothing fails the compile. A path carries no generic parameters.
- **Interfaces and packages.** A mapped interface or package emits nothing, and the generated code uses the mapped module's traits and types. The mapped module must come from `wcmp::bindgen!`; document that a Wasmtime-generated module does not compile here.
- **Resources.** A mapped resource becomes `pub use path as Name` in place of the empty enum. The handle is `Resource<Name>`, its identity `ResourceType::host::<Name>()`, and a host pushes its own value into a `ResourceTable` and returns the handle with no conversion.

## Acceptance criteria
- [x] `with` maps a resource to a type of a test host core, and the generated trait methods take and return `Resource<CoreType>`, on both targets.
- [x] `with` maps an interface: a second invocation maps an interface to the module that a first invocation generated, emits no module for it, and its world links with the first module's traits.
- [x] A `with` key that matches no WIT item fails the compile.
- [x] A more specific key wins over a less specific one (an interface key over its package key).
- [x] `lint` passes, and `tests all` is green on both targets with the conformance summary unchanged.


## Delivery notes

Delivered on the host in `feat(bindgen): Generate exports, async calls, resources, with, and errors` (with 9723b5 in `fix(linker): Link a used resource with no registration of its own` before it).

- A `with` path is brought in at the root as `__with_nameN` (keys sorted), and a mapped interface's module is `pub use <root>::__with_nameN::*;`, as in Wasmtime. A mapped resource is `pub use <root>::__with_nameN as Name;`. The macro's documentation states that a mapped module must be one `wcmp::bindgen!` wrote.
- A package key also matches the package's exported interfaces, which live under `exports`; the most-specific test maps the export by its own key, as a Wasmtime host would have to.
- Tests: `rust/wcmp-bindgen-tests/tests/with.rs` and a refusal unit test.
