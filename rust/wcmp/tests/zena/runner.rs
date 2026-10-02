// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The polyfill subject of a scenario, on the target the test runs on.

use std::sync::{Arc, Mutex, MutexGuard};

use wcmp::{
    Component, ComponentParameters, ComponentResult, ComponentValue, Engine, Error, ExternType,
    ExternalName, Func, FunctionType, Instance, InstanceItem, Linker, LinkerInstance,
    PrimitiveType, Store, TypeMismatchPosition, Val, ValueType,
};
use wcmp_scenario::{
    Call, Link, Outcome, Run, Stage, Subject, Typed, TypedSignature, Value, Verdict,
};

use crate::bundle::Scenario;
use crate::host::{self, Host};

/// A polyfill subject: one engine, shared by every scenario it runs,
/// over the backend of the subject.
///
/// The engine has the default configuration, so the suspend provider
/// is on wherever the target has one. Each scenario gets a linker with
/// the test host functions and its own run-time links, and a store of
/// its own, whose standard output the test host keeps.
pub struct PolyfillRun {
    engine: Engine,
    subject: Subject,
}

/// The instances of one scenario, each under its program's name. The
/// functions of a run-time link look the exporter up here when they
/// are called, and the runner looks up the component a call names.
type Instances = Arc<Mutex<Vec<(String, Instance)>>>;

/// The parsed components of one scenario, each under its program's
/// name, in the order of the names.
type Components<'a> = Vec<(&'a str, Component)>;

impl PolyfillRun {
    /// The polyfill subject of this run, over the backend of the run: the
    /// browser's in the browser, and natively the one `WCMP_TEST_BACKEND`
    /// names.
    pub fn new() -> Result<Self, Error> {
        Self::with_engine(
            Engine::with_backend(crate::test_backend::backend())?,
            subject_of_run(),
        )
    }

