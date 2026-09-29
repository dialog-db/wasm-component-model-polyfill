//! The browser backend.

use core::fmt;
use std::rc::Rc;

use js_sys::{Uint8Array, WebAssembly};
use wasm_bindgen::JsCast;
use wcmp_wasm_core::backend::{Backend, BackendModule, BackendStore, BoxFuture, StoreData};
use wcmp_wasm_core::{Capabilities, Capability, Result};

use crate::boundary::Boundary;
use crate::errors;
use crate::jspi::Jspi;
use crate::module::WebModule;
use crate::owner::Owner;
use crate::probes;
use crate::store::WebStore;
use crate::type_registry::TypeRegistry;

/// The browser backend of the runtime layer, over the WebAssembly
/// JavaScript API.
///
/// A host hands one to
/// [`Engine::with_backend`](wcmp_wasm_core::Engine::with_backend). The
/// backend probes the browser when it is made, and declares the
/// capabilities whose probe the browser accepts, and
/// [`host_suspension`](wcmp_wasm_core::Capability::HostSuspension) where
/// the browser has both functions of JavaScript Promise Integration. Two
/// values of this type
/// share nothing: each numbers its own concrete types.
pub struct Web {
    capabilities: Capabilities,
    jspi: Option<Rc<Jspi>>,
    types: Rc<TypeRegistry>,
}

impl Web {
    /// A backend over the browser this code runs in.
    ///
    /// It runs one small probe for each Wasm feature of the lexicon, and
    /// reads the two functions of JavaScript Promise Integration. A browser
    /// without a feature loads the backend and declares less.
    pub fn new() -> Self {
        let jspi = Jspi::read().map(Rc::new);
        Self {
            capabilities: capabilities(probes::capabilities(), jspi.is_some()),
            jspi,
            types: Rc::default(),
        }
    }

    /// Whether the browser has both functions of JavaScript Promise
    /// Integration, `WebAssembly.Suspending` and `WebAssembly.promising`.
    ///
    /// The backend declares
    /// [`host_suspension`](wcmp_wasm_core::Capability::HostSuspension)
    /// exactly where it does.
    pub fn has_jspi(&self) -> bool {
        self.jspi.is_some()
    }

    /// The module `module` that the browser compiled from `bytes`, with its
    /// boundary.
    fn module(&self, module: WebAssembly::Module, bytes: &[u8]) -> Result<Box<dyn BackendModule>> {
        let boundary = Boundary::read(bytes, &self.types)?;
        Ok(Box::new(WebModule::new(module, boundary)))
    }
}

impl Default for Web {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for Web {
    fn capabilities(&self) -> Capabilities {
        self.capabilities
    }

    fn compile<'a>(&'a self, bytes: &'a [u8]) -> BoxFuture<'a, Result<Box<dyn BackendModule>>> {
        Box::pin(async move {
            // `WebAssembly.compile`, so a module above the browser's limit
            // for a synchronous compile loads too.
            let module = WebAssembly::compile(&Uint8Array::from(bytes).into())
                .await
                .map_err(|error| errors::compile(&error))?
                .dyn_into::<WebAssembly::Module>()
                .map_err(|_| errors::backend("the compile gave no module"))?;
            self.module(module, bytes)
        })
    }

    fn compile_sync(&self, bytes: &[u8]) -> Result<Box<dyn BackendModule>> {
        // The browser refuses a synchronous compile of a module above its
        // limit on the main thread with a `RangeError`, which is a compile
        // error with the browser's message.
        let module = WebAssembly::Module::new(&Uint8Array::from(bytes).into())
            .map_err(|error| errors::compile(&error))?;
        self.module(module, bytes)
    }

    fn new_store(&self, data: StoreData) -> Result<Box<dyn BackendStore>> {
        Ok(Box::new(Owner::new(WebStore::new(
            data,
            self.types.clone(),
            self.jspi.clone(),
        ))))
    }
}

/// The capabilities of a backend whose probes found `probed`: those, and
/// `host_suspension` where the browser has both functions of JavaScript
/// Promise Integration.
fn capabilities(probed: Capabilities, jspi: bool) -> Capabilities {
    if jspi {
        probed.with(Capability::HostSuspension)
    } else {
        probed.without(Capability::HostSuspension)
    }
}

impl fmt::Debug for Web {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Web")
            .field("capabilities", &self.capabilities)
            .field("jspi", &self.jspi.is_some())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The browser's module inside `module`.
    fn browser_module(module: &dyn BackendModule) -> WebAssembly::Module {
        module
            .as_any()
            .downcast_ref::<WebModule>()
            .expect("the backend compiles its own modules")
            .module()
            .clone()
    }

    #[wcmp_macros::test]
    fn it_declares_host_suspension_only_with_both_functions_of_jspi() {
        let probed = Capabilities::empty().with(Capability::Gc);
        assert_eq!(
            capabilities(probed, true),
            probed.with(Capability::HostSuspension)
        );
        assert_eq!(capabilities(probed, false), probed);
    }

    #[wcmp_macros::test]
    async fn it_compiles_a_module_of_its_own_each_time() {
        let backend = Web::new();
        let bytes = wcmp_macros::wasm!(r#"(module (func (export "run")))"#);

        let first = backend.compile(bytes).await.expect("the module compiles");
        let second = backend.compile(bytes).await.expect("the module compiles");
        let third = backend.compile_sync(bytes).expect("the module compiles");

        let modules = [&first, &second, &third].map(|module| browser_module(&**module));
        for (index, module) in modules.iter().enumerate() {
            for other in &modules[index + 1..] {
                assert!(
                    !js_sys::Object::is(module, other),
                    "each compile makes a module of its own"
                );
            }
        }
    }
}
