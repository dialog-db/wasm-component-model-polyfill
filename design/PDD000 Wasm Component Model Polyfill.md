# Wasm Component Model Polyfill

Wasm Component Model Polyfill is a [polyfill] library that brings the
[Wasm Component Model] to web browsers where only [Wasm Core] is presently
supported.

At the time of its inception, the latest Wasm Component Model proposal is
called WASI Preview 3 (aka wasip3). All featureful work assumes that wasip3
is the target version of the Wasm Component Model until a future design
document specifies otherwise.

## Use Case

Wasm Components enable dynamic linking and code re-use in Wasm environments.
However, at this time it is not possible to compile, link and instantiate
Wasm Components in a web browser context. The Wasm Component Model Polyfill
enables compiling, linking and instantiating Wasm Components in web browsers
by emulating the counterpart [Wasmtime] APIs for doing the same and packaging
the emulation layer as library that can be used in apps that target web
browsers. Additionally, the library progressively falls back to [Wasmtime]
under the hood when running outside of the browser, which enables developers
to use a single API and mental model when targeting both platforms.

[polyfill]: https://developer.mozilla.org/en-US/docs/Glossary/Polyfill
[Wasm Core]: https://www.w3.org/TR/wasm-core-2/
[Wasm Component Model]: https://github.com/WebAssembly/component-model
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
