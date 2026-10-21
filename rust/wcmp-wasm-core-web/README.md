# wcmp-wasm-core-web

The browser backend of the runtime layer of the Wasm Component Model Polyfill.
It runs core WebAssembly through the browser's WebAssembly JavaScript API, by
way of `js-sys` and `wasm-bindgen`. It never makes a function from a string of
source, so it runs under a content security policy without `unsafe-eval`. The
JavaScript API exists only in the browser, so on every other target this crate
is empty.

See [`wcmp`](https://docs.rs/wcmp) for more context.
