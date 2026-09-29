//! The seam between the polyfill and the runtime layer it runs core
//! WebAssembly through.
//!
//! This module is the one place in the polyfill that names the
//! runtime layer's crates. Every other module reaches a runtime-layer
//! type through the names below, so pointing the polyfill at another
//! runtime layer changes this file alone. A flake check fails when a
//! source file of the crate other than this one names a runtime-layer
//! crate. The module is workspace-private; nothing inside it is
//! re-exported by `lib.rs`, and no public signature names a type it
//! re-exports.
//!
//! The choice of backend is target-conditional and an implementation
//! detail — the public API never names the backend directly. Native
//! targets use the Wasmtime backend (which participates only as a
//! core-Wasm engine; the polyfill's component-level work is
//! implemented above the runtime layer, not delegated to
//! `wasmtime::component`). `wasm32-unknown-unknown` uses the
//! browser's native `WebAssembly` interface via the `js_wasm` backend.
//!
//! Both backends call a host function at any depth: a host function
//! already on the stack is called again with no more ceremony than
//! any other, and the arguments and results of each call belong to
//! that call alone.

use crate::error::{Error, InstantiationError};

pub use wasm_runtime_layer::{
    AsContextMut, Engine, Extern, ExternType, Func, FuncType, Global, Imports, Instance, Memory,
    Module, RefType, Store, StoreContextMut, Table, Val, ValType,
};

/// The type of a memory, which only the tests make one from.
#[cfg(test)]
pub use wasm_runtime_layer::MemoryType;

/// The backend's own function and the backend-level extern and value
/// the runtime layer's types convert to and from. The browser target
/// reaches below the runtime layer's generic types for what only its
/// backend has: a function that suspends through JavaScript Promise
/// Integration.
#[cfg(target_arch = "wasm32")]
pub use js_wasm_runtime_layer::Func as BackendFunc;
#[cfg(target_arch = "wasm32")]
pub use wasm_runtime_layer::backend::{Extern as BackendExtern, Val as BackendVal};

#[cfg(not(target_arch = "wasm32"))]
pub type Backend = wasmtime_runtime_layer::Engine;

#[cfg(target_arch = "wasm32")]
pub type Backend = js_wasm_runtime_layer::Engine;

/// The polyfill error a runtime-layer failure of a guest call
/// becomes. Workspace-internal.
pub fn substrate_failure(error: anyhow::Error) -> Error {
    Error::from(InstantiationError::SubstrateFailure(error))
}