    /// The native polyfill subject over the backend `kind`, whatever the
    /// run chose: [`Subject::Native`] over Wasmtime, and
    /// [`Subject::Wasmi`] over Wasmi.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn over(kind: crate::test_backend::Kind) -> Result<Self, Error> {
        Self::with_engine(Engine::with_backend(kind.backend())?, native_subject(kind))
    }

    /// The subject `subject`, over `engine`, which has the default
    /// configuration. A linker with the test host functions is built
    /// once here, so a definition the polyfill refuses fails before any
    /// scenario runs.
    fn with_engine(engine: Engine, subject: Subject) -> Result<Self, Error> {
        let run = PolyfillRun { engine, subject };
        run.linker()?;
        Ok(run)
    }

    /// The subject this run reports as.
    pub fn subject(&self) -> Subject {
        self.subject
    }

    /// A linker with the test host functions.
    fn linker(&self) -> Result<Linker<Host>, Error> {
        let mut linker = Linker::new(&self.engine);
        host::define(&mut linker)?;
        Ok(linker)
    }

    /// Run `scenario` and judge it with the scenario model: against the
    /// Wasmtime run's observations when that run passed, and against the
    /// expectations otherwise.
    ///
    /// # Errors
    ///
    /// The scenario model's error when it refuses to judge the run,
    /// such as observations made from another version of the scenario,
    /// or when the wiring names a component the scenario does not have,
    /// leaves no order to instantiate in, or names an import or export
    /// that a component it joins does not have. That is a fault of the
    /// build, of the scenario's files, or of this runner, not a stage.
    pub async fn run(&self, scenario: &Scenario) -> Result<Verdict, wcmp_scenario::Error> {
        match self.observe(scenario).await? {
            Ok(run) => scenario.observations.judge(&scenario.expectations, &run),
            Err(verdict) => Ok(verdict),
        }
    }

    /// Run `scenario` up to its calls, or answer the verdict of the step
    /// that stopped it: parse every component, check each run-time link
    /// against the components it joins, and then link, instantiate, and
    /// call them.
    ///
    /// # Errors
    ///
    /// The scenario model's error when the wiring names a component the
    /// scenario does not have, leaves no order to instantiate in, or
    /// names an import or export that a component it joins does not
    /// have. That is a fault of the build or of the scenario's files,
    /// not a stage.
    pub async fn observe(
        &self,
        scenario: &Scenario,
    ) -> Result<Result<Run, Verdict>, wcmp_scenario::Error> {
        let names: Vec<&str> = scenario
            .programs
            .iter()
            .map(|program| program.name.as_str())
            .collect();
        let order = scenario.wiring.order(&names)?;
        let components = match self.parse(scenario).await {
            Ok(components) => components,
            Err(verdict) => return Ok(Err(verdict)),
        };
        for link in scenario.wiring.run_time() {
            link.check(
                component(&components, &link.importer)
                    .imports
                    .iter()
                    .map(|import| import.name.to_string()),
                component(&components, &link.exporter)
                    .exports
                    .iter()
                    .map(|export| export.name.to_string()),
            )?;
        }
        Ok(self.link_and_call(scenario, &order, &components).await)
    }

    /// Parse every component of `scenario` in the order of its
    /// program's name, or answer the verdict of the step that stopped
    /// it. A program that did not compile stops the scenario at
    /// `compile`, and then a composition the build could not make stops
    /// it at `compose`, and nothing is parsed. A composition the build
    /// made is one component under its importer's name.
    async fn parse<'a>(&self, scenario: &'a Scenario) -> Result<Components<'a>, Verdict> {
        let mut compiled = Vec::with_capacity(scenario.programs.len());
        for program in &scenario.programs {
            compiled.push((program.name.as_str(), program.component()?));
        }
        for program in &scenario.programs {
            program.composed()?;
        }
        let mut components = Vec::with_capacity(compiled.len());
        for (name, bytes) in compiled {
            match Component::new(&self.engine, bytes).await {
                Ok(component) => components.push((name, component)),
                Err(error) => return Err(stopped(Stage::Parse, name, &error)),
            }
        }
        Ok(components)
    }

    /// Run the parsed `components` of `scenario` up to its calls, or
    /// answer the verdict of the step that stopped it.
    ///
    /// The components are linked and instantiated in `order`, each
    /// exporter of a run-time link before its importer, all in one
    /// store. The polyfill links and instantiates in one step, so an
    /// error of that step is `link` when the linker could not supply an
    /// import and `instantiate` otherwise. The Wasmtime run links every
    /// component before it instantiates any, so when one component
    /// fails to link and an earlier one in `order` fails to
    /// instantiate, the two runs stop at different stages.
    ///
    /// A run-time link is made through the linker before any component
    /// is linked. For each function of the item the exporter exports
    /// under the link's import name, the linker gets a concurrent host
    /// function of the same name and the exporter's type, whose call
    /// looks up the exporter's instance and calls its function through
    /// `Func::call_concurrent`. That is the only way the polyfill's
    /// public API calls another instance from a host function, and the
    /// linker takes a concurrent host function only for an `async`
    /// import. So a run-time link forwards `async` functions only, and
    /// an item that holds a synchronous function stops the scenario at
    /// `link` with a reason that says so. The Wasmtime run makes the
    /// same links the same way.
    ///
    /// Each call of the expectations is then made in order, even after
    /// one fails. A call is untyped, through `Func::call` with `Val`
    /// arguments, unless the expectations mark it as typed. A typed call
    /// goes through `TypedFunc`, for the closed set of signatures of
    /// [`TypedSignature`], and fails for any other. A result that is
    /// neither a scalar nor a string fails its call too, because the
    /// scenario model cannot hold it.
    async fn link_and_call(
        &self,
        scenario: &Scenario,
        order: &[&str],
        components: &Components<'_>,
    ) -> Result<Run, Verdict> {
        let instances = Instances::default();
        let mut linker = self
            .linker()
            .unwrap_or_else(|error| panic!("a linker for scenario {}: {error}", scenario.name));
        for link in scenario.wiring.run_time() {
            if let Err(reason) = forward(
                &mut linker,
                link,
                component(components, &link.exporter),
                &instances,
            ) {
                return Err(Verdict::new(
                    Stage::Link,
                    format!("component {}: {reason}", link.importer),
                ));
            }
        }

        let mut store = Store::new(&self.engine, Host::default())
            .unwrap_or_else(|error| panic!("a store for scenario {}: {error}", scenario.name));
        for &name in order {
            match linker
                .instantiate(&mut store, component(components, name))
                .await
            {
                Ok(instance) => lock(&instances).push((name.to_string(), instance)),
                Err(error) => return Err(stopped(instantiation_stage(&error), name, &error)),
            }
        }

        let mut outcomes = Vec::with_capacity(scenario.expectations.entries.len());
        for entry in &scenario.expectations.entries {
            outcomes.push(call(&mut store, &instances, &entry.call).await);
        }
        Ok(Run {
            outcomes,
            output: store.data().lines(),
        })
    }
}

/// The parsed component of `name`, one of the scenario's own: the
/// wiring's order and a link's check name only those.
fn component<'a>(components: &'a Components<'_>, name: &str) -> &'a Component {
    components
        .iter()
        .find(|(component, _)| *component == name)
        .map(|(_, component)| component)
        .expect("the wiring names the scenario's own components")
}

