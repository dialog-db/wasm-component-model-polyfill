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
//!
//! Both backends call a host function at any depth: a host function
//! already on the stack is called again with no more ceremony than
//! any other, and the arguments and results of each call belong to
//! that call alone.

use crate::error::{Error, InstantiationError};

#[cfg(not(target_arch = "wasm32"))]
pub type Backend = wasmtime_runtime_layer::Engine;

#[cfg(target_arch = "wasm32")]
pub type Backend = js_wasm_runtime_layer::Engine;

/// The polyfill error a runtime-layer failure of a guest call
/// becomes. Workspace-internal.
pub fn substrate_failure(error: anyhow::Error) -> Error {
    Error::from(InstantiationError::SubstrateFailure(error))
}
