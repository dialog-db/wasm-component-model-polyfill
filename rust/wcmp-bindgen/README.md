# wcmp-bindgen

The `bindgen!` macro of the Wasm Component Model Polyfill. The macro reads WIT
and writes the Rust code that connects a host to a component. It keeps the names
and the workflow of Wasmtime's `wasmtime::component::bindgen!`, and the code it
writes uses the polyfill's own API. It runs at compile time, so no browser
binary carries the WIT parser. Use it as `wcmp::bindgen!`, which `wcmp`
re-exports.

See [`wcmp`](https://docs.rs/wcmp) for more context.
