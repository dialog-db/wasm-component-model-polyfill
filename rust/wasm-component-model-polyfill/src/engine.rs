//! The polyfill's compilation context.
//!
//! `Engine` is a thin wrapper over the runtime-layer engine selected
//! at compile time for the host platform. It hides the backend type
//! from the polyfill's public API and is the moral equivalent of
//! `wasmtime::component::Engine` — the type from which component
//! compilation hangs. The runtime-layer engine sees only core
//! WebAssembly; component-level work is layered on top of the
//! runtime-layer's generic abstractions in later modules and is not
//! delegated to a backend's component runtime.

use crate::backend::Backend;
use crate::engine_config::EngineConfig;
use crate::error::Result;

/// The polyfill's compilation context.
///
/// An `Engine` carries the configuration shared by every component the
/// polyfill compiles and instantiates. It is cheap to clone — internal
/// state is shared — and is constructed without arguments via
/// [`Engine::new`].
///
/// The engine carries the [`EngineConfig`] every component it
/// translates is validated with.
#[derive(Clone)]
pub struct Engine {
    inner: wasm_runtime_layer::Engine<Backend>,
    config: EngineConfig,
}

impl Engine {
    /// Construct an `Engine` over a default-configured backend, with
    /// the default [`EngineConfig`].
    ///
    /// The return type is [`Result`] for forward compatibility:
    /// today, both supported backends are infallibly default-
    /// constructible, but later work will accept configuration that
    /// can fail at construction time.
    pub fn new() -> Result<Self> {
        Self::with_config(&EngineConfig::default())
    }

    /// Construct an `Engine` from `config`, the polyfill's analogue
    /// to building a Wasmtime engine from a `Config`.
    #[allow(clippy::unnecessary_wraps)]
    pub fn with_config(config: &EngineConfig) -> Result<Self> {
        Ok(Self {
            inner: wasm_runtime_layer::Engine::new(Backend::default()),
            config: config.clone(),
        })
    }

    /// The configuration this engine was built from.
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// Borrow the wrapped runtime-layer engine.
    ///
    /// This accessor is workspace-internal and is the seam through
    /// which later work reaches into the runtime layer; it is not
    /// re-exported by `lib.rs` and never reaches downstream consumers.
    pub fn inner(&self) -> &wasm_runtime_layer::Engine<Backend> {
        &self.inner
    }
}