/// Make the run-time `link` through `linker`: define each function of
/// the item `exporter` exports under the link's import name, as a
/// function that calls the exporter's instance in `instances`. The
/// link is checked, so the exporter has the item. The answer is the
/// reason the link cannot be made.
fn forward(
    linker: &mut Linker<Host>,
    link: &Link,
    exporter: &Component,
    instances: &Instances,
) -> Result<(), String> {
    let export = exporter
        .exports
        .iter()
        .find(|export| export.name.to_string() == link.import)
        .expect("a checked link names an export of its exporter");
    match (&export.name, &export.ty) {
        (ExternalName::Interface(interface), ExternType::Instance(instance)) => forward_items(
            &mut linker.instance(interface),
            link,
            &instance.items,
            instances,
        ),
        (_, ExternType::Instance(instance)) => forward_items(
            &mut linker.root().instance(link.import.clone()),
            link,
            &instance.items,
            instances,
        ),
        (_, ExternType::Function(ty)) => forward_func(
            &mut linker.root(),
            &link.import,
            &link.exporter,
            link.import.clone(),
            ty,
            instances,
        ),
        _ => Err(format!(
            "{} is not a function or an instance, and a run-time link forwards functions only",
            link.import
        )),
    }
}

/// Define each of `items`, the functions of the instance `link`
/// imports, in `view`.
fn forward_items(
    view: &mut LinkerInstance<'_, Host>,
    link: &Link,
    items: &[InstanceItem],
    instances: &Instances,
) -> Result<(), String> {
    for item in items {
        let ExternType::Function(ty) = &item.ty else {
            return Err(format!(
                "{}#{} is not a function, and a run-time link forwards functions only",
                link.import, item.name
            ));
        };
        let export = format!("{}#{}", link.import, item.name);
        forward_func(view, &item.name, &link.exporter, export, ty, instances)?;
    }
    Ok(())
}

/// Define `name` in `view` as a function of type `ty` that calls the
/// function `export` of the instance of `exporter`.
fn forward_func(
    view: &mut LinkerInstance<'_, Host>,
    name: &str,
    exporter: &str,
    export: String,
    ty: &FunctionType,
    instances: &Instances,
) -> Result<(), String> {
    if !ty.async_ {
        return Err(format!(
            "{export} is a synchronous function of component {exporter}, and a run-time link forwards only an `async` one"
        ));
    }
    let exporter = exporter.to_string();
    let instances = instances.clone();
    view.func_new_concurrent(name, ty.clone(), move |accessor, args| {
        let accessor = accessor.clone();
        let func = exported(&instances, &exporter, &export);
        async move {
            let func = func.map_err(|message| Error::Internal { message })?;
            Ok(func.call_concurrent(&accessor, &args).await?.into_vec())
        }
    })
    .map_err(|error| describe(&error))
}

/// The function `export` of the instance of `component`, or why there
/// is none.
fn exported(instances: &Instances, component: &str, export: &str) -> Result<Func, String> {
    let instances = lock(instances);
    let (_, instance) = instances
        .iter()
        .find(|(name, _)| name == component)
        .ok_or_else(|| format!("the scenario has no component {component}"))?;
    lookup(instance, export)
        .ok_or_else(|| format!("component {component} has no function export {export}"))
}

/// Lock `instances`, whatever a panic elsewhere left in it.
fn lock(instances: &Instances) -> MutexGuard<'_, Vec<(String, Instance)>> {
    instances
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The verdict of a component `name` that stopped at `stage`.
fn stopped(stage: Stage, name: &str, error: &Error) -> Verdict {
    Verdict::new(stage, format!("component {name}: {}", describe(error)))
}

/// The stage of an error of `Linker::instantiate`: `link` when the
/// linker could not supply an import, or supplied one whose type does
/// not match, and `instantiate` otherwise.
fn instantiation_stage(error: &Error) -> Stage {
    match error {
        Error::Link(_) => Stage::Link,
        Error::TypeMismatch(mismatch)
            if matches!(
                mismatch.position,
                TypeMismatchPosition::HostFunctionRegistration { .. }
                    | TypeMismatchPosition::HostFunctionRegistrationPlain { .. }
            ) =>
        {
            Stage::Link
        }
        _ => Stage::Instantiate,
    }
}

/// An error with each of its causes, as one line of text.
fn describe(error: &(dyn std::error::Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        if !text.contains(&cause_text) {
            text.push_str(": ");
            text.push_str(&cause_text);
        }
        source = cause.source();
    }
    text
}

