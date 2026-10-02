// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Zena's compiler, instantiated on the polyfill.

use std::collections::HashMap;
use std::sync::Arc;

use wcmp::{Component, Engine, Error, Func, Linker, Store, Val};
use wcmp_scenario::SourceBundle;

use crate::platform;
use crate::wasi;

use super::{CompileError, CompileRequest, Compiled};

/// The interface of the compiler's one host import.
const HOST: &str = "wcmp:zena-compiler/host";

/// What the compiler's store holds: where `read-source` reads from.
struct Sources {
    bundle: Arc<SourceBundle>,
    /// The files of the compile in progress, by path.
    extra: HashMap<String, String>,
}

/// Zena's compiler, instantiated once on the polyfill, and again after
/// a compile that traps it.
pub struct Compiler {
    engine: Engine,
    component: Component,
    linker: Linker<Sources>,
    bundle: Arc<SourceBundle>,
    store: Store<Sources>,
    compile: Func,
}

impl Compiler {
    /// Parse the compiler component `bytes` and instantiate it on
    /// `engine`, reading sources from `bundle`.
    ///
    /// # Errors
    ///
    /// The polyfill's error when the component does not parse, link,
    /// or instantiate, or exports no `compile`.
    pub async fn new(
        engine: &Engine,
        bytes: &[u8],
        bundle: Arc<SourceBundle>,
    ) -> Result<Self, Error> {
        let component = Component::new(engine, bytes).await?;
        let mut linker = Linker::new(engine);
        wasi::define(&mut linker, "zena compiler")?;
        linker
            .instance(
                &HOST
                    .parse()
                    .expect("the host interface's identifier parses"),
            )
            .func_wrap("read-source", |call, (path,): (String,)| {
                let sources: &Sources = call.data();
                Ok(sources
                    .extra
                    .get(&path)
                    .cloned()
                    .or_else(|| sources.bundle.read(&path).map(str::to_string)))
            })?;
        let (store, compile) = instantiate(engine, &linker, &component, &bundle).await?;
        Ok(Compiler {
            engine: engine.clone(),
            component,
            linker,
            bundle,
            store,
            compile,
        })
    }

    /// Compile `request`.
    ///
    /// # Errors
    ///
    /// [`CompileError::Diagnostics`] when the compiler refused the
    /// program, and [`CompileError::Failed`] when it could not run.
    #[tracing::instrument(level = "debug", name = "Zena compile", skip_all, fields(entry = request.entry_path, world = request.world))]
    pub async fn compile(
        &mut self,
        request: &CompileRequest<'_>,
    ) -> Result<Compiled, CompileError> {
        self.store.data_mut().extra = request.files.iter().cloned().collect();
        let arguments = [
            Val::String(request.source.to_string()),
            Val::String(request.entry_path.to_string()),
            Val::String(request.wit.to_string()),
            Val::String(request.world.to_string()),
        ];
        let started = platform::now_millis();
        let results = self.compile.call(&mut self.store, &arguments).await;
        let millis = platform::now_millis() - started;
        self.store.data_mut().extra.clear();
        let results = match results {
            Ok(results) => results,
            Err(error) => {
                // A trap poisons the store, so the next compile gets a
                // new instance. If that fails too, the next compile
                // reports the poisoned store.
                if let Ok((store, compile)) =
                    instantiate(&self.engine, &self.linker, &self.component, &self.bundle).await
                {
                    self.store = store;
                    self.compile = compile;
                }
                return Err(error.into());
            }
        };
        match results.first() {
            Some(Val::Result(Ok(Some(bytes)))) => Ok(Compiled {
                bytes: bytes_of(bytes)?,
                millis,
            }),
            Some(Val::Result(Err(Some(text)))) => match text.as_ref() {
                Val::String(text) => Err(CompileError::Diagnostics {
                    text: text.clone(),
                    millis,
                }),
                other => Err(unexpected(other).into()),
            },
            other => Err(Error::Internal {
                message: format!("`compile` returned {other:?}"),
            }
            .into()),
        }
    }
}

/// Instantiate the compiler `component` through `linker`, in a new store
/// that reads sources from `bundle`, and answer its `compile`.
async fn instantiate(
    engine: &Engine,
    linker: &Linker<Sources>,
    component: &Component,
    bundle: &Arc<SourceBundle>,
) -> Result<(Store<Sources>, Func), Error> {
    let mut store = Store::new(
        engine,
        Sources {
            bundle: bundle.clone(),
            extra: HashMap::new(),
        },
    )?;
    let instance = linker.instantiate(&mut store, component).await?;
    let compile = instance
        .get_func("compile")
        .ok_or_else(|| Error::Unsupported {
            feature: "a compiler component with no `compile` export".to_string(),
        })?;
    Ok((store, compile))
}

/// The bytes of a `list<u8>`.
fn bytes_of(list: &Val) -> Result<Vec<u8>, Error> {
    let Val::List(items) = list else {
        return Err(unexpected(list));
    };
    items
        .iter()
        .map(|item| match item {
            Val::U8(byte) => Ok(*byte),
            other => Err(unexpected(other)),
        })
        .collect()
}

/// The error for a value of the wrong shape in `compile`'s result.
fn unexpected(val: &Val) -> Error {
    Error::Internal {
        message: format!("`compile` returned an unexpected value {val:?}"),
    }
}
