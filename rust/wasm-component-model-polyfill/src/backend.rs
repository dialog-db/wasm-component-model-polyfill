//! Selection of the runtime-layer backend the polyfill is built
//! against.
//!
//! The choice is target-conditional and an implementation detail —
//! the public API never names the backend directly. Native targets
//! use the Wasmtime backend (which participates only as a core-Wasm
//! engine; the polyfill's component-level work is implemented above
//! the runtime layer, not delegated to `wasmtime::component`).
//! `wasm32-unknown-unknown` uses the browser's native `WebAssembly`
//! interface via the `js_wasm` backend. This module is workspace-
//! private; nothing inside it is re-exported by `lib.rs`.

#[cfg(not(target_arch = "wasm32"))]
pub type Backend = wasmtime_runtime_layer::Engine;

#[cfg(target_arch = "wasm32")]
pub type Backend = js_wasm_runtime_layer::Engine;