/// Make one call and report how it ended.
async fn call(store: &mut Store<Host>, instances: &Instances, call: &Call) -> Outcome {
    let func = match exported(instances, &call.component, &call.export) {
        Ok(func) => func,
        Err(reason) => return Outcome::Failure(reason),
    };
    if call.typed {
        return typed_call(store, func, &call.arguments).await;
    }
    let arguments: Vec<Val> = call.arguments.iter().map(to_val).collect();
    let results = match func.call(store, &arguments).await {
        Ok(results) => results,
        Err(error) => return Outcome::Failure(describe(&error)),
    };
    let mut values = Vec::with_capacity(results.len());
    for (index, result) in results.iter().enumerate() {
        match from_val(result) {
            Some(value) => values.push(value),
            None => {
                return Outcome::Failure(format!(
                    "result {} is {result:?}, which is neither a scalar nor a string",
                    index + 1
                ));
            }
        }
    }
    Outcome::Results(values)
}

/// Make one typed call of `func` through the polyfill's `TypedFunc`,
/// and report how it ended.
///
/// The Rust types of the typed function come from the closed set of
/// [`TypedSignature`]: the parameter types from `arguments`, and the
/// result type from the export. A call outside that set, or one whose
/// export has a result the scenario model cannot hold, fails. So does a
/// call whose arguments do not have the export's parameter types, which
/// `Func::typed` refuses.
async fn typed_call(store: &mut Store<Host>, func: Func, arguments: &[Value]) -> Outcome {
    use wcmp_scenario::ValueType as Type;

    let results = match &func.ty().result {
        None => Vec::new(),
        Some(ty) => match value_type(ty) {
            Some(ty) => vec![ty],
            None => {
                return Outcome::Failure(format!(
                    "result 1 is {ty:?}, which is neither a scalar nor a string"
                ));
            }
        },
    };
    let parameters: Vec<Type> = arguments.iter().map(Value::ty).collect();
    let signature = match TypedSignature::new(&parameters, &results) {
        Ok(signature) => signature,
        Err(error) => return Outcome::Failure(error.to_string()),
    };
    let arguments = arguments.to_vec();
    match signature.ty {
        None => typed::<(), ()>(store, func, (), |()| Vec::new()).await,
        Some(Type::Bool) => typed_of::<bool>(store, func, signature, arguments).await,
        Some(Type::S8) => typed_of::<i8>(store, func, signature, arguments).await,
        Some(Type::U8) => typed_of::<u8>(store, func, signature, arguments).await,
        Some(Type::S16) => typed_of::<i16>(store, func, signature, arguments).await,
        Some(Type::U16) => typed_of::<u16>(store, func, signature, arguments).await,
        Some(Type::S32) => typed_of::<i32>(store, func, signature, arguments).await,
        Some(Type::U32) => typed_of::<u32>(store, func, signature, arguments).await,
        Some(Type::S64) => typed_of::<i64>(store, func, signature, arguments).await,
        Some(Type::U64) => typed_of::<u64>(store, func, signature, arguments).await,
        Some(Type::F32) => typed_of::<f32>(store, func, signature, arguments).await,
        Some(Type::F64) => typed_of::<f64>(store, func, signature, arguments).await,
        Some(Type::Char) => typed_of::<char>(store, func, signature, arguments).await,
        Some(Type::String) => typed_of::<String>(store, func, signature, arguments).await,
    }
}

/// Make a typed call of `signature` whose parameters and result are
/// all of the Rust type `T`.
async fn typed_of<T>(
    store: &mut Store<Host>,
    func: Func,
    signature: TypedSignature,
    arguments: Vec<Value>,
) -> Outcome
where
    T: Typed + ComponentValue,
{
    let Some(arguments) = arguments
        .into_iter()
        .map(T::from_value)
        .collect::<Option<Vec<T>>>()
    else {
        return Outcome::Failure(format!("an argument is not a {}", T::TYPE));
    };
    let result = |result: T| vec![result.into_value()];
    let none = |()| Vec::new();
    // `TypedSignature` holds a typed call to two arguments at most.
    let mut arguments = arguments.into_iter();
    match (arguments.next(), arguments.next(), signature.result) {
        (None, _, false) => typed::<(), ()>(store, func, (), none).await,
        (None, _, true) => typed::<(), T>(store, func, (), result).await,
        (Some(a), None, false) => typed::<(T,), ()>(store, func, (a,), none).await,
        (Some(a), None, true) => typed::<(T,), T>(store, func, (a,), result).await,
        (Some(a), Some(b), false) => typed::<(T, T), ()>(store, func, (a, b), none).await,
        (Some(a), Some(b), true) => typed::<(T, T), T>(store, func, (a, b), result).await,
    }
}

