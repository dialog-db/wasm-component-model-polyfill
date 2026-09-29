//! A module as the browser compiled it.

use core::any::Any;

use js_sys::WebAssembly;
use wcmp_wasm_core::backend::BackendModule;
use wcmp_wasm_core::{ExportType, ImportType};

use crate::boundary::Boundary;

/// A `WebAssembly.Module`, and the description of its boundary.
///
/// The backend describes the boundary once, when it compiles the module.
/// The description covers the imports and the exports, and nothing inside
/// the module.
pub struct WebModule {
    module: WebAssembly::Module,
    boundary: Boundary,
}

impl WebModule {
    /// The module `module`, whose boundary is `boundary`.
    pub fn new(module: WebAssembly::Module, boundary: Boundary) -> Self {
        Self { module, boundary }
    }

    /// The module as the browser compiled it.
    pub fn module(&self) -> &WebAssembly::Module {
        &self.module
    }
}

impl BackendModule for WebModule {
    fn imports(&self) -> &[ImportType] {
        &self.boundary.imports
    }

    fn exports(&self) -> &[ExportType] {
        &self.boundary.exports
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
