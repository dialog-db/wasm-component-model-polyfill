# wcmp-wasm-core

The runtime layer of the Wasm Component Model Polyfill. It is one trait over
core WebAssembly, with the types, capabilities, trap kinds, and errors that
every backend shares. The types follow Wasmtime's core API in name and in
ownership. Each backend is a crate of its own and runs core WebAssembly on one
engine. `wcmp` runs the Component Model on top of this layer.

See [`wcmp`](https://docs.rs/wcmp) for more context.
