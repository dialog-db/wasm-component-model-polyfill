//! The conformance harness: runs the vendored `.wast` corpora under
//! `tests/corpus/` against the polyfill's public API.
//!
//! Every `.wast` file becomes one test (see `conformance/manifest.rs`).
//! A test walks the file's directives, maps each onto the polyfill
//! (`Component::new`, `Linker::instantiate`, `Func::call`), and
//! records a failure per directive that does not behave as the
//! directive asserts. The test then compares its failures with
//! `tests/corpus/expected-failures.txt`: a listed directive that now
//! passes and an unlisted directive that fails both fail the test,
//! so the list stays current.
//!
//! Every expectation carries a category, and one more test,
//! `it_reports_conformance_progress`, runs every file in one process
//! and prints a summary per corpus directory: directives, passes, and
//! expected failures per category. When `WCMP_CONFORMANCE_SUMMARY`
//! names a file, the test writes the same summary there as JSON.
//!
//! The host environment is the one Wasmtime's wast runner provides:
//! the fixed set of `host` items its component spectest registers,
//! and the module exports of every named component a file
//! instantiates, reflected into the linker under the component's
//! name.

#![cfg(test)]

#[path = "conformance/report.rs"]
mod report;

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use wasm_component_model_polyfill::{
    Component, Engine, Error, ExternType, ExternalName, FunctionParameter, FunctionType,
    HostResource, Instance, Linker, Module, PrimitiveType, ResourceType, Store, Val, ValField,
    ValueType,
};
use wast::component::WastVal;
use wast::parser::{self, ParseBuffer};
use wast::token::Span;
use wast::{Wast, WastArg, WastDirective, WastExecute, WastRet};

use report::{Expectation, Failure, FileReport, Summary, parse_expectations};

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

const EXPECTED_FAILURES: &str = include_str!("corpus/expected-failures.txt");

/// The browser-only delta, applied on top of the shared list on
/// `wasm32-unknown-unknown`: differences of the substrate, not of the
/// polyfill. The native progress run reads it too, to project the
/// browser's summary.
const EXPECTED_FAILURES_WEB: &str = include_str!("corpus/expected-failures.web.txt");

/// Parse the expectations that apply on this target.
fn expectations() -> Vec<Expectation> {
    let shared = parse_expectations(EXPECTED_FAILURES).unwrap_or_else(|err| panic!("{err}"));
    #[cfg(not(target_arch = "wasm32"))]
    {
        shared
    }
    #[cfg(target_arch = "wasm32")]
    {
        let mut all = shared;
        all.extend(
            parse_expectations(EXPECTED_FAILURES_WEB)
                .unwrap_or_else(|err| panic!("expected-failures.web.txt: {err}")),
        );
        all
    }
}

/// The 4-byte version word after `\0asm` in a core module.
const CORE_MODULE_VERSION: [u8; 4] = [0x01, 0x00, 0x00, 0x00];

/// The host state a `.wast` file runs against: one engine, one
/// store, the components defined so far, and the instances created
/// so far.
struct Runner {
    engine: Engine,
    store: Store<()>,
    linker: Linker<()>,
    definitions: HashMap<String, Component>,
    last_definition: Option<Component>,
    /// Every instance created so far, in creation order.
    instances: Vec<Instance>,
    /// Named instances, as indices into `instances`.
    named: HashMap<String, usize>,
    /// The instance an unqualified `invoke` targets.
    current: Option<usize>,
}

impl Runner {
    async fn new() -> Self {
        let engine = Engine::new().expect("engine");
        let store = Store::new(&engine, ()).expect("store");
        let mut linker = Linker::new(&engine);
        link_spectest(&engine, &mut linker).await;
        Self {
            engine,
            store,
            linker,
            definitions: HashMap::new(),
            last_definition: None,
            instances: Vec::new(),
            named: HashMap::new(),
            current: None,
        }
    }

