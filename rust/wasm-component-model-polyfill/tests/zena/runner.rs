//! The polyfill subject of a scenario, on the target the test runs on.

use wasm_component_model_polyfill::{
    Component, ComponentParameters, ComponentResult, ComponentValue, Engine, Error, Func, Instance,
    Linker, PrimitiveType, Store, TypeMismatchPosition, Val, ValueType,
};
use wcmp_scenario::{Call, Outcome, Run, Stage, Typed, TypedSignature, Value, Verdict};

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
    /// one fails. A call is untyped, through `Func::call` with `Val`
    /// arguments, unless the expectations mark it as typed. A typed call
    /// goes through `TypedFunc`, for the closed set of signatures of
    /// [`TypedSignature`], and fails for any other. A result that is
    /// neither a scalar nor a string fails its call too, because the
    /// scenario model cannot hold it.
    pub async fn observe(&self, scenario: &Scenario) -> Result<Run, Verdict> {
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
    let Some((_, instance)) = instances.iter().find(|(name, _)| *name == call.component) else {
        return Outcome::Failure(format!("the scenario has no component {}", call.component));
    };
    let Some(func) = lookup(instance, &call.export) else {
        return Outcome::Failure(format!(
            "component {} has no function export {}",
            call.component, call.export
        ));
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
