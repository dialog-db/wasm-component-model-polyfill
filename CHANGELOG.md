# Changelog

This file records the notable changes to each release of the published crates.
Every published crate carries the same version. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the versions
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.1] - 2026-10-21

### Performance

- `wcmp-wasm-core-web` no longer decodes every function body of a module to
  learn whether it waits or notifies. A module whose `target_features` section
  names no atomics cannot, so the browser backend skips its bodies. A module
  without the section is read as before. This removes about 70 of 105 ms from
  the load of a 10 MB module in Chrome. By Irakli Gozalishvili in
  [#1](https://github.com/dialog-db/wasm-component-model-polyfill/pull/1).

[0.1.1]: https://github.com/dialog-db/wasm-component-model-polyfill/releases/tag/v0.1.1