/// Make a typed call of `func` with the Rust types `P` and `R`, and
/// turn its result into the scenario model's values with `values`.
async fn typed<P, R>(
    store: &mut Store<Host>,
    func: Func,
    arguments: P,
    values: impl FnOnce(R) -> Vec<Value>,
) -> Outcome
where
    P: ComponentParameters,
    R: ComponentResult,
{
    let typed = match func.typed::<P, R>() {
        Ok(typed) => typed,
        Err(error) => return Outcome::Failure(describe(&error)),
    };
    match typed.call(store, arguments).await {
        Ok(result) => Outcome::Results(values(result)),
        Err(error) => Outcome::Failure(describe(&error)),
    }
}

/// The scenario model's type of a polyfill type, or `None` when the
/// model has no values of it: it holds scalars and strings only.
fn value_type(ty: &ValueType) -> Option<wcmp_scenario::ValueType> {
    use wcmp_scenario::ValueType as Type;
    let ValueType::Primitive(primitive) = ty else {
        return None;
    };
    Some(match primitive {
        PrimitiveType::Bool => Type::Bool,
        PrimitiveType::S8 => Type::S8,
        PrimitiveType::U8 => Type::U8,
        PrimitiveType::S16 => Type::S16,
        PrimitiveType::U16 => Type::U16,
        PrimitiveType::S32 => Type::S32,
        PrimitiveType::U32 => Type::U32,
        PrimitiveType::S64 => Type::S64,
        PrimitiveType::U64 => Type::U64,
        PrimitiveType::F32 => Type::F32,
        PrimitiveType::F64 => Type::F64,
        PrimitiveType::Char => Type::Char,
        PrimitiveType::String => Type::String,
    })
}

/// The function an export name names: `add` at the root of the
/// component, or `local:demo/api#greet` inside an exported interface.
fn lookup(instance: &Instance, export: &str) -> Option<Func> {
    match export.split_once('#') {
        Some((interface, name)) => instance.exports().instance(interface)?.func(name),
        None => instance.get_func(export),
    }
}

/// The polyfill's value of an argument.
fn to_val(value: &Value) -> Val {
    match value {
        Value::Bool(value) => Val::Bool(*value),
        Value::S8(value) => Val::S8(*value),
        Value::U8(value) => Val::U8(*value),
        Value::S16(value) => Val::S16(*value),
        Value::U16(value) => Val::U16(*value),
        Value::S32(value) => Val::S32(*value),
        Value::U32(value) => Val::U32(*value),
        Value::S64(value) => Val::S64(*value),
        Value::U64(value) => Val::U64(*value),
        Value::F32(value) => Val::F32(*value),
        Value::F64(value) => Val::F64(*value),
        Value::Char(value) => Val::Char(*value),
        Value::String(value) => Val::String(value.clone()),
    }
}

/// The scenario model's value of a result, or `None` when the model has
/// no value of its type: it holds scalars and strings only.
fn from_val(val: &Val) -> Option<Value> {
    Some(match val {
        Val::Bool(value) => Value::Bool(*value),
        Val::S8(value) => Value::S8(*value),
        Val::U8(value) => Value::U8(*value),
        Val::S16(value) => Value::S16(*value),
        Val::U16(value) => Value::U16(*value),
        Val::S32(value) => Value::S32(*value),
        Val::U32(value) => Value::U32(*value),
        Val::S64(value) => Value::S64(*value),
        Val::U64(value) => Value::U64(*value),
        Val::F32(value) => Value::F32(*value),
        Val::F64(value) => Value::F64(*value),
        Val::Char(value) => Value::Char(*value),
        Val::String(value) => Value::String(value.clone()),
        _ => return None,
    })
}

/// The polyfill subject of this run: the browser.
#[cfg(target_arch = "wasm32")]
pub fn subject_of_run() -> Subject {
    Subject::Web
}

/// The polyfill subject of this run: the subject of the backend
/// `WCMP_TEST_BACKEND` names.
#[cfg(not(target_arch = "wasm32"))]
pub fn subject_of_run() -> Subject {
    native_subject(crate::test_backend::Kind::of_run())
}

/// The native polyfill subject over the backend `kind`.
#[cfg(not(target_arch = "wasm32"))]
fn native_subject(kind: crate::test_backend::Kind) -> Subject {
    match kind {
        crate::test_backend::Kind::Wasmtime => Subject::Native,
        crate::test_backend::Kind::Wasmi => Subject::Wasmi,
    }
}
