//! The polyfill's handle for a compiled core module.

use std::sync::Arc;

use wasm_runtime_layer::{
    Imports as RuntimeImports, Instance as RuntimeInstance, Module as RuntimeModule,
};

use crate::engine::Engine;
use crate::error::{Error, InstantiationError, Result};
use crate::store::Store;

use super::core_extern::CoreExtern;
use super::core_instance::CoreInstance;
use super::module_export::ModuleExport;
use super::module_import::ModuleImport;
use super::read;

/// A compiled core WebAssembly module.
///
/// A `Module` is one of two things: a core module a component
/// exports, reached through [`Instance::get_module`] or the export
/// navigator, or a core module the host loaded from bytes with
/// [`Module::new`]. Either way the handle describes the module's
/// imports and exports and instantiates it into a [`CoreInstance`]
/// with imports the host supplies. It is the polyfill's analogue to
/// `wasmtime::Module`, and it hides the runtime layer's module type.
///
/// Cloning a `Module` is cheap: clones share the compiled module.
///
/// [`Instance::get_module`]: crate::Instance::get_module
#[derive(Clone)]
pub struct Module {
    /// The runtime-layer module. Workspace-internal; never
    /// re-exported through `lib.rs`.
    pub inner: RuntimeModule,
    imports: Arc<[ModuleImport]>,
    exports: Arc<[ModuleExport]>,
}

impl Module {
    /// Compile a core module from its binary against an [`Engine`].
    ///
    /// The future suspends only while the browser compiles the module
    /// through its asynchronous API, which is the only way a large
    /// module loads on the main thread. On native it completes
    /// without suspending. Bytes that are not a valid core module,
    /// including a component binary, are refused with
    /// [`Error::Instantiation`].
    ///
    /// [`Error::Instantiation`]: crate::Error::Instantiation
    pub async fn new(engine: &Engine, bytes: &[u8]) -> Result<Self> {
        let inner = crate::executor::compile_module(engine, bytes).await?;
        let shape = read::read_shape(bytes)?;
        Ok(Self {
            inner,
            imports: shape.imports.into(),
            exports: shape.exports.into(),
        })
    }

    /// The imports the module declares, in declaration order.
    /// [`Self::instantiate`] takes one value per entry, in this order.
    pub fn imports(&self) -> &[ModuleImport] {
        &self.imports
    }

    /// The exports the module declares, in declaration order.
    pub fn exports(&self) -> &[ModuleExport] {
        &self.exports
    }

    /// Instantiate the module into `store` with `imports`, one value
    /// per entry of [`Self::imports`] in the same order, and run its
    /// start function.
    ///
    /// The count of `imports` must match the declared count, and
    /// every value must live in `store`; an item of the wrong kind or
    /// type, and a start function that traps, surface as the runtime
    /// substrate's failure under [`Error::Instantiation`].
    ///
    /// The future completes without suspending on both targets today;
    /// it is awaited so that a module whose instantiation must yield
    /// to the host can do so without a change of signature.
    ///
    /// [`Error::Instantiation`]: crate::Error::Instantiation
    pub async fn instantiate<T: 'static>(
        &self,
        store: &mut Store<T>,
        imports: &[CoreExtern],
    ) -> Result<CoreInstance> {
        if imports.len() != self.imports.len() {
            return Err(Error::from(InstantiationError::ImportCount {
                expected: self.imports.len(),
                found: imports.len(),
            }));
        }
        if imports.iter().any(|import| import.store_id != store.id) {
            return Err(Error::from(InstantiationError::WrongStore));
        }
        let mut runtime_imports = RuntimeImports::default();
        for (declared, supplied) in self.imports.iter().zip(imports) {
            runtime_imports.define(&declared.module, &declared.name, supplied.inner.clone());
        }
        let inner = RuntimeInstance::new(store.inner_mut(), &self.inner, &runtime_imports)
            .map_err(InstantiationError::SubstrateFailure)?;
        Ok(CoreInstance {
            inner,
            store_id: store.id,
        })
    }
}

impl core::fmt::Debug for Module {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Module")
            .field("imports", &self.imports)
            .field("exports", &self.exports)
            .finish_non_exhaustive()
    }
}
