//! The polyfill's compilation context.
//!
//! See [PDD005] for the design intent: a thin newtype over
//! [`wasm_runtime_layer::Engine`] that hides the runtime-layer backend
//! behind a cfg-selected alias and is the moral equivalent of
//! [Wasmtime]'s `wasmtime::component::Engine`.
//!
//! [PDD005]: ../../../../design/PDD005%20Library%20Foundations.md
//! [Wasmtime]: https://github.com/bytecodealliance/wasmtime

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
