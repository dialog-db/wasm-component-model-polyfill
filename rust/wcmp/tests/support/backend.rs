//! The backend the polyfill's integration tests hand their engines.
//!
//! The polyfill has no backend of its own, so a test names one:
//! Wasmtime natively, and the browser's engine in the browser.

/// The Wasmtime backend of the runtime layer.
#[cfg(not(target_arch = "wasm32"))]
pub fn backend() -> wcmp_wasm_core_wasmtime::Wasmtime {
    wcmp_wasm_core_wasmtime::Wasmtime::new().expect("Wasmtime makes an engine")
}

/// The browser backend of the runtime layer.
#[cfg(target_arch = "wasm32")]
pub fn backend() -> wcmp_wasm_core_web::Web {
    wcmp_wasm_core_web::Web::new()
}