    /// Run one file. Returns the number of directives the file holds
    /// and the failures. A file that does not lex or parse counts as
    /// one directive that fails.
    async fn run(&mut self, text: &str) -> (usize, Vec<Failure>) {
        let mut failures = Vec::new();
        let buffer = match ParseBuffer::new(text) {
            Ok(buffer) => buffer,
            Err(err) => {
                failures.push(Failure {
                    line: 1,
                    reason: format!("the file does not lex: {err}"),
                });
                return (1, failures);
            }
        };
        let wast: Wast<'_> = match parser::parse(&buffer) {
            Ok(wast) => wast,
            Err(err) => {
                let (line, _) = err.span().linecol_in(text);
                failures.push(Failure {
                    line: line + 1,
                    reason: format!("the file does not parse: {}", err.message()),
                });
                return (1, failures);
            }
        };
        let mut directives = 0;
        for directive in wast.directives {
            directives += 1;
            let span = directive_span(&directive);
            let (line, _) = span.linecol_in(text);
            if let Err(reason) = self.directive(directive).await {
                failures.push(Failure {
                    line: line + 1,
                    reason,
                });
            }
        }
        (directives, failures)
    }

    async fn directive(&mut self, directive: WastDirective<'_>) -> Result<(), String> {
        match directive {
            WastDirective::Module(mut quote) => {
                // A directive that fails leaves no current instance, so
                // a later `invoke` reports the cascade rather than
                // running against whatever was current before.
                self.current = None;
                let name = quote.name().map(|id| id.name().to_owned());
                let bytes = quote.encode().map_err(|err| format!("encode: {err}"))?;
                let component = self.component(&bytes).await?;
                let index = self.instantiate(&component).await?;
                if let Some(name) = name {
                    self.register_named(&name, &component, index);
                    self.named.insert(name, index);
                }
                self.current = Some(index);
                Ok(())
            }
            WastDirective::ModuleDefinition(mut quote) => {
                let name = quote.name().map(|id| id.name().to_owned());
                let bytes = quote.encode().map_err(|err| format!("encode: {err}"))?;
                let component = self.component(&bytes).await?;
                if let Some(name) = name {
                    self.definitions.insert(name, component.clone());
                }
                self.last_definition = Some(component);
                Ok(())
            }
            WastDirective::ModuleInstance {
                instance, module, ..
            } => {
                self.current = None;
                let component = match module {
                    Some(id) => self
                        .definitions
                        .get(id.name())
                        .cloned()
                        .ok_or_else(|| format!("no definition named `{}`", id.name()))?,
                    None => self
                        .last_definition
                        .clone()
                        .ok_or_else(|| "no definition to instantiate".to_owned())?,
                };
                let index = self.instantiate(&component).await?;
                if let Some(id) = instance {
                    self.named.insert(id.name().to_owned(), index);
                }
                self.current = Some(index);
                Ok(())
            }
            WastDirective::Register { .. } => {
                Err("the `register` directive is not supported".into())
            }
            WastDirective::Invoke(invoke) => self
                .invoke(invoke.module, invoke.name, invoke.args)
                .await
                .map(|_| ()),
            WastDirective::AssertReturn { exec, results, .. } => {
                let (actual, result_type) = self.execute(exec).await?;
                let mut expected = Vec::with_capacity(results.len());
                for ret in results {
                    match ret {
                        WastRet::Component(val) => {
                            let value = convert(&val)?;
                            expected.push(match &result_type {
                                Some(ty) => coerce(value, ty),
                                None => value,
                            });
                        }
                        _ => return Err("core-Wasm result in a component directive".into()),
                    }
                }
                if actual.len() != expected.len()
                    || !actual
                        .iter()
                        .zip(expected.iter())
                        .all(|(a, e)| vals_equal(a, e))
                {
                    return Err(format!("expected {expected:?}, got {actual:?}"));
                }
                Ok(())
            }
            WastDirective::AssertTrap { exec, message, .. } => match self.execute(exec).await {
                Ok((values, _)) => Err(format!("expected a trap `{message}`, got {values:?}")),
                Err(err) => {
                    if err.contains(message) {
                        Ok(())
                    } else {
                        Err(format!("expected a trap `{message}`, got `{err}`"))
                    }
                }
            },
            WastDirective::AssertInvalid {
                mut module,
                message,
                ..
            }
            | WastDirective::AssertInvalidCustom {
                mut module,
                message,
                ..
            }
            | WastDirective::AssertMalformed {
                mut module,
                message,
                ..
            }
            | WastDirective::AssertMalformedCustom {
                mut module,
                message,
                ..
            } => {
                let bytes = match module.encode() {
                    Ok(bytes) => bytes,
                    // Text that does not encode is rejected before the
                    // polyfill sees it, which satisfies the assertion.
                    Err(_) => return Ok(()),
                };
                match self.component(&bytes).await {
                    Ok(_) => Err(format!(
                        "expected rejection `{message}`, but the component parsed"
                    )),
                    Err(_) => Ok(()),
                }
            }
            WastDirective::AssertUnlinkable {
                mut module,
                message,
                ..
            } => {
                let bytes = module.encode().map_err(|err| format!("encode: {err}"))?;
                let component = match self.component(&bytes).await {
                    Ok(component) => component,
                    Err(_) => return Ok(()),
                };
                match self.instantiate(&component).await {
                    Ok(_) => Err(format!("expected link failure `{message}`, but it linked")),
                    Err(_) => Ok(()),
                }
            }
            WastDirective::AssertExhaustion { .. } => {
                Err("the `assert_exhaustion` directive is not supported".into())
            }
            WastDirective::AssertException { .. } => {
                Err("the `assert_exception` directive is not supported".into())
            }
            WastDirective::AssertSuspension { .. } => {
                Err("the `assert_suspension` directive is not supported".into())
            }
            WastDirective::Thread(_) | WastDirective::Wait { .. } => {
                Err("thread directives are not supported".into())
            }
        }
    }

    async fn component(&self, bytes: &[u8]) -> Result<Component, String> {
        if bytes.len() >= 8 && bytes[4..8] == CORE_MODULE_VERSION {
            return Err("core module directives are not supported".into());
        }
        Component::new(&self.engine, bytes)
            .await
            .map_err(|err| format!("component rejected: {}", chain(&err)))
    }

    /// Instantiate `component` and return its index in `instances`.
    async fn instantiate(&mut self, component: &Component) -> Result<usize, String> {
        let instance = self
            .linker
            .instantiate(&mut self.store, component)
            .await
            .map_err(|err| format!("instantiation failed: {}", chain(&err)))?;
        self.instances.push(instance);
        Ok(self.instances.len() - 1)
    }

    /// Reflect a named component's module exports into the linker
    /// under the component's name, as Wasmtime's runner does, so a
    /// later directive can import them. Functions are not reflected
    /// there either.
    fn register_named(&mut self, name: &str, component: &Component, index: usize) {
        let instance = &self.instances[index];
        let mut root = self.linker.root();
        let mut registration = root.instance(name);
        for export in component.exports.iter() {
            let (ExternType::Module(_), ExternalName::Plain(export_name)) =
                (&export.ty, &export.name)
            else {
                continue;
            };
            if let Some(module) = instance.get_module(export_name) {
                registration.module(export_name, &module);
            }
        }
    }

    /// Run an action: the values it produced and, for an invocation,
    /// the declared result type of the function.
    async fn execute(
        &mut self,
        exec: WastExecute<'_>,
    ) -> Result<(Box<[Val]>, Option<ValueType>), String> {
        match exec {
            WastExecute::Invoke(invoke) => {
                self.invoke(invoke.module, invoke.name, invoke.args).await
            }
            WastExecute::Wat(mut wat) => {
                let bytes = wat.encode().map_err(|err| format!("encode: {err}"))?;
                let component = self.component(&bytes).await?;
                self.instantiate(&component).await?;
                Ok((Box::new([]), None))
            }
            WastExecute::Get { .. } => Err("the `get` directive is not supported".into()),
        }
    }

    async fn invoke(
        &mut self,
        module: Option<wast::token::Id<'_>>,
        name: &str,
        args: Vec<WastArg<'_>>,
    ) -> Result<(Box<[Val]>, Option<ValueType>), String> {
        let index = match module {
            Some(id) => *self
                .named
                .get(id.name())
                .ok_or_else(|| format!("no instance named `{}`", id.name()))?,
            None => self
                .current
                .ok_or_else(|| "no instance to invoke".to_owned())?,
        };
        let instance = &self.instances[index];
        let func = instance
            .get_func(name)
            .ok_or_else(|| format!("no function export named `{name}`"))?;
        let signature = func.ty().clone();
        let mut values = Vec::with_capacity(args.len());
        for (index, arg) in args.into_iter().enumerate() {
            match arg {
                WastArg::Component(val) => {
                    let value = convert(&val)?;
                    values.push(match signature.parameters.get(index) {
                        Some(parameter) => coerce(value, &parameter.ty),
                        None => value,
                    });
                }
                _ => return Err("core-Wasm argument in a component directive".into()),
            }
        }
        let results = func
            .call(&mut self.store, &values)
            .await
            .map_err(|err| chain(&err))?;
        Ok((results, signature.result))
    }
}

