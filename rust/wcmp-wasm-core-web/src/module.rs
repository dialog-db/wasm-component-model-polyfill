// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

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

    /// Whether a memory of the module, imported or its own, is shared.
    pub fn shared_memory(&self) -> bool {
        self.boundary.shared_memory
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
