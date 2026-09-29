//! The Wasmi backend.

use core::fmt;

use wcmp_wasm_core::backend::{Backend, BackendModule, BackendStore, BoxFuture, StoreData};
use wcmp_wasm_core::{Capabilities, Capability, Error, Result};

use crate::module::WasmiModule;
use crate::refusal;
use crate::state::State;
use crate::store::WasmiStore;

/// The Wasmi backend of the runtime layer: the practical native backend.
///
/// A host hands one to
/// [`Engine::with_backend`](wcmp_wasm_core::Engine::with_backend). Each
/// value holds its own Wasmi engine, so two engines over two values of this
/// type share nothing.
pub struct Wasmi {
    engine: wasmi::Engine,
    capabilities: Capabilities,
}

impl Wasmi {
    /// The capabilities the backend declares: the features above Wasm 2.0
    /// that Wasmi implements.
    const FEATURES: [Capability; 4] = [
        Capability::MultiMemory,
        Capability::Memory64,
        Capability::TailCall,
        Capability::RelaxedSimd,
    ];

    /// A backend over a new Wasmi engine, with every feature the backend
    /// declares turned on, and host suspension, which Wasmi's resumable
    /// calls fill.
    pub fn new() -> Self {
        let mut config = wasmi::Config::default();
        config
            .wasm_multi_memory(true)
            .wasm_memory64(true)
            .wasm_tail_call(true)
            .wasm_simd(true)
            .wasm_relaxed_simd(true);
        Self {
            engine: wasmi::Engine::new(&config),
            capabilities: Capabilities::from_iter(Self::FEATURES).with(Capability::HostSuspension),
        }
    }
}

impl Default for Wasmi {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for Wasmi {
    fn capabilities(&self) -> Capabilities {
        self.capabilities
    }

    fn compile<'a>(&'a self, bytes: &'a [u8]) -> BoxFuture<'a, Result<Box<dyn BackendModule>>> {
        // Wasmi compiles synchronously, so the future is ready on its first
        // poll.
        Box::pin(core::future::ready(self.compile_sync(bytes)))
    }

    fn compile_sync(&self, bytes: &[u8]) -> Result<Box<dyn BackendModule>> {
        // Without its `wat` feature, Wasmi refuses the text format here, as
        // every other engine refuses it.
        let module = wasmi::Module::new(&self.engine, bytes).map_err(|error| {
            match refusal::missing_capability(self.capabilities, bytes) {
                Some(capability) => Error::Unsupported(capability),
                None => Error::Compile {
                    message: error.to_string(),
                },
            }
        })?;
        Ok(Box::new(WasmiModule::new(module, bytes)))
    }

    fn new_store(&self, data: StoreData) -> Result<Box<dyn BackendStore>> {
        Ok(Box::new(WasmiStore::new(wasmi::Store::new(
            &self.engine,
            State::new(data),
        ))))
    }
}

impl fmt::Debug for Wasmi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Wasmi")
            .field("capabilities", &self.capabilities)
            .finish_non_exhaustive()
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    fn it_refuses_the_text_format_with_a_compile_error() {
        let refused = Wasmi::new().compile_sync(b"(module)");
        assert!(
            matches!(refused, Err(Error::Compile { .. })),
            "{:?}",
            refused.err()
        );
    }
}
