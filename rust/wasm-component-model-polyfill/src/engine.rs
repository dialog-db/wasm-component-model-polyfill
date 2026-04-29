//! The polyfill's compilation context.
//!
//! `Engine` is a thin wrapper over the runtime-layer engine selected
//! at compile time for the host platform. It hides the backend type
//! from the polyfill's public API and is the moral equivalent of
//! `wasmtime::component::Engine` — the type from which component
//! compilation will hang in subsequent slices.

use crate::backend::Backend;
use crate::error::Result;

/// The polyfill's compilation context.
///
/// An `Engine` carries the configuration shared by every component the
/// polyfill compiles and instantiates. It is cheap to clone — internal
/// state is shared — and is constructed without arguments via
/// [`Engine::new`].
///
/// Later slices will hang component compilation off this type; for now
/// the public surface is just construction.
#[derive(Clone)]
pub struct Engine {
    inner: wasm_runtime_layer::Engine<Backend>,
}

impl Engine {
    /// Construct an `Engine` over a default-configured backend.
    ///
    /// The return type is [`Result`] for forward compatibility:
    /// today, both supported backends are infallibly default-
    /// constructible, but later slices will accept configuration that
    /// can fail at construction time.
    #[allow(clippy::unnecessary_wraps)]
    pub fn new() -> Result<Self> {
        Ok(Self {
            inner: wasm_runtime_layer::Engine::new(Backend::default()),
        })
    }

    /// Borrow the wrapped runtime-layer engine.
    ///
    /// This accessor is workspace-internal and is the seam through
    /// which later slices reach into the runtime layer; it is not
    /// re-exported by `lib.rs` and never reaches downstream consumers.
    pub fn inner(&self) -> &wasm_runtime_layer::Engine<Backend> {
        &self.inner
    }
}
