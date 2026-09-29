//! A module as the Wasmtime backend compiled it.

use core::any::Any;

use wcmp_wasm_core::backend::BackendModule;
use wcmp_wasm_core::{ExportType, ImportType};

use crate::convert;
use crate::type_registry::TypeRegistry;

/// A module that Wasmtime compiled, and the description of its boundary.
///
/// The backend describes the boundary once, when it compiles the module.
/// The description covers the imports and the exports, and nothing inside
/// the module.
pub struct WasmtimeModule {
    module: wasmtime::Module,
    imports: Vec<ImportType>,
    exports: Vec<ExportType>,
}

impl WasmtimeModule {
    /// The module `module`, whose concrete types `types` numbers.
    pub fn new(module: wasmtime::Module, types: &TypeRegistry) -> Self {
        let imports = module
            .imports()
            .map(|import| {
                ImportType::new(
                    import.module(),
                    import.name(),
                    convert::extern_type(types, &import.ty()),
                )
            })
            .collect();
        let exports = module
            .exports()
            .map(|export| ExportType::new(export.name(), convert::extern_type(types, &export.ty())))
            .collect();
        Self {
            module,
            imports,
            exports,
        }
    }

    /// The module as Wasmtime compiled it.
    pub fn module(&self) -> &wasmtime::Module {
        &self.module
    }
}

impl BackendModule for WasmtimeModule {
    fn imports(&self) -> &[ImportType] {
        &self.imports
    }

    fn exports(&self) -> &[ExportType] {
        &self.exports
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
