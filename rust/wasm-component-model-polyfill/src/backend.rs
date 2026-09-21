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
//! # One divergence between the two backends
//!
//! A native engine calls a host function at any depth: a host
//! function already on the stack is called again with no more
//! ceremony than any other. The browser cannot. A host function
//! there is one JavaScript function object over one Rust closure,
//! and the arguments and the results buffer of a call belong to
//! that call alone, so a second call made while the first is still
//! running has nowhere to put them. The browser backend detects that
//! second call and refuses it; the message it carries and the
//! reasoning behind it are recorded in that backend's `PATCHES.md`.
//!
//! The polyfill reaches the divergence through nested turns. A
//! blocking built-in that finds no suspend provider runs turns of
//! the store from inside the lowered import the guest called, so
//! that import's host function is on the stack for as long as the
//! block lasts. Work those turns run is ordinary guest work and may
//! call any import it likes — including, on the web target, the very
//! import the block is inside. Such a call runs natively and fails
//! in the browser, and it is the one shape of component that does.
//!
//! [`substrate_failure`] is where the refusal becomes a polyfill
//! error: it answers the `ReentrantHostCall` scheduler cause, so the
//! failure reaches a host as a cause it can branch on rather than as
//! a backend string. Every call of a guest function goes through it.

use crate::error::{Error, InstantiationError};

#[cfg(not(target_arch = "wasm32"))]
pub type Backend = wasmtime_runtime_layer::Engine;

#[cfg(target_arch = "wasm32")]
pub type Backend = js_wasm_runtime_layer::Engine;

/// The polyfill error a runtime-layer failure of a guest call
/// becomes.
///
/// Every such failure is a substrate failure, with one exception:
/// the browser backend's refusal of a re-entrant host call, which
/// names a limitation of the target rather than anything the call
/// did, and which a host may want to tell apart from a guest that
/// trapped. The refusal travels as the backend's own error type, so
/// this reads it back off the error the call failed with. Workspace-
/// internal.
#[cfg(target_arch = "wasm32")]
pub fn substrate_failure(error: anyhow::Error) -> Error {
    if error
        .downcast_ref::<js_wasm_runtime_layer::ReentrantHostCall>()
        .is_some()
    {
        return Error::Scheduler(crate::error::SchedulerCause::ReentrantHostCall);
    }
    Error::from(InstantiationError::SubstrateFailure(error))
}

/// The polyfill error a runtime-layer failure of a guest call
/// becomes. See the browser definition for the one failure that is
/// not a substrate failure; the native backend cannot produce it,
/// because a native engine calls a host function at any depth.
/// Workspace-internal.
#[cfg(not(target_arch = "wasm32"))]
pub fn substrate_failure(error: anyhow::Error) -> Error {
    Error::from(InstantiationError::SubstrateFailure(error))
}
