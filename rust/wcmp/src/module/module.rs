//! The polyfill's handle for a compiled core module.

use std::sync::Arc;

use crate::engine::Engine;
use crate::error::{Error, InstantiationError, Result};
use crate::internal::{
    CoreExternInternal, CoreExternParts, CoreInstanceParts, ErrorInternal, ModuleInternal,
};
use crate::runtime_layer::{
    Imports as RuntimeImports, Module as RuntimeModule, Shared, instantiate, substrate_failure,
};
use crate::store::Store;
use crate::store::StoreInternalExt;

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
    /// The runtime-layer module.
    inner: Shared<RuntimeModule>,
    imports: Arc<[ModuleImport]>,
    /// Whether the module declares an import whose type the host's
    /// description cannot express, which [`Self::imports`] leaves out.
    undescribed_imports: bool,
    exports: Arc<[ModuleExport]>,
    /// Whether the module declares a `start` function.
    start: bool,
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
        Self::from_compiled(inner, bytes)
    }

    /// The imports the module declares, in declaration order.
    /// [`Self::instantiate`] takes one value per entry, in this order.
    ///
    /// An import whose type a [`CoreValueType`] cannot express, such as
    /// a reference to a garbage-collected type, is left out. The host
    /// cannot supply one, so it cannot instantiate such a module, but a
    /// component that holds the module still runs it.
    ///
    /// [`CoreValueType`]: crate::CoreValueType
    pub fn imports(&self) -> &[ModuleImport] {
        &self.imports
    }

    /// The exports the module declares, in declaration order. An export
    /// whose type a [`CoreValueType`] cannot express is left out, as an
    /// import is from [`Self::imports`].
    ///
    /// [`CoreValueType`]: crate::CoreValueType
    pub fn exports(&self) -> &[ModuleExport] {
        &self.exports
    }

    /// Instantiate the module into `store` with `imports`, one value
    /// per entry of [`Self::imports`] in the same order, and run its
    /// start function.
    ///
    /// The count of `imports` must match the declared count, and
    /// every value must live in `store`. A module with an import that
    /// [`Self::imports`] leaves out is refused as unsupported; an item of the wrong kind or
    /// type, and a start function that traps, surface as the runtime
    /// substrate's failure under [`Error::Instantiation`].
    ///
    /// The start function is guest code. One that traps poisons the
    /// store, and a store a trap poisoned refuses the instantiation
    /// with the cannot-enter cause, [`TaskCause::CannotEnter`], whether
    /// or not the module declares a start function.
    ///
    /// The future completes without suspending on both targets today;
    /// it is awaited so that a module whose instantiation must yield
    /// to the host can do so without a change of signature.
    ///
    /// [`Error::Instantiation`]: crate::Error::Instantiation
    /// [`TaskCause::CannotEnter`]: crate::TaskCause::CannotEnter
    pub async fn instantiate<T: 'static>(
        &self,
        store: &mut Store<T>,
        imports: &[CoreExtern],
    ) -> Result<CoreInstance> {
        if self.undescribed_imports {
            return Err(Error::unsupported(
                "host instantiation of a core module with an import of a type the host cannot describe",
            ));
        }
        if imports.len() != self.imports.len() {
            return Err(Error::from(InstantiationError::ImportCount {
                expected: self.imports.len(),
                found: imports.len(),
            }));
        }
        if imports
            .iter()
            .any(|import| import.store_id() != store.internal().id())
        {
            return Err(Error::from(InstantiationError::WrongStore));
        }
        store.internal().enter_guest()?;
        let mut runtime_imports = RuntimeImports::default();
        for (declared, supplied) in self.imports.iter().zip(imports) {
            runtime_imports.define(&declared.module, &declared.name, *supplied.inner());
        }
        let inner =
            match instantiate(store.internal().inner_mut(), &self.inner, &runtime_imports).await {
                Ok(inner) => inner,
                Err(error) => {
                    // The module's `start` function is guest code, and
                    // its failure is a trap, which poisons the store.
                    if self.start {
                        store.internal().poison();
                    }
                    return Err(substrate_failure(error));
                }
            };
        let store_id = store.internal().id();
        let exports = self
            .exports
            .iter()
            .map(|export| {
                let item = inner
                    .get_export(store.internal().inner_mut(), &export.name)
                    .map_err(substrate_failure)?
                    .ok_or_else(|| {
                        Error::internal("a core instance lacks an export its module declares")
                    })?;
                let parts = CoreExternParts {
                    inner: item,
                    store_id,
                    ty: export.ty.clone(),
                };
                Ok((export.name.clone(), CoreExtern::from(parts)))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(CoreInstanceParts { exports }.into())
    }
}

impl ModuleInternal for Module {
    fn from_compiled(inner: RuntimeModule, bytes: &[u8]) -> Result<Module> {
        let shape = read::read_shape(bytes)?;
        Ok(Self {
            inner: Shared::new(inner),
            imports: shape.imports.into(),
            undescribed_imports: shape.undescribed_imports,
            exports: shape.exports.into(),
            start: shape.start,
        })
    }

    fn inner(&self) -> &RuntimeModule {
        &self.inner
    }

    fn has_start(&self) -> bool {
        self.start
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
