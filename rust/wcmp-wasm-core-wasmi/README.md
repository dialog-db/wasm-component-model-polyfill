# wcmp-wasm-core-wasmi

The Wasmi backend of the runtime layer of the Wasm Component Model Polyfill. It
runs core WebAssembly on the Wasmi interpreter, which is small and quick to
start. It is the practical backend for a native host. Wasmi builds for every
target, `wasm32` included, so this crate does too.

See [`wcmp`](https://docs.rs/wcmp) for more context.
