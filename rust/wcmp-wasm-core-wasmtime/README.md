# wcmp-wasm-core-wasmtime

The Wasmtime backend of the runtime layer of the Wasm Component Model Polyfill.
It runs core WebAssembly on Wasmtime. It is the control backend, which runs
beside the other backends so that their results can be compared with Wasmtime's.
Wasmtime does not run in the browser, so on `wasm32` this crate is empty.

See [`wcmp`](https://docs.rs/wcmp) for more context.
