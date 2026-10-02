// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A compiled module.

use core::fmt;
use std::sync::Arc;

use crate::contract::BackendModule;
use crate::engine::Engine;
use crate::error::Result;
use crate::internal::{EngineInternal, ModuleInternal};
use crate::module::{ExportType, ImportType};

/// A compiled module, which any store of its engine can instantiate.
///
/// The module describes its boundary, its imports and its exports, and
/// nothing else. Nothing inside a module is a reason to refuse it: the
/// engine decides whether it compiles. The module is cheap to clone.
#[derive(Clone)]
pub struct Module {
    engine: Engine,
    inner: Arc<dyn BackendModule>,
}

impl Module {
    /// Compiles `bytes` on `engine`, asynchronously.
    ///
    /// Every backend compiles this way. The browser backend compiles with
    /// `WebAssembly.compile`, so a module above the browser's limit for a
    /// synchronous compile loads too. Each compile makes a module of its
    /// own: the engine keeps no cache of modules by their bytes.
    pub async fn compile(engine: &Engine, bytes: &[u8]) -> Result<Self> {
        let inner = engine.backend().compile(bytes).await?;
        Ok(Self {
            engine: engine.clone(),
            inner: Arc::from(inner),
        })
    }

    /// Compiles `bytes` on `engine`, synchronously, as Wasmtime's
    /// `Module::new` does.
    ///
    /// This compile is for small modules that a backend or a host
    /// generates. In the browser, a module above the browser's limit for a
    /// synchronous compile fails with a structured error.
    pub fn new(engine: &Engine, bytes: &[u8]) -> Result<Self> {
        let inner = engine.backend().compile_sync(bytes)?;
        Ok(Self {
            engine: engine.clone(),
            inner: Arc::from(inner),
        })
    }

    /// The engine that compiled the module.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// The imports of the module, in the order an instantiation takes them.
    pub fn imports(&self) -> impl ExactSizeIterator<Item = &ImportType> {
        self.inner.imports().iter()
    }

    /// The exports of the module, in the order the module declares them.
    pub fn exports(&self) -> impl ExactSizeIterator<Item = &ExportType> {
        self.inner.exports().iter()
    }
}

impl ModuleInternal for Module {
    fn backend_module(&self) -> &dyn BackendModule {
        &*self.inner
    }
}

impl fmt::Debug for Module {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Module")
            .field("imports", &self.inner.imports())
            .field("exports", &self.inner.exports())
            .finish_non_exhaustive()
    }
}
