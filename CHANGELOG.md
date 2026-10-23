# Changelog

This file records the notable changes to each release of the published crates.
Every published crate carries the same version. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the versions
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Performance

- `context.get` and `context.set` no longer call the host for a slot that is
  loaded. Each store keeps the current thread's slots in the globals of a
  generated module, and writes them back to the thread's record at each crossing
  from guest code to the host. A Rust component built for
  `wasm32-wasip3` reads its stack pointer this way around most of its
  functions. On one such kernel, a call-heavy workload on Wasmtime went from
  1414 ms to 187 ms. In
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

[Unreleased]: https://github.com/dialog-db/wasm-component-model-polyfill/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/dialog-db/wasm-component-model-polyfill/releases/tag/v0.1.1