/// The core module Wasmtime's wast runner registers as
/// `host.simple-module`.
const SIMPLE_MODULE: &[u8] = wcmp_macros::wasm!(
    r#"
    (module
      (global (export "g") i32 i32.const 100)
      (func (export "f") (result i32) i32.const 101))
    "#
);

/// The drop bookkeeping behind `host.resource1`, read back through
/// `[static]resource1.drops` and `[static]resource1.last-drop`.
#[derive(Default)]
struct ResourceState {
    drops: AtomicU32,
    last_drop: AtomicU32,
}

/// Register the host items Wasmtime's wast runner provides for its
/// component tests (`crates/wast/src/spectest.rs`,
/// `link_component_spectest`), so the directives that import them
/// run as they do there. The asynchronous items (`host-echo-u32`,
/// `never-return`, `return-two-slowly`, `echo-slowly`, and
/// `[method]resource1.never-return`) are left out: the corpus files
/// that use them are not vendored.
async fn link_spectest(engine: &Engine, linker: &mut Linker<()>) {
    linker
        .root()
        .func_wrap("host-return-two", |_, (): ()| Ok(2u32));

    let simple_module = Module::new(engine, SIMPLE_MODULE)
        .await
        .expect("the spectest module compiles");
    let state = Arc::new(ResourceState::default());
    let resource1 = HostResource::new({
        let state = state.clone();
        move |_, rep| {
            state.drops.fetch_add(1, Ordering::SeqCst);
            state.last_drop.store(rep, Ordering::SeqCst);
            Ok(())
        }
    });

    let mut root = linker.root();
    let mut host = root.instance("host");
    host.func_wrap("return-three", |_, (): ()| Ok(3u32));
    host.instance("nested")
        .func_wrap("return-four", |_, (): ()| Ok(4u32));
    host.module("simple-module", &simple_module);

    let resource1_id = host.resource_with("resource1", resource1.clone());
    host.resource("resource2", |_, _| Ok(()));
    // The same resource type under a second name, as the runner
    // registers it.
    host.resource_with("resource1-again", resource1);

    let own = || ValueType::Own(ResourceType::new("resource1"));
    let borrow = || ValueType::Borrow(ResourceType::new("resource1"));
    let u32_ty = || ValueType::Primitive(PrimitiveType::U32);
    let signature = |params: &[(&str, ValueType)], result: Option<ValueType>| FunctionType {
        parameters: params
            .iter()
            .map(|(name, ty)| FunctionParameter {
                name: (*name).to_owned(),
                ty: ty.clone(),
            })
            .collect(),
        result,
    };

    host.func_new(
        "[constructor]resource1",
        signature(&[("r", u32_ty())], Some(own())),
        move |call, args, results| {
            let Some(Val::U32(rep)) = args.first() else {
                panic!("[constructor]resource1: expected a u32 rep, got {args:?}");
            };
            results[0] = Val::Own(call.resource_new(resource1_id, *rep)?);
            Ok(())
        },
    );
    host.func_new(
        "[static]resource1.assert",
        signature(&[("r", own()), ("rep", u32_ty())], None),
        |_, args, _| {
            let (Some(Val::Own(handle)), Some(Val::U32(rep))) = (args.first(), args.get(1)) else {
                panic!("[static]resource1.assert: expected (own, u32), got {args:?}");
            };
            assert_eq!(handle.rep, *rep, "[static]resource1.assert: rep mismatch");
            Ok(())
        },
    );
    host.func_wrap("[static]resource1.last-drop", {
        let state = state.clone();
        move |_, (): ()| Ok(state.last_drop.load(Ordering::SeqCst))
    });
    host.func_wrap("[static]resource1.drops", {
        let state = state.clone();
        move |_, (): ()| Ok(state.drops.load(Ordering::SeqCst))
    });
    host.func_new(
        "[method]resource1.simple",
        signature(&[("self", borrow()), ("rep", u32_ty())], None),
        |_, args, _| {
            let (Some(Val::Borrow(handle)), Some(Val::U32(rep))) = (args.first(), args.get(1))
            else {
                panic!("[method]resource1.simple: expected (borrow, u32), got {args:?}");
            };
            assert_eq!(handle.rep, *rep, "[method]resource1.simple: rep mismatch");
            Ok(())
        },
    );
    host.func_new(
        "[method]resource1.take-borrow",
        signature(&[("self", borrow()), ("b", borrow())], None),
        |_, args, _| {
            assert!(
                matches!(args, [Val::Borrow(_), Val::Borrow(_)]),
                "[method]resource1.take-borrow: expected two borrows, got {args:?}"
            );
            Ok(())
        },
    );
    host.func_new(
        "[method]resource1.take-own",
        signature(&[("self", borrow()), ("b", own())], None),
        |_, args, _| {
            assert!(
                matches!(args, [Val::Borrow(_), Val::Own(_)]),
                "[method]resource1.take-own: expected a borrow and an own, got {args:?}"
            );
            Ok(())
        },
    );
    host.func_wrap("return-hi", |_, (): ()| Ok("hi".to_owned()));
}

