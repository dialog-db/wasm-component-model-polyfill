// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One context's engine and compiler.

use std::rc::Rc;
use std::sync::Arc;

use futures::lock::Mutex;
use wasm_bindgen::JsValue;
use wcmp::{Component, Engine, Linker, Store};
use wcmp_scenario::SourceBundle;

use super::{Instantiated, fetch_bytes, text};
use crate::compiler::{CompileError, CompileRequest, Compiled, Compiler};
use crate::platform;

/// The compiler component, beside the page.
const COMPILER: &str = "./zena-compiler.wasm";

/// The source bundle, beside the page.
const BUNDLE: &str = "./zena-sources.bundle";

/// One context's engine and compiler.
pub struct Context {
    engine: Engine,
    compiler: Rc<Mutex<Compiler>>,
}

impl Context {
    /// Start a context: fetch the compiler component and the source
    /// bundle, and instantiate the compiler on the browser backend.
    ///
    /// # Errors
    ///
    /// The exception of a failed fetch, or the polyfill's error as text
    /// when the compiler does not instantiate.
    #[tracing::instrument(level = "debug", name = "context start", skip_all)]
    pub async fn start() -> Result<Rc<Self>, JsValue> {
        let engine = Engine::with_backend(wcmp_wasm_core_web::Web::new()).map_err(text)?;
        let bundle = SourceBundle::parse(&fetch_bytes(BUNDLE).await?).map_err(text)?;
        let compiler_bytes = fetch_bytes(COMPILER).await?;
        let compiler = Compiler::new(&engine, &compiler_bytes, Arc::new(bundle))
            .await
            .map_err(text)?;
        Ok(Rc::new(Context {
            engine,
            compiler: Rc::new(Mutex::new(compiler)),
        }))
    }

    /// The engine every component of this context runs on.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// The context's one compiler, to share with a router.
    pub fn compiler(&self) -> Rc<Mutex<Compiler>> {
        self.compiler.clone()
    }

    /// Compile `request`. Compiles in one context run one at a time.
    ///
    /// # Errors
    ///
    /// The compiler's diagnostics, or the reason it could not run.
    pub async fn compile(&self, request: &CompileRequest<'_>) -> Result<Compiled, CompileError> {
        self.compiler.lock().await.compile(request).await
    }

    /// Compile `bytes` as a component on this context's engine, and
    /// answer it with the time the compile took, in milliseconds: the
    /// polyfill's translation and the compiles of its core modules.
    ///
    /// # Errors
    ///
    /// The polyfill's error when the component does not parse.
    #[tracing::instrument(level = "debug", name = "demo parse", skip_all, fields(bytes = bytes.len()))]
    pub async fn parse(&self, bytes: &[u8]) -> Result<(Component, f64), wcmp::Error> {
        let started = platform::now_millis();
        let component = Component::new(&self.engine, bytes).await?;
        Ok((component, platform::now_millis() - started))
    }

    /// Instantiate `component` with `linker` in a new store that holds
    /// `data`, and answer the time the link and the instantiation took.
    ///
    /// # Errors
    ///
    /// The polyfill's error when the component does not link or
    /// instantiate.
    #[tracing::instrument(level = "debug", name = "demo instantiate", skip_all)]
    pub async fn instantiate<T: 'static>(
        &self,
        component: &Component,
        linker: &Linker<T>,
        data: T,
    ) -> Result<Instantiated<T>, wcmp::Error> {
        let started = platform::now_millis();
        let mut store = Store::new(&self.engine, data)?;
        let instance = linker.instantiate(&mut store, component).await?;
        Ok(Instantiated {
            store,
            instance,
            millis: platform::now_millis() - started,
        })
    }
}
