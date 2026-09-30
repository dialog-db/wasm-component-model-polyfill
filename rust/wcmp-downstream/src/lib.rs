//! A consumer of the polyfill from outside its workspace. It makes an engine
//! over the backend of its target, as a host does, so a build of this crate
//! links the polyfill and the backend together.

use wcmp::{Engine, Result};

/// An engine over the backend of the target: Wasmtime natively, and the
/// browser's own engine under `wasm32`.
pub fn engine() -> Result<Engine> {
    #[cfg(not(target_arch = "wasm32"))]
    let backend =
        wcmp_wasm_core_wasmtime::Wasmtime::new().map_err(|error| wcmp::Error::Internal {
            message: format!("Wasmtime makes no engine: {error}"),
        })?;
    #[cfg(target_arch = "wasm32")]
    let backend = wcmp_wasm_core_web::Web::new();
    Engine::with_backend(backend)
}