/// Every message in an error's source chain, joined so a trap
/// message anywhere in the chain can be matched.
fn chain(err: &Error) -> String {
    let mut out = String::new();
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = current {
        if !out.is_empty() {
            out.push_str(": ");
        }
        let _ = write!(out, "{e}");
        current = e.source();
    }
    // One line per reason: a substrate trap carries a multi-line
    // backtrace, and the expectation list is line-oriented.
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn directive_span(directive: &WastDirective<'_>) -> Span {
    match directive {
        WastDirective::Module(quote) | WastDirective::ModuleDefinition(quote) => quote.span(),
        WastDirective::ModuleInstance { span, .. }
        | WastDirective::AssertMalformed { span, .. }
        | WastDirective::AssertMalformedCustom { span, .. }
        | WastDirective::AssertInvalid { span, .. }
        | WastDirective::AssertInvalidCustom { span, .. }
        | WastDirective::Register { span, .. }
        | WastDirective::AssertTrap { span, .. }
        | WastDirective::AssertReturn { span, .. }
        | WastDirective::AssertExhaustion { span, .. }
        | WastDirective::AssertUnlinkable { span, .. }
        | WastDirective::AssertException { span, .. }
        | WastDirective::AssertSuspension { span, .. }
        | WastDirective::Wait { span, .. } => *span,
        WastDirective::Invoke(invoke) => invoke.span,
        WastDirective::Thread(thread) => thread.span,
    }
}

fn convert(val: &WastVal<'_>) -> Result<Val, String> {
    Ok(match val {
        WastVal::Bool(v) => Val::Bool(*v),
        WastVal::U8(v) => Val::U8(*v),
        WastVal::S8(v) => Val::S8(*v),
        WastVal::U16(v) => Val::U16(*v),
        WastVal::S16(v) => Val::S16(*v),
        WastVal::U32(v) => Val::U32(*v),
        WastVal::S32(v) => Val::S32(*v),
        WastVal::U64(v) => Val::U64(*v),
        WastVal::S64(v) => Val::S64(*v),
        WastVal::F32(v) => Val::F32(f32::from_bits(v.bits)),
        WastVal::F64(v) => Val::F64(f64::from_bits(v.bits)),
        WastVal::Char(v) => Val::Char(*v),
        WastVal::String(v) => Val::String((*v).to_owned()),
        WastVal::List(items) => Val::List(convert_all(items)?),
        WastVal::Tuple(items) => Val::Tuple(convert_all(items)?),
        WastVal::Record(fields) => {
            let mut out = Vec::with_capacity(fields.len());
            for (name, value) in fields {
                out.push(ValField {
                    name: (*name).to_owned(),
                    value: convert(value)?,
                });
            }
            Val::Record(out.into_boxed_slice())
        }
        WastVal::Variant(name, payload) => Val::Variant {
            discriminant: (*name).to_owned(),
            payload: convert_boxed(payload.as_deref())?,
        },
        WastVal::Enum(name) => Val::Enum((*name).to_owned()),
        WastVal::Option(payload) => Val::Option(convert_boxed(payload.as_deref())?),
        WastVal::Result(result) => Val::Result(match result {
            Ok(payload) => Ok(convert_boxed(payload.as_deref())?),
            Err(payload) => Err(convert_boxed(payload.as_deref())?),
        }),
        WastVal::Flags(names) => Val::Flags(
            names
                .iter()
                .map(|name| (*name).to_owned())
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        ),
    })
}

/// `wast` has no syntax for a `map` value or a fixed-length list, so
/// a directive spells a map as a list of two-element tuples, which is
/// how the canonical ABI lays a map out, and a fixed-length list as a
/// list. Where the function's declared type says `map` or `list<T,
/// N>`, the harness turns such a list into the polyfill's value, for
/// arguments and expected results alike; every other value passes
/// through unchanged, and the polyfill reports the mismatch.
fn coerce(value: Val, ty: &ValueType) -> Val {
    match (value, ty) {
        (Val::List(items), ValueType::Map(map)) => {
            let entry = map.entry();
            let items: Vec<Val> = items
                .into_vec()
                .into_iter()
                .map(|item| coerce(item, &entry))
                .collect();
            if !items
                .iter()
                .all(|item| matches!(item, Val::Tuple(pair) if pair.len() == 2))
            {
                return Val::List(items.into_boxed_slice());
            }
            Val::Map(
                items
                    .into_iter()
                    .map(|item| {
                        let Val::Tuple(pair) = item else {
                            unreachable!("checked above");
                        };
                        let mut pair = pair.into_vec();
                        let value = pair.pop().expect("two elements");
                        let key = pair.pop().expect("two elements");
                        (key, value)
                    })
                    .collect(),
            )
        }
        (Val::List(items), ValueType::FixedLengthList(fixed)) => Val::FixedLengthList(
            items
                .into_vec()
                .into_iter()
                .map(|item| coerce(item, fixed.element()))
                .collect(),
        ),
        (Val::List(items), ValueType::List(list)) => Val::List(
            items
                .into_vec()
                .into_iter()
                .map(|item| coerce(item, list.element()))
                .collect(),
        ),
        (Val::Tuple(items), ValueType::Tuple(tuple)) => Val::Tuple(
            items
                .into_vec()
                .into_iter()
                .zip(
                    tuple
                        .elements()
                        .iter()
                        .chain(std::iter::repeat(&ValueType::Primitive(
                            PrimitiveType::Bool,
                        ))),
                )
                .map(|(item, ty)| coerce(item, ty))
                .collect(),
        ),
        (Val::Record(fields), ValueType::Record(record)) => Val::Record(
            fields
                .into_vec()
                .into_iter()
                .map(|field| {
                    let ty = record
                        .fields()
                        .iter()
                        .find(|candidate| candidate.name() == field.name)
                        .map(|candidate| candidate.ty().clone());
                    ValField {
                        name: field.name,
                        value: match ty {
                            Some(ty) => coerce(field.value, &ty),
                            None => field.value,
                        },
                    }
                })
                .collect(),
        ),
        (Val::Option(Some(inner)), ValueType::Option(option)) => {
            Val::Option(Some(Box::new(coerce(*inner, option.payload()))))
        }
        (Val::Result(Ok(Some(inner))), ValueType::Result(result)) => {
            Val::Result(Ok(Some(Box::new(match result.ok() {
                Some(ty) => coerce(*inner, ty),
                None => *inner,
            }))))
        }
        (Val::Result(Err(Some(inner))), ValueType::Result(result)) => {
            Val::Result(Err(Some(Box::new(match result.err() {
                Some(ty) => coerce(*inner, ty),
                None => *inner,
            }))))
        }
        (
            Val::Variant {
                discriminant,
                payload: Some(inner),
            },
            ValueType::Variant(variant),
        ) => {
            let payload_ty = variant
                .cases()
                .iter()
                .find(|case| case.name() == discriminant)
                .and_then(|case| case.payload().cloned());
            Val::Variant {
                discriminant,
                payload: Some(Box::new(match payload_ty {
                    Some(ty) => coerce(*inner, &ty),
                    None => *inner,
                })),
            }
        }
        (value, _) => value,
    }
}

fn convert_all(items: &[WastVal<'_>]) -> Result<Box<[Val]>, String> {
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        out.push(convert(item)?);
    }
    Ok(out.into_boxed_slice())
}

fn convert_boxed(payload: Option<&WastVal<'_>>) -> Result<Option<Box<Val>>, String> {
    Ok(match payload {
        Some(value) => Some(Box::new(convert(value)?)),
        None => None,
    })
}

/// Structural equality with bit-exact floats, so a directive that
/// expects a particular NaN pattern compares the pattern.
fn vals_equal(a: &Val, b: &Val) -> bool {
    match (a, b) {
        (Val::F32(x), Val::F32(y)) => x.to_bits() == y.to_bits(),
        (Val::F64(x), Val::F64(y)) => x.to_bits() == y.to_bits(),
        (Val::List(x), Val::List(y))
        | (Val::Tuple(x), Val::Tuple(y))
        | (Val::FixedLengthList(x), Val::FixedLengthList(y)) => {
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(a, b)| vals_equal(a, b))
        }
        (Val::Record(x), Val::Record(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y.iter())
                    .all(|(a, b)| a.name == b.name && vals_equal(&a.value, &b.value))
        }
        (
            Val::Variant {
                discriminant: da,
                payload: pa,
            },
            Val::Variant {
                discriminant: db,
                payload: pb,
            },
        ) => da == db && boxed_equal(pa.as_deref(), pb.as_deref()),
        (Val::Option(x), Val::Option(y)) => boxed_equal(x.as_deref(), y.as_deref()),
        (Val::Result(x), Val::Result(y)) => match (x, y) {
            (Ok(a), Ok(b)) | (Err(a), Err(b)) => boxed_equal(a.as_deref(), b.as_deref()),
            _ => false,
        },
        (Val::Flags(x), Val::Flags(y)) => {
            x.len() == y.len() && x.iter().all(|name| y.contains(name))
        }
        (Val::Map(x), Val::Map(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y.iter())
                    .all(|((ka, va), (kb, vb))| vals_equal(ka, kb) && vals_equal(va, vb))
        }
        _ => a == b,
    }
}

