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
//! The shared list records the best case: the failures under a
//! suspend provider. Two overlays go on top of it. The web overlay,
//! `expected-failures.web.txt`, holds what only the browser does
//! differently and applies on `wasm32-unknown-unknown`. The
//! no-provider overlay, `expected-failures.no-provider.txt`, holds
//! what fails beyond the shared list in a nested turn, and applies
//! whenever the store runs no guest thread through a provider, on
//! either target: the engine's provider query answers none. Every line of it carries the stack-switch reason, or the
//! run fails. Each file runs twice, with the provider allowed and
//! with it turned off through `EngineConfig`, and the nextest profiles
//! in `.config/nextest.toml` split the two into their own lanes.
//!
//! Every expectation carries a category, and one more test,
//! `it_reports_conformance_progress`, runs every file in one process
//! and prints a summary per corpus directory: directives, passes, and
//! expected failures per category. When `WCMP_CONFORMANCE_SUMMARY`
//! names a file, the test writes the same summary there as JSON.
//!
//! That test is also the regeneration mode of the list: when
//! `WCMP_REGENERATE_EXPECTATIONS` names a list, it rewrites that file
//! from the run, which is what the `tests regenerate` menu command
//! runs. Every failing directive's reason comes from the run, so a
//! change that alters many reasons at once needs no hand loop over the
//! printed `unexpected:` lines. Its twin with the provider turned off,
//! `it_reports_conformance_progress_without_a_provider`, rewrites the
//! no-provider overlay the same way, from the failures the shared list
//! does not name.
//!
//! The host environment is the one Wasmtime's wast runner provides:
//! the fixed set of `host` items its component spectest registers,
//! and the module exports of every named component a file
//! instantiates, reflected into the linker under the component's
//! name.

#![cfg(test)]

#[path = "conformance/report.rs"]
mod report;

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use wasm_component_model_polyfill::{
    Accessor, Component, Engine, EngineConfig, Error, ExternType, ExternalName, FunctionParameter,
    FunctionType, HostResource, Instance, Linker, Module, PrimitiveType, ResourceType,
    SchedulerCause, Store, SuspendProviderKind, Val, ValField, ValueType,
};
use wast::component::WastVal;
use wast::parser::{self, ParseBuffer};
use wast::token::Span;
use wast::{Wast, WastArg, WastDirective, WastExecute, WastRet};

use report::{Expectation, Failure, FileReport, Summary, parse_expectations, parse_overlay};

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

const EXPECTED_FAILURES: &str = include_str!("corpus/expected-failures.txt");

/// The browser-only delta, applied on top of the shared list on
/// `wasm32-unknown-unknown`: differences of the substrate, not of the
/// polyfill. The native progress run reads it too, to project the
/// browser's summary.
const EXPECTED_FAILURES_WEB: &str = include_str!("corpus/expected-failures.web.txt");

/// The overlay of directives that fail only without a suspend
/// provider, applied on top of the shared list whenever the store runs
/// no guest thread through a provider, on either target: a lane that
/// turns the provider off, a native build on a platform without the
/// stack-switching proposal, and a browser without JSPI. A nested turn
/// is one code path on both targets, so one overlay serves both. Every
/// line carries the stack-switch reason.
const EXPECTED_FAILURES_NO_PROVIDER: &str =
    include_str!("corpus/expected-failures.no-provider.txt");

/// The directives of the shared list that pass when no provider runs
/// the store's threads, and fail only under one. The harness drops
/// them from the expectations of such a run. Each names a directive
/// the shared list names. The list is written by hand: `tests
/// regenerate` neither reads nor writes it.
const EXPECTED_PASSES_NO_PROVIDER: &str = include_str!("corpus/expected-passes.no-provider.txt");

/// The expected-failure lists, parsed.
struct Lists {
    /// The failures under a provider, on every target.
    shared: Vec<Expectation>,
    /// What only the browser does differently.
    web: Vec<Expectation>,
    /// What fails beyond `shared` without a provider.
    no_provider: Vec<Expectation>,
    /// The directives of `shared` that pass without a provider, by
    /// file and line.
    passes_without_provider: Vec<(String, usize)>,
}

impl Lists {
    /// The lists compiled into the harness. A malformed list panics,
    /// which fails every test that judges a run by it.
    fn compiled() -> Self {
        Self::parse(
            EXPECTED_FAILURES,
            EXPECTED_FAILURES_WEB,
            EXPECTED_FAILURES_NO_PROVIDER,
        )
        .and_then(|lists| lists.with_passes(EXPECTED_PASSES_NO_PROVIDER))
        .unwrap_or_else(|err| panic!("{err}"))
    }

    /// Add the directives of the shared list that pass without a
    /// provider, one `<path>:<line> <reason>` per line. A line that
    /// names no directive of the shared list is refused, so the list
    /// cannot outlive the failure it excuses.
    fn with_passes(mut self, text: &str) -> Result<Self, String> {
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let number = index + 1;
            let location = line.split_whitespace().next().unwrap_or_default();
            let parsed = location
                .rsplit_once(':')
                .and_then(|(file, directive)| Some((file.to_owned(), directive.parse().ok()?)));
            let Some((file, directive)) = parsed else {
                return Err(format!(
                    "expected-passes.no-provider.txt:{number}: `{line}` does not start with \
                     `<path>:<line>`"
                ));
            };
            if !self
                .shared
                .iter()
                .any(|expectation| expectation.file == file && expectation.line == directive)
            {
                return Err(format!(
                    "expected-passes.no-provider.txt:{number}: `{file}:{directive}` is not in \
                     expected-failures.txt: the list excuses only a failure the shared list names"
                ));
            }
            self.passes_without_provider.push((file, directive));
        }
        Ok(self)
    }

    /// Parse the three lists. Every line of the overlay must carry the
    /// stack-switch reason, and none may name a directive the shared
    /// list already names.
    fn parse(shared: &str, web: &str, no_provider: &str) -> Result<Self, String> {
        let shared = parse_expectations(shared)?;
        let web =
            parse_expectations(web).map_err(|err| format!("expected-failures.web.txt: {err}"))?;
        let no_provider = parse_overlay(no_provider, &stack_switch_reason())
            .map_err(|err| format!("expected-failures.no-provider.txt: {err}"))?;
        if let Some(twice) = no_provider.iter().find(|overlay| {
            shared
                .iter()
                .any(|line| line.file == overlay.file && line.line == overlay.line)
        }) {
            return Err(format!(
                "expected-failures.no-provider.txt: `{}:{}` is in expected-failures.txt too: the \
                 overlay lists only what fails beyond the shared list",
                twice.file, twice.line
            ));
        }
        Ok(Self {
            shared,
            web,
            no_provider,
            passes_without_provider: Vec::new(),
        })
    }

    /// The expectations a run on this target judges a file by, for an
    /// engine whose provider query answered `provider`: the shared
    /// list, the web delta in a browser, and, when the store runs no
    /// guest thread through a provider, the overlay in place of the
    /// shared directives that pass without one.
    fn applying(&self, provider: SuspendProviderKind) -> Vec<Expectation> {
        let mut all = self.shared.clone();
        if cfg!(target_arch = "wasm32") {
            all.extend(self.web.iter().cloned());
        }
        if !runs_threads_through(provider) {
            all.retain(|expectation| {
                !self
                    .passes_without_provider
                    .iter()
                    .any(|(file, line)| *file == expectation.file && *line == expectation.line)
            });
            all.extend(self.no_provider.iter().cloned());
        }
        all
    }
}

