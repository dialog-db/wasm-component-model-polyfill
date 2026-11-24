# Changelog

This file records the notable changes to each release of the published crates.
Every published crate carries the same version. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the versions
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- A host can read the current size of each memory of a component's core
  instances. `Instance::core_memories` lists each memory that a core instance
  of the component defines and exports. `Memory::size` gives its size in pages
  and `Memory::data_size` in bytes, as `wasmtime::Memory` does. A read runs no
  guest code, so it works after a trap. For example, a host that caps the
  memory of a guest can tell whether the guest grew its memory to the cap
  before it trapped.

### Fixed

- The published crates no longer require exactly `wasm-bindgen` 0.2.126. They
  take 0.2.126 or any later 0.2 release, so a crate that also depends on a
  newer `wasm-bindgen`, `js-sys`, or `web-sys`, wgpu 30 among them, can use the
  polyfill. Reported by Mistale in
  [#5](https://github.com/dialog-db/wasm-component-model-polyfill/issues/5).
- The repository checks out on Windows, so it serves as a Cargo git dependency
  there. A design document's file name held a colon, which Windows refuses.
  Reported by Mistale in
  [#5](https://github.com/dialog-db/wasm-component-model-polyfill/issues/5).

## [0.1.2] - 2026-10-25

### Performance

- `context.get` and `context.set` no longer call the host for a slot that is
  loaded. Each store keeps the current thread's slots in the globals of a
  generated module, and writes them back to the thread's record at the crossings
  between guest code and the host. A Rust component built for
  `wasm32-wasip3` reads its stack pointer this way around most of its
  functions. On one such kernel, a call-heavy workload on Wasmtime went from
  1414 ms to 187 ms. By Irakli Gozalishvili in
  [#3](https://github.com/dialog-db/wasm-component-model-polyfill/pull/3).

### Added

- `wcmp-wasm-core` stores take a call hook, as Wasmtime stores do.
  `Store::call_hook` and `StoreContextMut::call_hook` set it, and the engine
  calls it with a `CallHook` at each crossing between the host and guest code.

## [0.1.1] - 2026-10-21

### Performance

- `wcmp-wasm-core-web` no longer decodes every function body of a module to
  learn whether it waits or notifies. A module whose `target_features` section
  names no atomics cannot, so the browser backend skips its bodies. A module
  without the section is read as before. This removes about 70 of 105 ms from
  the load of a 10 MB module in Chrome. By Irakli Gozalishvili in
  [#1](https://github.com/dialog-db/wasm-component-model-polyfill/pull/1).

[Unreleased]: https://github.com/dialog-db/wasm-component-model-polyfill/compare/v0.1.2...HEAD
[0.1.2]: https://github.com/dialog-db/wasm-component-model-polyfill/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/dialog-db/wasm-component-model-polyfill/releases/tag/v0.1.1