fn boxed_equal(a: Option<&Val>, b: Option<&Val>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => vals_equal(a, b),
        (None, None) => true,
        _ => false,
    }
}

/// Run one corpus file against the expectations that name it.
async fn report_file(path: &str, text: &str, expectations: &[Expectation]) -> FileReport {
    let mut runner = Runner::new().await;
    let (directives, failures) = runner.run(text).await;
    let expected = expectations
        .iter()
        .filter(|expectation| expectation.file == path)
        .map(|expectation| Expectation {
            file: expectation.file.clone(),
            line: expectation.line,
            category: expectation.category,
        })
        .collect();
    FileReport {
        path: path.to_owned(),
        directives,
        failures,
        expected,
    }
}

/// Run one corpus file and compare its failures with the expected
/// list. Panics with every unexpected failure and every stale
/// expectation, in the format the list uses, and on a list line
/// without a category.
async fn check(path: &str, text: &str) {
    let expectations = expectations();
    let report = report_file(path, text, &expectations).await;

    let mut out = String::new();
    for failure in report.unexpected() {
        let _ = writeln!(
            out,
            "unexpected: {path}:{} {}",
            failure.line, failure.reason
        );
    }
    for expectation in report.stale() {
        let _ = writeln!(out, "stale expectation: {path}:{}", expectation.line);
    }
    assert!(out.is_empty(), "\n{out}");
}