/// Whether a store of an engine that answered `provider` runs its
/// guest threads through that provider, so that a block suspends
/// rather than waiting in a nested turn.
///
/// Every provider does: the stack-switching provider natively, and
/// the JSPI provider in a browser that ships JSPI. Only an engine that
/// answers none serves every block with the nested turn, and only
/// then does the overlay apply.
fn runs_threads_through(provider: SuspendProviderKind) -> bool {
    provider != SuspendProviderKind::None
}

/// The reason every line of the overlay carries: the text of the
/// stack-switch cause, which a block whose releasing work lies on a
/// frame below it fails with in a nested turn.
fn stack_switch_reason() -> String {
    SchedulerCause::StackSwitchNeeded.to_string()
}

/// The 4-byte version word after `\0asm` in a core module.
const CORE_MODULE_VERSION: [u8; 4] = [0x01, 0x00, 0x00, 0x00];

/// The host state a `.wast` file runs against: one engine, the
/// components defined so far, and the instances created so far, each
/// in a store of its own.
///
/// Every component instantiation gets a fresh store, as Wasmtime's wast
/// runner gives one (`instantiate_component` in its `crates/wast`),
/// and the instance lives in that store alone. What a later directive
/// shares with an earlier one is what that runner shares: the engine,
/// the linker with the spectest items and the core modules a named
/// component exports, and the component definitions. A task, a thread,
/// or a callback an earlier call left behind stays in the earlier
/// instance's store, so it never runs during a later directive's call,
/// as it never does under Wasmtime, where a trap leaves a store that
/// can only be dropped.
///
/// A store lives as long as its instance can still be reached: while
/// the instance is current, which is the instance an unqualified
/// `invoke` targets, or bound to a name. Wasmtime's runner keeps only
/// the current instance's store, and drops it when the next
/// instantiation replaces it. This harness also invokes a named
/// instance, so it keeps the store of every instance a name still
/// binds, and drops the others the moment they become unreachable.
/// Dropping a store drops whatever its guest threads left suspended,
/// with no destructor run.
struct Runner {
    engine: Engine,
    linker: Linker<()>,
    definitions: HashMap<String, Component>,
    last_definition: Option<Component>,
    /// Every instance created so far, in creation order, with the
    /// store it lives in, until the instance can no longer be reached
    /// and its store is dropped.
    instances: Vec<Option<(Store<()>, Instance)>>,
    /// Named instances, as indices into `instances`.
    named: HashMap<String, usize>,
    /// Every name whose module exports are reflected into the linker,
    /// with the component and instance the reflection was built from.
    /// A rebuild of the linker replays them.
    reflected: HashMap<String, (Component, usize)>,
    /// The instance an unqualified `invoke` targets.
    current: Option<usize>,
    /// Whether the file is from the Component Model's own suite
    /// (`cm/`), whose trap wording `trap_matches` relaxes.
    cm_corpus: bool,
}

