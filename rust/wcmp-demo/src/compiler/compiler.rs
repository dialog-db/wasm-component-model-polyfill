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

use crate::language::{self, Completion, Diagnostic, Hover, Location};
use crate::platform;
use crate::wasi;

use super::{CompileError, CompileRequest, Compiled};

/// The interface of the compiler's one host import.
const HOST: &str = "wcmp:zena-compiler/host";

/// The interface of the language service the compiler exports.
const LANGUAGE: &str = "wcmp:zena-compiler/language";

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
    exports: Exports,
}

/// The compiler's exports: `compile`, and the language service's.
struct Exports {
    compile: Func,
    check: Func,
    hover: Func,
    complete: Func,
    definition: Func,
    format: Func,
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
        let (store, exports) = instantiate(engine, &linker, &component, &bundle).await?;
        Ok(Compiler {
            engine: engine.clone(),
            component,
            linker,
            bundle,
            store,
            exports,
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
        let results = self.call(|exports| &exports.compile, &arguments).await;
        let millis = platform::now_millis() - started;
        self.store.data_mut().extra.clear();
        match results?.first() {
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

impl Compiler {
    /// Check `request` with the language service: the diagnostics of the
    /// program `compile` would compile, in every file that is not the
    /// standard library's. The queries below answer about the program
    /// of the last check.
    ///
    /// # Errors
    ///
    /// The polyfill's error when the compiler could not run.
    #[tracing::instrument(level = "debug", name = "Zena check", skip_all, fields(entry = request.entry_path))]
    pub async fn check(&mut self, request: &CompileRequest<'_>) -> Result<Vec<Diagnostic>, Error> {
        let files = request
            .files
            .iter()
            .map(|(path, text)| {
                Val::Tuple(Box::new([
                    Val::String(path.clone()),
                    Val::String(text.clone()),
                ]))
            })
            .collect();
        let arguments = [
            Val::String(request.source.to_string()),
            Val::String(request.entry_path.to_string()),
            Val::List(files),
            Val::String(request.wit.to_string()),
            Val::String(request.world.to_string()),
        ];
        let results = self.call(|exports| &exports.check, &arguments).await?;
        language::diagnostics_of(first(&results)?)
    }

    /// What is at byte `offset` of the file `path`, in the last check.
    ///
    /// # Errors
    ///
    /// The polyfill's error when the compiler could not run.
    pub async fn hover(&mut self, path: &str, offset: u32) -> Result<Option<Hover>, Error> {
        let arguments = [Val::String(path.to_string()), Val::U32(offset)];
        let results = self.call(|exports| &exports.hover, &arguments).await?;
        language::hover_of(first(&results)?)
    }

    /// The completions at byte `offset` of the file `path`, in the last
    /// check.
    ///
    /// # Errors
    ///
    /// The polyfill's error when the compiler could not run.
    pub async fn complete(&mut self, path: &str, offset: u32) -> Result<Vec<Completion>, Error> {
        let arguments = [Val::String(path.to_string()), Val::U32(offset)];
        let results = self.call(|exports| &exports.complete, &arguments).await?;
        language::completions_of(first(&results)?)
    }

    /// Where what is at byte `offset` of the file `path` is declared, in
    /// the last check.
    ///
    /// # Errors
    ///
    /// The polyfill's error when the compiler could not run.
    pub async fn definition(&mut self, path: &str, offset: u32) -> Result<Option<Location>, Error> {
        let arguments = [Val::String(path.to_string()), Val::U32(offset)];
        let results = self.call(|exports| &exports.definition, &arguments).await?;
        language::location_of(first(&results)?)
    }

    /// `source` as Zena's formatter prints it, or the text of the error
    /// that stopped it.
    ///
    /// # Errors
    ///
    /// The polyfill's error when the compiler could not run.
    pub async fn format(&mut self, source: &str) -> Result<Result<String, String>, Error> {
        let arguments = [Val::String(source.to_string())];
        let results = self.call(|exports| &exports.format, &arguments).await?;
        language::formatted_of(first(&results)?)
    }

    /// Call the export `which` picks with `arguments`. A trap poisons the
    /// store, so the next call gets a new instance; if that fails too,
    /// the next call reports the poisoned store.
    async fn call(
        &mut self,
        which: impl Fn(&Exports) -> &Func,
        arguments: &[Val],
    ) -> Result<Box<[Val]>, Error> {
        let results = which(&self.exports).call(&mut self.store, arguments).await;
        if results.is_err()
            && let Ok((store, exports)) =
                instantiate(&self.engine, &self.linker, &self.component, &self.bundle).await
        {
            self.store = store;
            self.exports = exports;
        }
        results
    }
}

/// The first result of a call.
fn first(results: &[Val]) -> Result<&Val, Error> {
    results.first().ok_or_else(|| Error::Internal {
        message: "the compiler answered no result".to_string(),
    })
}

/// Instantiate the compiler `component` through `linker`, in a new store
/// that reads sources from `bundle`, and answer its exports.
async fn instantiate(
    engine: &Engine,
    linker: &Linker<Sources>,
    component: &Component,
    bundle: &Arc<SourceBundle>,
) -> Result<(Store<Sources>, Exports), Error> {
    let mut store = Store::new(
        engine,
        Sources {
            bundle: bundle.clone(),
            extra: HashMap::new(),
        },
    )?;
    let instance = linker.instantiate(&mut store, component).await?;
    let missing = |name: &str| Error::Unsupported {
        feature: format!("a compiler component with no `{name}` export"),
    };
    let compile = instance
        .get_func("compile")
        .ok_or_else(|| missing("compile"))?;
    let language = |name: &str| {
        instance
            .exports()
            .instance(LANGUAGE)
            .and_then(|language| language.func(name))
            .ok_or_else(|| missing(&format!("{LANGUAGE}#{name}")))
    };
    let exports = Exports {
        compile,
        check: language("check")?,
        hover: language("hover")?,
        complete: language("complete")?,
        definition: language("definition")?,
        format: language("format")?,
    };
    Ok((store, exports))
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