/// The progress metric: every corpus file in one process, summarized
/// per corpus directory. The per-file tests judge pass or fail; this
/// test only reports, and fails only when the expectation list itself
/// is malformed.
#[wcmp_macros::test]
async fn it_reports_conformance_progress() {
    let expectations = expectations();
    let mut reports: Vec<FileReport> = Vec::with_capacity(CORPUS_FILES.len());
    for (path, text) in CORPUS_FILES {
        reports.push(report_file(path, text, &expectations).await);
    }
    let summary = Summary::new(&reports);
    #[cfg(target_arch = "wasm32")]
    println!("\nwasm32-unknown-unknown\n{}", summary.table());
    #[cfg(not(target_arch = "wasm32"))]
    {
        println!("\nnative\n{}", summary.table());
        // The browser's run cannot print for a passing test, so its
        // summary is projected here from the delta it applies. The
        // projection is exact while `tests web debug` passes.
        let delta = parse_expectations(EXPECTED_FAILURES_WEB)
            .unwrap_or_else(|err| panic!("expected-failures.web.txt: {err}"));
        let web = Summary::new(&report::project(&reports, &delta));
        println!(
            "wasm32-unknown-unknown (projected: these results plus expected-failures.web.txt)\n{}",
            web.table()
        );
        if let Ok(target) = std::env::var("WCMP_CONFORMANCE_SUMMARY") {
            std::fs::write(&target, summary.json())
                .unwrap_or_else(|err| panic!("cannot write the summary to {target}: {err}"));
            let web_target = target.replace(".json", ".web.json");
            std::fs::write(&web_target, web.json())
                .unwrap_or_else(|err| panic!("cannot write the summary to {web_target}: {err}"));
            println!("summary written to {target} and {web_target}");
        }
    }
}

macro_rules! corpus_test {
    ($name:ident, $path:literal) => {
        #[wcmp_macros::test]
        async fn $name() {
            check($path, include_str!(concat!("../corpus/", $path))).await;
        }
    };
}

include!("conformance/manifest.rs");