impl Runner {
    async fn new(config: &EngineConfig, cm_corpus: bool) -> Self {
        let engine = Engine::with_config(config).expect("engine");
        let mut linker = Linker::new(&engine);
        link_spectest(&engine, &mut linker).await;
        Self {
            engine,
            linker,
            definitions: HashMap::new(),
            last_definition: None,
            instances: Vec::new(),
            named: HashMap::new(),
            reflected: HashMap::new(),
            current: None,
            cm_corpus,
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
                self.make_current(None);
                let name = quote.name().map(|id| id.name().to_owned());
                // A directive that fails leaves neither a named
                // instance nor a linker registration under its name,
                // so a later directive that names it, or imports the
                // items the name reflects, reports the cascade rather
                // than running against whatever the name was bound to
                // before.
                if let Some(name) = &name {
                    self.unbind(name).await;
                }
                let bytes = quote.encode().map_err(|err| format!("encode: {err}"))?;
                let component = self.component(&bytes).await?;
                let index = self.instantiate(&component).await?;
                if let Some(name) = name {
                    self.register_named(&name, &component, index);
                    self.bind(name, index);
                }
                self.make_current(Some(index));
                Ok(())
            }
            WastDirective::ModuleDefinition(mut quote) => {
                let name = quote.name().map(|id| id.name().to_owned());
                // A definition that fails leaves no definition of its
                // name, so a later directive that names it reports the
                // cascade rather than running whatever the name was
                // bound to before.
                if let Some(name) = &name {
                    self.definitions.remove(name);
                }
                self.last_definition = None;
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
                self.make_current(None);
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
                    self.bind(id.name().to_owned(), index);
                }
                self.make_current(Some(index));
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
                    if trap_matches(self.cm_corpus, message, &err) {
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
                // Text that does not encode is rejected before the
                // polyfill sees it. Either way the rejection must say
                // what the directive says, as Wasmtime's wast runner
                // requires: a component rejected for another reason
                // does not satisfy the assertion.
                let err = match module.encode() {
                    Ok(bytes) => match self.component(&bytes).await {
                        Ok(_) => {
                            return Err(format!(
                                "expected rejection `{message}`, but the component parsed"
                            ));
                        }
                        Err(err) => err,
                    },
                    Err(err) => format!("encode: {err}"),
                };
                if err.contains(message) {
                    Ok(())
                } else {
                    Err(format!("expected rejection `{message}`, got `{err}`"))
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
                    Ok(index) => {
                        self.release(index);
                        Err(format!("expected link failure `{message}`, but it linked"))
                    }
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

    /// Instantiate `component` in a store of its own and return its
    /// index in `instances`. A store whose instantiation fails is
    /// dropped with the failure.
    async fn instantiate(&mut self, component: &Component) -> Result<usize, String> {
        let mut store = Store::new(&self.engine, ()).expect("store");
        let instance = self
            .linker
            .instantiate(&mut store, component)
            .await
            .map_err(|err| format!("instantiation failed: {}", chain(&err)))?;
        self.instances.push(Some((store, instance)));
        Ok(self.instances.len() - 1)
    }

    /// Make the instance at `index`, or none, the one an unqualified
    /// `invoke` targets, and drop the store of the instance that was
    /// current when nothing else reaches it.
    fn make_current(&mut self, index: Option<usize>) {
        let previous = core::mem::replace(&mut self.current, index);
        if let Some(previous) = previous
            && Some(previous) != index
        {
            self.release(previous);
        }
    }

    /// Bind `name` to the instance at `index`, and drop the store of
    /// the instance the name bound before when nothing else reaches
    /// it.
    fn bind(&mut self, name: String, index: usize) {
        if let Some(previous) = self.named.insert(name, index)
            && previous != index
        {
            self.release(previous);
        }
    }

    /// Drop the store of the instance at `index` unless it is current,
    /// a name binds it, or a name reflects its module exports into the
    /// linker: nothing can reach the instance any more.
    fn release(&mut self, index: usize) {
        let reachable = self.current == Some(index)
            || self.named.values().any(|named| *named == index)
            || self
                .reflected
                .values()
                .any(|(_, reflected)| *reflected == index);
        if !reachable && let Some(slot) = self.instances.get_mut(index) {
            *slot = None;
        }
    }

    /// Undo `register_named` for a name: drop the named instance and
    /// the root-level linker entry the registration reflects it into.
    /// `Linker`'s public surface adds registrations and never removes
    /// one, so the harness builds a fresh linker and reflects every
    /// name that is still bound into it. A name that reflected
    /// nothing costs nothing: the rebuild runs only when the name
    /// being unbound had a registration of its own.
    async fn unbind(&mut self, name: &str) {
        let previous = self.named.remove(name);
        let reflected = self.reflected.remove(name);
        if let Some(previous) = previous {
            self.release(previous);
        }
        if reflected.is_none() {
            return;
        }
        let mut linker = Linker::new(&self.engine);
        link_spectest(&self.engine, &mut linker).await;
        self.linker = linker;
        let bound: Vec<(String, Component, usize)> = self
            .reflected
            .iter()
            .map(|(bound_name, (component, index))| (bound_name.clone(), component.clone(), *index))
            .collect();
        for (bound_name, component, index) in bound {
            self.register_named(&bound_name, &component, index);
        }
    }

    /// Reflect a named component's module exports into the linker
    /// under the component's name, as Wasmtime's runner does, so a
    /// later directive can import them. Functions are not reflected
    /// there either.
    fn register_named(&mut self, name: &str, component: &Component, index: usize) {
        self.reflected
            .insert(name.to_owned(), (component.clone(), index));
        let Some((_, instance)) = &self.instances[index] else {
            return;
        };
        let mut root = self.linker.root();
        let mut registration = root.instance(name);
        for export in component.exports.iter() {
            let (ExternType::Module(_), ExternalName::Plain(export_name)) =
                (&export.ty, &export.name)
            else {
                continue;
            };
            if let Some(module) = instance.get_module(export_name) {
                registration
                    .module(export_name, &module)
                    .expect("the registration");
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
                let index = self.instantiate(&component).await?;
                self.release(index);
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
        let (store, instance) = self.instances[index]
            .as_mut()
            .ok_or_else(|| "the instance's store was dropped".to_owned())?;
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
        let results = func.call(store, &values).await.map_err(|err| chain(&err))?;
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

/// A future that is pending the first time it is polled and ready
/// with `value` afterwards: what a single yield on Wasmtime's
/// executor does to the body of a host `async` function, expressed
/// against the polyfill's store, which polls a host task once per
/// turn.
///
/// The waker is woken before the future parks, because a driver that
/// finds only a host task pending parks too: the wake is what brings
/// it back for the turn that completes the call.
struct YieldOnce<V> {
    polled: bool,
    value: Option<V>,
}

impl<V> YieldOnce<V> {
    fn new(value: V) -> Self {
        Self {
            polled: false,
            value: Some(value),
        }
    }
}

impl<V: Unpin> Future for YieldOnce<V> {
    type Output = Result<V, Error>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if this.polled {
            let value = this.value.take().expect("the future is polled once ready");
            return Poll::Ready(Ok(value));
        }
        this.polled = true;
        context.waker().wake_by_ref();
        Poll::Pending
    }
}

/// Register the host items Wasmtime's wast runner provides for its
/// component tests (`crates/wast/src/spectest.rs`,
/// `link_component_spectest`), so the directives that import them
/// run as they do there.
///
/// Five of them are declared `async func`, so the link rule wants a
/// concurrent registration for each, and each behaves as the runner
/// makes it behave. `host-echo-u32` resolves at once with its
/// argument. `never-return` and `[method]resource1.never-return` stay
/// pending for ever. `echo-slowly` and `return-two-slowly` are
/// pending once and resolve at the next poll, which is what the
/// runner's single `yield_now` on Wasmtime's executor comes to.
///
/// `[method]resource1.never-return` takes a `borrow<resource1>`, and
/// the typed entries derive no signature for a resource handle, so it
/// goes through the untyped concurrent entry with the signature
/// written out — exactly as the synchronous methods of the same
/// resource go through the untyped synchronous entry.
async fn link_spectest(engine: &Engine, linker: &mut Linker<()>) {
    linker
        .root()
        .func_wrap("host-return-two", |_, (): ()| Ok(2u32))
        .expect("the registration");
    linker
        .root()
        .func_wrap_concurrent(
            "host-echo-u32",
            |_: &Accessor<()>, (v,): (u32,)| async move { Ok(v) },
        )
        .expect("the registration");

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
    host.func_wrap("return-three", |_, (): ()| Ok(3u32))
        .expect("the registration");
    host.instance("nested")
        .func_wrap("return-four", |_, (): ()| Ok(4u32))
        .expect("the registration");
    host.module("simple-module", &simple_module)
        .expect("the registration");

    let resource1_id = host
        .resource_with("resource1", resource1.clone())
        .expect("the registration");
    host.resource("resource2", |_, _| Ok(()))
        .expect("the registration");
    // The same resource type under a second name, as the runner
    // registers it.
    host.resource_with("resource1-again", resource1)
        .expect("the registration");

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
        async_: false,
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
    )
    .expect("the registration");
    host.func_new(
        "[static]resource1.assert",
        signature(&[("r", own()), ("rep", u32_ty())], None),
        |_, args, _| {
            let (Some(Val::Own(handle)), Some(Val::U32(rep))) = (args.first(), args.get(1)) else {
                panic!("[static]resource1.assert: expected (own, u32), got {args:?}");
            };
            assert_eq!(handle.rep(), *rep, "[static]resource1.assert: rep mismatch");
            Ok(())
        },
    )
    .expect("the registration");
    host.func_wrap("[static]resource1.last-drop", {
        let state = state.clone();
        move |_, (): ()| Ok(state.last_drop.load(Ordering::SeqCst))
    })
    .expect("the registration");
    host.func_wrap("[static]resource1.drops", {
        let state = state.clone();
        move |_, (): ()| Ok(state.drops.load(Ordering::SeqCst))
    })
    .expect("the registration");
    host.func_new(
        "[method]resource1.simple",
        signature(&[("self", borrow()), ("rep", u32_ty())], None),
        |_, args, _| {
            let (Some(Val::Borrow(handle)), Some(Val::U32(rep))) = (args.first(), args.get(1))
            else {
                panic!("[method]resource1.simple: expected (borrow, u32), got {args:?}");
            };
            assert_eq!(handle.rep(), *rep, "[method]resource1.simple: rep mismatch");
            Ok(())
        },
    )
    .expect("the registration");
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
    )
    .expect("the registration");
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
    )
    .expect("the registration");
    host.func_wrap_concurrent("never-return", |_: &Accessor<()>, (): ()| {
        core::future::pending::<Result<(), Error>>()
    })
    .expect("the registration");
    host.func_wrap_concurrent("return-two-slowly", |_: &Accessor<()>, (): ()| {
        YieldOnce::new(2i32)
    })
    .expect("the registration");
    host.func_wrap_concurrent("echo-slowly", |_: &Accessor<()>, (a,): (u32,)| {
        YieldOnce::new(a)
    })
    .expect("the registration");
    host.func_new_concurrent(
        "[method]resource1.never-return",
        FunctionType {
            async_: true,
            ..signature(&[("self", borrow())], None)
        },
        |_: &Accessor<()>, _args: Vec<Val>| core::future::pending::<Result<Vec<Val>, Error>>(),
    )
    .expect("the registration");
    host.func_wrap("return-hi", |_, (): ()| Ok("hi".to_owned()))
        .expect("the registration");

    // The `wasmtime` instance the runner registers beside the
    // spectest for its own misc tests. Its `gc` collects the
    // engine's garbage, which the polyfill's substrate does on its
    // own, so the function is here to be called and does nothing. A
    // file imports it to force a destructor's deferred thread into a
    // real one partway through.
    linker
        .root()
        .instance("wasmtime")
        .func_wrap("gc", |_, (): ()| Ok(()))
        .expect("the registration");
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

/// Whether a trap raised with `actual` satisfies an `assert_trap`
/// that expects `message`: `actual` contains `message`, or, in a file
/// of the Component Model's own suite, both contain "cannot write" or
/// both contain "cannot read".
///
/// The second arm mirrors Wasmtime's wast runner
/// (`crates/wast/src/wast.rs`, `assert_trap`, lines 551-554 at the
/// commit the corpora are drawn from). It accepts any trap containing
/// "cannot write" when the expected text contains it, and the same
/// for "cannot read", because "upstream component model tests expect
/// slightly different error messages than we generate". The spec
/// fixes no wording for these traps, and the polyfill raises
/// Wasmtime's, so the Component Model suite is held to the rule that
/// Wasmtime passes it by. Wasmtime's own suite expects Wasmtime's
/// wording and gets no relaxation.
///
/// The rule is not confined to a copy on a done end: it reaches every
/// expected text with either phrase. The refusal of a read and a write
/// from one instance is one such text ("cannot read from and write to
/// intra-component future"), and any trap that says "cannot read"
/// would satisfy it. The polyfill's refusal contains that text as
/// written, so it passes on the first arm.
fn trap_matches(cm_corpus: bool, message: &str, actual: &str) -> bool {
    actual.contains(message)
        || (cm_corpus
            && ((message.contains("cannot write") && actual.contains("cannot write"))
                || (message.contains("cannot read") && actual.contains("cannot read"))))
}

/// The engine configuration a corpus file runs with, as Wasmtime's
/// wast runner configures it (`crates/test-util/src/wast.rs`
/// upstream). A file of the Component Model's own suite (`cm/`) gets
/// every gated feature the runner turns on for that suite; a file of
/// Wasmtime's suite starts from the polyfill's defaults. Either way a
/// `;;! component_model_<feature> = <bool>` header line then sets
/// that feature. Core-Wasm flags (`gc`, `memory64`, `multi_memory`,
/// ...) name features the polyfill validates with already and are
/// ignored.
fn engine_config(path: &str, text: &str) -> EngineConfig {
    let mut config = EngineConfig::new();
    if path.starts_with("cm/") {
        config
            .wasm_component_model_implements(true)
            .wasm_component_model_more_async_builtins(true)
            .wasm_component_model_async_stackful(true)
            .wasm_component_model_threading(true);
    }
    for line in text.lines() {
        let Some(directive) = line.strip_prefix(";;!") else {
            if line.trim().is_empty() || line.starts_with(";;") {
                continue;
            }
            break;
        };
        let Some((key, value)) = directive.split_once('=') else {
            continue;
        };
        let enable = value.trim() == "true";
        match key.trim() {
            "component_model_implements" => config.wasm_component_model_implements(enable),
            "component_model_map" => config.wasm_component_model_map(enable),
            "component_model_fixed_length_lists" => {
                config.wasm_component_model_fixed_length_lists(enable)
            }
            "component_model_memory64" => config.wasm_component_model_memory64(enable),
            "component_model_error_context" => config.wasm_component_model_error_context(enable),
            "component_model_gc" => config.wasm_component_model_gc(enable),
            "component_model_async_stackful" => config.wasm_component_model_async_stackful(enable),
            "component_model_threading" => config.wasm_component_model_threading(enable),
            "component_model_more_async_builtins" => {
                config.wasm_component_model_more_async_builtins(enable)
            }
            _ => &mut config,
        };
    }
    config
}

/// Run one corpus file, with the suspend provider allowed or turned
/// off through `EngineConfig`, against the expectations that name it.
/// The lists that apply follow the engine's own answer to which
/// provider it selected, not the setting: the overlay applies whenever
/// the engine has none, which a target without a provider gives with
/// the setting on.
async fn report_file(path: &str, text: &str, lists: &Lists, suspend_provider: bool) -> FileReport {
    let mut config = engine_config(path, text);
    config.suspend_provider(suspend_provider);
    let mut runner = Runner::new(&config, path.starts_with("cm/")).await;
    let expected = lists
        .applying(runner.engine.suspend_provider())
        .into_iter()
        .filter(|expectation| expectation.file == path)
        .collect();
    let (directives, failures) = runner.run(text).await;
    FileReport {
        path: path.to_owned(),
        directives,
        failures,
        expected,
    }
}

/// Run one corpus file and compare its failures with the expected
/// lists. Panics with every unexpected failure and every stale
/// expectation, in the format the list uses, and on a malformed list.
async fn check(path: &str, text: &str, suspend_provider: bool) {
    let lists = Lists::compiled();
    let report = report_file(path, text, &lists, suspend_provider).await;

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
/// test only reports, and fails only when an expectation list itself
/// is malformed.
#[wcmp_macros::test]
async fn it_reports_conformance_progress() {
    progress(true).await;
}

/// The progress metric with the suspend provider turned off, which
/// is also the regeneration mode of the overlay: when
/// `WCMP_REGENERATE_EXPECTATIONS` names a list, the test rewrites it
/// from the failures that `WCMP_REGENERATE_BASE`, the shared list, does
/// not name already. Without that variable the base is the shared
/// list the harness compiled in.
#[wcmp_macros::test]
async fn it_reports_conformance_progress_without_a_provider() {
    progress(false).await;
}

async fn progress(suspend_provider: bool) {
    let lists = Lists::compiled();
    let mut reports: Vec<FileReport> = Vec::with_capacity(CORPUS_FILES.len());
    for (path, text) in CORPUS_FILES {
        reports.push(report_file(path, text, &lists, suspend_provider).await);
    }
    let summary = Summary::new(&reports);
    let state = if suspend_provider {
        ""
    } else {
        ", suspend provider off"
    };
    #[cfg(target_arch = "wasm32")]
    println!("\nwasm32-unknown-unknown{state}\n{}", summary.table());
    #[cfg(not(target_arch = "wasm32"))]
    {
        println!("\nnative{state}\n{}", summary.table());
        // The browser's run cannot print for a passing test, so its
        // summary is projected here from the delta it applies. The
        // projection is exact while the browser's lane in the same
        // state passes.
        let web = Summary::new(&report::project(&reports, &lists.web));
        println!(
            "wasm32-unknown-unknown{state} (projected: these results plus \
             expected-failures.web.txt)\n{}",
            web.table()
        );
        if suspend_provider && let Ok(target) = std::env::var("WCMP_CONFORMANCE_SUMMARY") {
            std::fs::write(&target, summary.json())
                .unwrap_or_else(|err| panic!("cannot write the summary to {target}: {err}"));
            let web_target = target.replace(".json", ".web.json");
            std::fs::write(&web_target, web.json())
                .unwrap_or_else(|err| panic!("cannot write the summary to {web_target}: {err}"));
            println!("summary written to {target} and {web_target}");
        }
        if let Ok(list) = std::env::var("WCMP_REGENERATE_EXPECTATIONS") {
            if suspend_provider {
                regenerate_expectations(&list, &reports);
            } else {
                let base = match std::env::var("WCMP_REGENERATE_BASE") {
                    Ok(base) => std::fs::read_to_string(&base).unwrap_or_else(|err| {
                        panic!("cannot read the shared expectation list {base}: {err}")
                    }),
                    Err(_) => EXPECTED_FAILURES.to_owned(),
                };
                let beyond = report::beyond(&reports, &base)
                    .unwrap_or_else(|err| panic!("the shared list: {err}"));
                regenerate_expectations(&list, &beyond);
            }
        }
    }
}

/// The directives whose block only a frame below the blocked thread
/// can release: a caller that started the blocked callee from inside
/// its own frame, and that would go on once control returned to it.
/// A provider runs the callee on a stack of its own, so the caller
/// gets control back and releases the block. A nested turn runs the
/// callee above the caller on the one real stack, so the block fails
/// with the stack-switch cause, and never with the deadlock cause,
/// because the reference permits it. The entries of one file are
/// adjacent.
const FRAME_BELOW_DIRECTIVES: &[(&str, usize)] = &[
    ("cm/async/async-calls-sync.wast", 250),
    ("cm/async/async-calls-sync.wast", 251),
    ("cm/async/cancel-and-exclusive-lock.wast", 196),
    ("cm/async/sync-streams.wast", 208),
    ("wasmtime/async/reenter-during-yield.wast", 81),
    ("wasmtime/async/stream-zero-ops.wast", 201),
    ("wasmtime/async/streams-massive-send.wast", 238),
    ("wasmtime/async/streams-massive-send.wast", 240),
    ("wasmtime/async/sync-streams.wast", 186),
    ("wasmtime/async/task-builtins.wast", 723),
    ("wasmtime/async/trap-if-done.wast", 594),
    ("wasmtime/async/trap-if-done.wast", 596),
    ("wasmtime/async/trap-if-done.wast", 612),
    ("wasmtime/async/trap-if-done.wast", 614),
    ("wasmtime/async/trap-if-done.wast", 623),
    ("wasmtime/async/trap-if-done.wast", 625),
    ("wasmtime/async/trap-if-done.wast", 627),
];

/// The one frame-below directive that also cancels a subtask. Under a
/// provider its block is served, and it goes on to fail at
/// `subtask.cancel`, which the polyfill does not build.
const FRAME_BELOW_CANCEL: (&str, usize) = ("cm/async/cancel-and-exclusive-lock.wast", 196);

/// Every frame-below directive, in both provider states: served where
/// a provider runs the store's threads, and failed with the
/// stack-switch cause, not the deadlock cause, where none does. The
/// corpus tests compare a failure with its list by file and line only,
/// so this test is what holds each directive to its cause, on both
/// targets.
#[wcmp_macros::test]
async fn it_serves_every_frame_below_block_under_a_provider_and_fails_it_with_the_stack_switch_cause_in_a_nested_turn()
 {
    let stack_switch = stack_switch_reason();
    let deadlock = SchedulerCause::Deadlock.to_string();
    let mut paths: Vec<&str> = FRAME_BELOW_DIRECTIVES
        .iter()
        .map(|(path, _)| *path)
        .collect();
    paths.dedup();

    let mut out = String::new();
    for suspend_provider in [true, false] {
        for path in &paths {
            let (_, text) = CORPUS_FILES
                .iter()
                .find(|(file, _)| file == path)
                .unwrap_or_else(|| panic!("{path} is not in the corpus"));
            let mut config = engine_config(path, text);
            config.suspend_provider(suspend_provider);
            let mut runner = Runner::new(&config, path.starts_with("cm/")).await;
            let threads = runs_threads_through(runner.engine.suspend_provider());
            let (_, failures) = runner.run(text).await;
            for (_, line) in FRAME_BELOW_DIRECTIVES
                .iter()
                .filter(|(file, _)| file == path)
            {
                let reason = failures
                    .iter()
                    .find(|failure| failure.line == *line)
                    .map(|failure| failure.reason.as_str());
                let state = if threads {
                    "under a provider"
                } else {
                    "in a nested turn"
                };
                let held = match reason {
                    None => threads && (*path, *line) != FRAME_BELOW_CANCEL,
                    Some(reason) if threads && (*path, *line) == FRAME_BELOW_CANCEL => {
                        reason.contains("subtask.cancel")
                            && !reason.contains(&stack_switch)
                            && !reason.contains(&deadlock)
                    }
                    Some(reason) => {
                        !threads && reason.contains(&stack_switch) && !reason.contains(&deadlock)
                    }
                };
                if !held {
                    let _ = writeln!(out, "{path}:{line} {state}: {}", reason.unwrap_or("passes"));
                }
            }
        }
    }
    assert!(out.is_empty(), "\n{out}");
}

/// Rewrite an expected-failures list from this run, in place: a
/// directive that still fails keeps its line's category and any
/// hand-written parenthetical and takes the run's reason, a directive
/// that passes now loses its line, and a directive the list does not
/// name arrives with the placeholder category, which the harness
/// rejects until a person replaces it.
///
/// `WCMP_REGENERATE_EXPECTATIONS` names the list to rewrite. The
/// `tests regenerate` menu command points it at a copy of the
/// checked-in list and installs the result, or, for a dry run, only
/// prints the diff. The list the harness compiled in is what this run
/// measured against; the list named here is what the merge reads, so
/// the reasons come from the run and the categories from the file on
/// disk.
#[cfg(not(target_arch = "wasm32"))]
fn regenerate_expectations(list: &str, reports: &[FileReport]) {
    let current = std::fs::read_to_string(list)
        .unwrap_or_else(|err| panic!("cannot read the expectation list {list}: {err}"));
    let regeneration =
        report::regenerate(&current, reports).unwrap_or_else(|err| panic!("{list}: {err}"));
    std::fs::write(list, &regeneration.text)
        .unwrap_or_else(|err| panic!("cannot write the expectation list {list}: {err}"));
    println!(
        "\nregenerated {list}: {} kept, {} dropped, {} new",
        regeneration.kept,
        regeneration.dropped.len(),
        regeneration.added.len()
    );
    for line in &regeneration.dropped {
        println!("passes now: {line}");
    }
    for line in &regeneration.added {
        println!("new failure: {line}");
    }
    if !regeneration.added.is_empty() {
        println!(
            "{} new line(s) carry the placeholder category `{}`: read each failure and write the \
             category that names its cause, or the next run rejects the list.",
            regeneration.added.len(),
            report::PLACEHOLDER_CATEGORY
        );
    }
}

/// The harness's own behavior, on `.wast` text written for the test
/// rather than vendored: bookkeeping the corpora cannot assert,
/// because a corpus file only ever states what the reference
/// implementation does.
#[cfg(test)]
mod tests {
    use super::*;

    /// Run `text` as a file and return one string per directive:
    /// `None` where the directive passed, the failure's reason where
    /// it did not.
    async fn outcomes(text: &str) -> Vec<Option<String>> {
        let mut runner = Runner::new(&EngineConfig::new(), false).await;
        let (directives, failures) = runner.run(text).await;
        let mut lines: Vec<Option<String>> = vec![None; directives];
        let numbers: Vec<usize> = text
            .lines()
            .enumerate()
            .filter(|(_, line)| line.starts_with('('))
            .map(|(index, _)| index + 1)
            .collect();
        assert_eq!(numbers.len(), directives, "one directive per opening line");
        for failure in failures {
            let index = numbers
                .iter()
                .position(|line| *line == failure.line)
                .unwrap_or_else(|| panic!("failure on line {} is not a directive", failure.line));
            lines[index] = Some(failure.reason);
        }
        lines
    }

    /// A core module whose function leaves two values on the stack
    /// where its result type declares one: text that encodes and that
    /// no conforming implementation accepts.
    const INVALID: &str = "(component definition $A
  (component
    (core module $m
      (func (export \"f\") (result i32) i32.const 1 i32.const 2)
    )
    (core instance (instantiate $m))
  ))";

    const SHARED: &str = "cm/x.wast:1 cascade no instance to invoke\n";
    const WEB: &str = "cm/x.wast:2 substrate a difference of the browser\n";

    /// An overlay whose one line carries the stack-switch reason.
    fn overlay(line: usize) -> String {
        format!(
            "cm/x.wast:{line} deferred-feature the call fails with `{}`\n",
            stack_switch_reason()
        )
    }

    fn lines(expectations: &[Expectation]) -> Vec<usize> {
        let mut lines: Vec<usize> = expectations.iter().map(|e| e.line).collect();
        lines.sort_unstable();
        lines
    }

    #[wcmp_macros::test]
    async fn it_drops_a_shared_directive_that_passes_without_a_provider_only_in_such_a_run() {
        let lists = Lists::parse(SHARED, "", &overlay(3))
            .and_then(|lists| lists.with_passes("cm/x.wast:1 passes by another route\n"))
            .expect("parses");
        assert_eq!(
            lines(&lists.applying(SuspendProviderKind::StackSwitching)),
            [1],
            "under a provider the shared directive is still expected to fail"
        );
        assert_eq!(
            lines(&lists.applying(SuspendProviderKind::None)),
            [3],
            "without one it is expected to pass, and the overlay applies"
        );
    }

    #[wcmp_macros::test]
    async fn it_refuses_a_pass_without_a_provider_the_shared_list_does_not_name() {
        let err = Lists::parse(SHARED, "", "")
            .and_then(|lists| lists.with_passes("cm/x.wast:7 not a failure of the shared list\n"))
            .err()
            .expect("rejected");
        assert!(
            err.starts_with("expected-passes.no-provider.txt:1:"),
            "{err}"
        );
    }

    #[wcmp_macros::test]
    async fn it_applies_the_overlay_exactly_when_no_provider_runs_the_threads() {
        let lists = Lists::parse(SHARED, "", &overlay(3)).expect("parses");
        assert_eq!(
            lines(&lists.applying(SuspendProviderKind::StackSwitching)),
            [1]
        );
        // The JSPI provider runs guest threads as the stack-switching
        // provider does, so a store of an engine that selected it
        // blocks as one under that provider does.
        assert_eq!(lines(&lists.applying(SuspendProviderKind::Jspi)), [1]);
        assert_eq!(lines(&lists.applying(SuspendProviderKind::None)), [1, 3]);
    }

    #[wcmp_macros::test]
    async fn it_applies_the_web_delta_beside_the_overlay_in_a_browser() {
        let lists = Lists::parse(SHARED, WEB, &overlay(3)).expect("parses");
        let applied = lines(&lists.applying(SuspendProviderKind::None));
        if cfg!(target_arch = "wasm32") {
            assert_eq!(applied, [1, 2, 3]);
        } else {
            assert_eq!(applied, [1, 3]);
        }
    }

    #[wcmp_macros::test]
    async fn it_judges_a_file_by_the_engines_own_answer() {
        const PATH: &str = "cm/x.wast";
        const TEXT: &str = "(assert_return (invoke \"f\"))\n";
        let lists = Lists::parse("", "", &overlay(1)).expect("parses");

        // Turned off, the engine answers none, so the overlay applies.
        let off = report_file(PATH, TEXT, &lists, false).await;
        assert_eq!(lines(&off.expected), [1]);
        assert_eq!(off.unexpected().count(), 0);
        assert_eq!(off.stale().count(), 0);

        // Allowed, the overlay applies only if the engine found no
        // provider that runs the store's threads, which is what a
        // target without one answers.
        let answer = Engine::with_config(&engine_config(PATH, TEXT))
            .expect("engine")
            .suspend_provider();
        let on = report_file(PATH, TEXT, &lists, true).await;
        assert_eq!(
            on.expected.len() == 1,
            !runs_threads_through(answer),
            "the engine answered {answer:?}"
        );
    }

    #[wcmp_macros::test]
    async fn it_fails_a_run_on_an_overlay_line_without_the_stack_switch_reason() {
        let overlay = format!("{}cm/x.wast:4 cascade no instance to invoke\n", overlay(3));
        let err = Lists::parse(SHARED, WEB, &overlay).err().expect("rejected");
        assert!(
            err.starts_with("expected-failures.no-provider.txt:"),
            "{err}"
        );
        assert!(err.contains("`cm/x.wast:4`"), "{err}");
        assert!(err.contains(&stack_switch_reason()), "{err}");
    }

    #[wcmp_macros::test]
    async fn it_rejects_an_overlay_line_the_shared_list_already_names() {
        let err = Lists::parse(SHARED, WEB, &overlay(1))
            .err()
            .expect("rejected");
        assert!(
            err.contains("`cm/x.wast:1` is in expected-failures.txt too"),
            "{err}"
        );
    }

    #[wcmp_macros::test]
    async fn it_accepts_another_copy_wording_only_in_the_cm_corpus() {
        let expected = "cannot write to stream after being notified that the readable end dropped";
        let actual = "cannot write after being notified that the readable end dropped";
        assert!(trap_matches(true, expected, actual));
        assert!(!trap_matches(false, expected, actual));
        assert!(trap_matches(
            true,
            "cannot read from future after previous read succeeded",
            "cannot read after being notified that the writable end dropped"
        ));
    }

    #[wcmp_macros::test]
    async fn it_matches_the_intra_instance_refusal_without_the_cm_relaxation() {
        // The text `same-component-stream-future.wast` expects, which
        // the relaxation would also grant any "cannot read" trap.
        let expected = "cannot read from and write to intra-component future";
        let actual = Error::Copy(wasm_component_model_polyfill::CopyCause::IntraInstanceNonNumber)
            .to_string();
        assert!(
            trap_matches(false, expected, &actual),
            "the polyfill's refusal contains the text as written: {actual}"
        );
    }

    #[wcmp_macros::test]
    async fn it_keeps_the_copy_direction_in_the_cm_corpus() {
        assert!(!trap_matches(
            true,
            "cannot write to stream after being notified that the readable end dropped",
            "cannot read after being notified that the writable end dropped"
        ));
        assert!(!trap_matches(
            true,
            "unreachable",
            "cannot write after being notified that the readable end dropped"
        ));
    }

    #[wcmp_macros::test]
    async fn it_binds_a_definition_that_succeeds() {
        let outcomes =
            outcomes("(component definition $A (component))\n(component instance $A $A)\n").await;
        assert_eq!(outcomes, vec![None, None]);
    }

    #[wcmp_macros::test]
    async fn it_unbinds_a_name_whose_definition_fails_to_translate() {
        let text = format!(
            "(component definition $A (component))\n{INVALID}\n(component instance $A $A)\n"
        );
        let outcomes = outcomes(&text).await;
        assert_eq!(outcomes[0], None, "the first definition translates");
        assert!(
            outcomes[1]
                .as_deref()
                .is_some_and(|reason| reason.contains("component rejected")),
            "the second definition must fail to translate, got {:?}",
            outcomes[1]
        );
        assert_eq!(
            outcomes[2].as_deref(),
            Some("no definition named `A`"),
            "the name must be unbound, not still bound to the first component"
        );
    }

    #[wcmp_macros::test]
    async fn it_unbinds_a_name_whose_definition_fails_to_encode() {
        let text = "(component definition $A (component))
(component definition $A (component (export \"f\" (func $missing))))
(component instance $A $A)
";
        let outcomes = outcomes(text).await;
        assert_eq!(outcomes[0], None, "the first definition translates");
        assert!(
            outcomes[1]
                .as_deref()
                .is_some_and(|reason| reason.starts_with("encode:")),
            "the second definition must fail to encode, got {:?}",
            outcomes[1]
        );
        assert_eq!(outcomes[2].as_deref(), Some("no definition named `A`"));
    }

    #[wcmp_macros::test]
    async fn it_forgets_the_last_definition_when_a_definition_fails() {
        let text =
            format!("(component definition $A (component))\n{INVALID}\n(component instance)\n");
        let outcomes = outcomes(&text).await;
        assert_eq!(outcomes[0], None, "the first definition translates");
        assert!(outcomes[1].is_some(), "the second definition must fail");
        assert_eq!(
            outcomes[2].as_deref(),
            Some("no definition to instantiate"),
            "an unnamed instance must not run the definition before the failure"
        );
    }

    /// A named component that lifts one function and re-exports the
    /// core module behind it: instantiating it binds `$A` for a later
    /// `invoke` and reflects `m` into the linker under `A`.
    const NAMED: &str = "(component $A
  (core module $m (func (export \"f\") (result i32) i32.const 1))
  (core instance $i (instantiate $m))
  (func (export \"f\") (result u32) (canon lift (core func $i \"f\")))
  (export \"m\" (core module $m))
)";

    /// The same shape, named the same, with the stack-height error of
    /// `INVALID`: text that encodes and that no conforming
    /// implementation accepts.
    const NAMED_INVALID: &str = "(component $A
  (core module $m (func (export \"f\") (result i32) i32.const 1 i32.const 2))
  (core instance (instantiate $m))
)";

    /// A component that imports the module export `NAMED` reflects
    /// into the linker under `A`.
    const IMPORTS_A: &str = "(component
  (import \"A\" (instance (export \"m\" (core module (export \"f\" (func (result i32)))))))
)";

    #[wcmp_macros::test]
    async fn it_binds_and_registers_a_named_component_that_succeeds() {
        let text = format!("{NAMED}\n(invoke $A \"f\")\n{IMPORTS_A}\n");
        let outcomes = outcomes(&text).await;
        assert_eq!(outcomes, vec![None, None, None]);
    }

    #[wcmp_macros::test]
    async fn it_unbinds_a_name_whose_component_fails_to_translate() {
        let text = format!("{NAMED}\n{NAMED_INVALID}\n(invoke $A \"f\")\n");
        let outcomes = outcomes(&text).await;
        assert_eq!(outcomes[0], None, "the first component translates");
        assert!(
            outcomes[1]
                .as_deref()
                .is_some_and(|reason| reason.contains("component rejected")),
            "the second component must fail to translate, got {:?}",
            outcomes[1]
        );
        assert_eq!(
            outcomes[2].as_deref(),
            Some("no instance named `A`"),
            "the name must be unbound, not still bound to the first instance"
        );
    }

    #[wcmp_macros::test]
    async fn it_unregisters_a_name_whose_component_fails_to_translate() {
        let text = format!("{NAMED}\n{NAMED_INVALID}\n{IMPORTS_A}\n");
        let outcomes = outcomes(&text).await;
        assert_eq!(outcomes[0], None, "the first component translates");
        assert!(outcomes[1].is_some(), "the second component must fail");
        assert!(
            outcomes[2].is_some(),
            "the registration must be gone, not still reflecting the first instance"
        );
    }

    #[wcmp_macros::test]
    async fn it_unbinds_a_name_whose_component_fails_to_encode() {
        let text = format!(
            "{NAMED}\n(component $A (component (export \"f\" (func $missing))))\n(invoke $A \"f\")\n"
        );
        let outcomes = outcomes(&text).await;
        assert_eq!(outcomes[0], None, "the first component translates");
        assert!(
            outcomes[1]
                .as_deref()
                .is_some_and(|reason| reason.starts_with("encode:")),
            "the second component must fail to encode, got {:?}",
            outcomes[1]
        );
        assert_eq!(outcomes[2].as_deref(), Some("no instance named `A`"));
    }
}

/// Two tests per corpus file: `$name` runs it with the suspend provider
/// allowed, and `$name::it_passes_without_a_provider` runs it with the
/// provider turned off through `EngineConfig`. The nextest profiles in
/// `.config/nextest.toml` split them: the ordinary lanes run the first,
/// and the `no-provider` lanes run only the second. A function and a
/// module of one name live in different namespaces, so each file's
/// text is included once, in the module, and both tests read it.
macro_rules! corpus_test {
    ($name:ident, $path:literal) => {
        #[wcmp_macros::test]
        async fn $name() {
            check($path, $name::TEXT, true).await;
        }

        mod $name {
            pub const TEXT: &str = include_str!(concat!("../corpus/", $path));

            #[wcmp_macros::test]
            async fn it_passes_without_a_provider() {
                super::check($path, TEXT, false).await;
            }
        }
    };
}

include!("conformance/manifest.rs");
