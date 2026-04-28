//! Selection of the [`wasm_runtime_layer`] backend that the polyfill's
//! foundational types are built against.
//!
//! The choice is target-conditional and an implementation detail of the
//! polyfill — the public API never names the backend directly. Per
//! [PDD002] and [PDD005], native targets use the Wasmtime backend and
//! `wasm32-unknown-unknown` uses the browser's native `WebAssembly`
//! interface via `js_wasm_runtime_layer`. This module is itself
//! workspace-private; nothing inside it is re-exported by `lib.rs`.
//!
//! [PDD002]: ../../../../design/PDD002%20Ecosystem%20Foundation.md
//! [PDD005]: ../../../../design/PDD005%20Library%20Foundations.md

#[cfg(not(target_arch = "wasm32"))]
pub type Backend = wasmtime_runtime_layer::Engine;

#[cfg(target_arch = "wasm32")]
pub type Backend = js_wasm_runtime_layer::Engine;
