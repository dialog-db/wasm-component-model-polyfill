//! The polyfill subject of a scenario, on the target the test runs on.

use wasm_component_model_polyfill::{
    Component, Engine, Error, Func, Instance, Linker, Store, TypeMismatchPosition, Val,
};
use wcmp_scenario::{Call, Outcome, Run, Stage, Value, Verdict};

use crate::bundle::Scenario;
use crate::host::{self, Host};

/// The polyfill subject: one engine and one linker with the test host
/// functions, shared by every scenario it runs.
///
/// The engine has the default configuration, so the suspend provider
/// is on wherever the target has one. Each scenario gets a store of its
/// own, whose standard output the test host keeps.
pub struct PolyfillRun {
    engine: Engine,
    linker: Linker<Host>,
}

impl PolyfillRun {
    /// An engine with the default configuration and a linker with the
    /// test host functions.
    pub fn new() -> Result<Self, Error> {
        let engine = Engine::new()?;
        let mut linker = Linker::new(&engine);
        host::define(&mut linker)?;
        Ok(PolyfillRun { engine, linker })
    }

    /// Run `scenario` and judge it with the scenario model: against the
    /// Wasmtime run's observations when that run passed, and against the
    /// expectations otherwise.
    ///
    /// # Errors
    ///
    /// The scenario model's error when it refuses to judge the run,
    /// such as observations made from another version of the scenario.
    /// That is a fault of the build or of this runner, not a stage.
    pub async fn run(&self, scenario: &Scenario) -> Result<Verdict, wcmp_scenario::Error> {
        match self.observe(scenario).await {
            Ok(run) => scenario.observations.judge(&scenario.expectations, &run),
            Err(verdict) => Ok(verdict),
        }
    }

    /// Run `scenario` up to its calls, or answer the verdict of the step
    /// that stopped it.
    ///
    /// A program that did not compile stops the scenario at `compile`,
    /// and nothing runs. Otherwise every component is parsed, and then
    /// linked and instantiated in the order of its program's name, all
    /// in one store. The polyfill links and instantiates in one step, so
    /// an error of that step is `link` when the linker could not supply
    /// an import and `instantiate` otherwise.
    ///
    /// Each call of the expectations is then made in order, even after
    /// one fails, untyped: `Func::call` with `Val` arguments. A call the
    /// expectations mark as typed fails, since typed calls are not
    /// supported here yet. A result that is neither a scalar nor a
    /// string fails its call too, because the scenario model cannot
    /// hold it.
    async fn observe(&self, scenario: &Scenario) -> Result<Run, Verdict> {
        let mut compiled = Vec::with_capacity(scenario.programs.len());
        for program in &scenario.programs {
            compiled.push((program.name.as_str(), program.component()?));
        }

        let mut components = Vec::with_capacity(compiled.len());
        for (name, bytes) in compiled {
            match Component::new(&self.engine, bytes).await {
                Ok(component) => components.push((name, component)),
                Err(error) => return Err(stopped(Stage::Parse, name, &error)),
            }
        }

        let mut store = Store::new(&self.engine, Host::default())
            .unwrap_or_else(|error| panic!("a store for scenario {}: {error}", scenario.name));
        let mut instances = Vec::with_capacity(components.len());
        for (name, component) in &components {
            match self.linker.instantiate(&mut store, component).await {
                Ok(instance) => instances.push((*name, instance)),
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
async fn call(store: &mut Store<Host>, instances: &[(&str, Instance)], call: &Call) -> Outcome {
    if call.typed {
        return Outcome::Failure("the polyfill run does not make typed calls yet".to_string());
    }
    let Some((_, instance)) = instances.iter().find(|(name, _)| *name == call.component) else {
        return Outcome::Failure(format!("the scenario has no component {}", call.component));
    };
    let Some(func) = lookup(instance, &call.export) else {
        return Outcome::Failure(format!(
            "component {} has no function export {}",
            call.component, call.export
        ));
    };
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
