//! Compiling one core module against the engine.
//!
//! The runtime layer's `Module::new` is synchronous. On native that
//! is the whole story. In the browser the synchronous
//! `WebAssembly.Module` constructor is refused on the main thread
//! above a size limit, so the backend first compiles the bytes with
//! `WebAssembly.compile`, the browser's asynchronous path, and the
//! runtime layer's constructor then takes the compiled module.

use wasm_runtime_layer::Module as RuntimeModule;

use crate::engine::Engine;
use crate::error::{Error, InstantiationError, Result};
use crate::internal::EngineInternal;

/// Compile `bytes`, one core module of a component, against `engine`.
/// The translator already validated the module; a failure here means
/// the runtime substrate refused a valid module.
pub async fn compile_module(engine: &Engine, bytes: &[u8]) -> Result<RuntimeModule> {
    #[cfg(target_arch = "wasm32")]
    engine
        .inner()
        .clone()
        .into_backend()
        .precompile(bytes)
        .await
        .map_err(InstantiationError::SubstrateFailure)
        .map_err(Error::from)?;
    RuntimeModule::new(engine.inner(), bytes)
        .map_err(InstantiationError::SubstrateFailure)
        .map_err(Error::from)
}
