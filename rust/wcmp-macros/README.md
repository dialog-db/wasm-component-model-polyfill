# wcmp-macros

Procedural macros for the Wasm Component Model Polyfill. `wcmp` re-exports the
`ComponentType`, `Lift`, and `Lower` derives and the `flags!` macro, which
define typed component values as Wasmtime's do. The `wasm!` and `component!`
macros assemble WebAssembly Text Format into bytes at compile time. The `test`
and `bench` attributes serve the polyfill's own test and benchmark suites.

See [`wcmp`](https://docs.rs/wcmp) for more context.
