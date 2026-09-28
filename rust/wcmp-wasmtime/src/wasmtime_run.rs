//! The Wasmtime run of one scenario.

use wasmtime::component::types::{ComponentFunc, ComponentItem};
use wasmtime::component::{
    Component, ComponentNamedList, ComponentType, Func, Instance, Lift, Linker, LinkerInstance,
    Lower, ResourceTable, Val,
};
use wasmtime::{AsContextMut, Config, Engine, Store};
use wasmtime_wasi::p2::pipe::MemoryOutputPipe;
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wcmp_scenario::{
    Call, Link, Observations, Outcome, Run, Stage, Typed, TypedSignature, Value, ValueType,
};

use crate::error::{Error, Result};
use crate::scenario::Scenario;
use crate::value::{from_val, to_val, value_type};

/// The most a scenario can print. A scenario is small and deterministic,
/// so a megabyte is far more than any prints; a write past it fails in
/// the guest.
const STDOUT_CAPACITY: usize = 1 << 20;

/// The Wasmtime subject: one engine and one linker, shared by every
/// scenario it runs.
///
/// The linker supplies the WASI Preview 2 and Preview 3 imports of
/// `wasmtime-wasi` and the fixed test interface. Each scenario gets a
/// store of its own, whose standard output goes to a buffer in memory,
/// so the lines a scenario prints are compared with its expectations.
/// A scenario whose components link at run time gets a copy of the
/// linker with its links added.
pub struct WasmtimeRun {
    engine: Engine,
    linker: Linker<Host>,
}

impl WasmtimeRun {
    /// The fixed test interface a scenario can import. Its one function,
    /// [`WasmtimeRun::TEST_FUNCTION`], takes a string and returns it.
    pub const TEST_INTERFACE: &str = wcmp_scenario::TEST_INTERFACE;

    /// The function of [`WasmtimeRun::TEST_INTERFACE`]:
    /// `echo: func(text: string) -> string`.
    pub const TEST_FUNCTION: &str = wcmp_scenario::TEST_FUNCTION;

    /// An engine with Wasmtime's default configuration, and a linker
    /// with the WASI imports and the test interface.
    ///
    /// # Errors
    ///
    /// [`Error::Setup`] when Wasmtime refuses the configuration or a
    /// definition.
    pub fn new() -> Result<Self> {
        let setup = |error: wasmtime::Error| Error::Setup(format!("{error:#}"));
        let engine = Engine::new(&Config::new()).map_err(setup)?;
        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_async(&mut linker).map_err(setup)?;
        wasmtime_wasi::p3::add_to_linker(&mut linker).map_err(setup)?;
        linker
            .instance(Self::TEST_INTERFACE)
            .and_then(|mut interface| {
                interface.func_wrap(Self::TEST_FUNCTION, |_, (text,): (String,)| Ok((text,)))
            })
            .map_err(setup)?;
        Ok(WasmtimeRun { engine, linker })
    }

    /// Run `scenario` and judge it against its expectations.
    ///
    /// A program that did not compile stops the scenario at `compile`,
    /// and nothing runs. Otherwise every component is parsed, then
    /// linked, then instantiated, all in one store, and the first step
    /// that fails for any of them is the stage. Components are parsed
    /// in the order of their programs' names, and linked and
    /// instantiated in the order the wiring gives: each exporter of a
    /// run-time link before its importer, and otherwise by name.
    ///
    /// Once every component is parsed, each run-time link is checked:
    /// its importer must import, and its exporter export, an item under
    /// the link's import name. A link that fails the check is a mistake
    /// in the wiring file, so it fails the run instead of being recorded
    /// as a stage.
    ///
    /// A run-time link is made through the linker before any component
    /// is linked. For each function of the item the exporter exports
    /// under the link's import name, the linker gets a function of the
    /// same name that calls the exporter's function once the exporter
    /// is instantiated. Such a function is a concurrent host function
    /// that calls the exporter through `Func::call_concurrent`, because
    /// nothing else in Wasmtime's API calls another instance from inside
    /// a host function: a call that runs its own event loop there
    /// panics, and Wasmtime refuses a concurrent host function for an
    /// import that is not `async`. So a run-time link forwards `async`
    /// functions only, and an item that holds a synchronous function
    /// stops the scenario at `link` with a reason that says so.
    ///
    /// Once every component is instantiated, each call of the
    /// expectations is made in order, even after one fails, and the
    /// lines printed to standard output are kept.
    ///
    /// A call is untyped, through `Func::call_async` with `Val`
    /// arguments, unless the expectations mark it as typed. A typed call
    /// goes through Wasmtime's `TypedFunc`, for the closed set of
    /// signatures of [`TypedSignature`], and fails for any other. A
    /// result that is neither a scalar nor a string fails its call too,
    /// because the scenario model cannot hold it.
    ///
    /// # Errors
    ///
    /// [`Error::Judge`] when the scenario model refuses the run, or the
    /// scenario's wiring: its links leave no order to instantiate in,
    /// or one names an import or export that a component it joins does
    /// not have. That is a fault of this runner or of the scenario's
    /// files, not a stage of the scenario.
    pub async fn run(&self, scenario: &Scenario) -> Result<Observations> {
        let judge = |source| Error::Judge {
            scenario: scenario.name.clone(),
            source,
        };
        let stopped = |stage, reason: String| Observations::stopped(stage, reason).map_err(judge);

        let mut compiled = Vec::with_capacity(scenario.programs.len());
        for program in &scenario.programs {
            match program.compiled() {
                Ok(bytes) => compiled.push((program.name.as_str(), bytes)),
                Err(verdict) => return stopped(verdict.stage, verdict.reason),
            }
        }

        let mut components = Vec::with_capacity(compiled.len());
        for (name, bytes) in compiled {
            match Component::new(&self.engine, bytes) {
                Ok(component) => components.push((name, component)),
                Err(error) => return stopped(Stage::Parse, format!("component {name}: {error:#}")),
            }
        }
        let names: Vec<&str> = components.iter().map(|(name, _)| *name).collect();
        let order = scenario.wiring.order(&names).map_err(judge)?;
        let component = |name: &str| {
            components
                .iter()
                .find(|(component, _)| *component == name)
                .map(|(_, component)| component)
                .expect("the wiring orders the scenario's own components")
        };

        for link in scenario.wiring.run_time() {
            let imports = component(&link.importer).component_type();
            let exports = component(&link.exporter).component_type();
            link.check(
                imports.imports(&self.engine).map(|(name, _)| name),
                exports.exports(&self.engine).map(|(name, _)| name),
            )
            .map_err(judge)?;
        }

        let mut linker = self.linker.clone();
        for link in scenario.wiring.run_time() {
            if let Err(reason) = forward(&mut linker, &self.engine, link, component(&link.exporter))
            {
                return stopped(
                    Stage::Link,
                    format!("component {}: {reason}", link.importer),
                );
            }
        }
        let mut prepared = Vec::with_capacity(order.len());
        for name in order {
            match linker.instantiate_pre(component(name)) {
                Ok(pre) => prepared.push((name, pre)),
                Err(error) => return stopped(Stage::Link, format!("component {name}: {error:#}")),
            }
        }

        let stdout = MemoryOutputPipe::new(STDOUT_CAPACITY);
        let mut store = Store::new(&self.engine, Host::new(stdout.clone()));
        for (name, pre) in prepared {
            match pre.instantiate_async(&mut store).await {
                Ok(instance) => store
                    .data_mut()
                    .instances
                    .push((name.to_string(), instance)),
                Err(error) => {
                    return stopped(Stage::Instantiate, format!("component {name}: {error:#}"));
                }
            }
        }

        let mut outcomes = Vec::with_capacity(scenario.expectations.entries.len());
        for entry in &scenario.expectations.entries {
            outcomes.push(call(&mut store, &entry.call).await);
        }
        let output = String::from_utf8_lossy(&stdout.contents())
            .lines()
            .map(str::to_string)
            .collect();
        Observations::observe(&scenario.expectations, Run { outcomes, output }).map_err(judge)
    }
}

/// Make the run-time `link` through `linker`: define each function of
/// the item `exporter` exports under the link's import name, as a
/// function that calls the exporter's instance. The link is checked,
/// so the exporter has the item. The answer is the reason the link
/// cannot be made.
fn forward(
    linker: &mut Linker<Host>,
    engine: &Engine,
    link: &Link,
    exporter: &Component,
) -> core::result::Result<(), String> {
    let exporter_type = exporter.component_type();
    let item = exporter_type
        .get_export(engine, &link.import)
        .expect("a checked link names an export of its exporter");
    match item.ty {
        ComponentItem::ComponentInstance(instance) => {
            let mut interface = linker
                .instance(&link.import)
                .map_err(|error| format!("{error:#}"))?;
            for (name, item) in instance.exports(engine) {
                let ComponentItem::ComponentFunc(func) = item.ty else {
                    return Err(format!(
                        "{}#{name} is not a function, and a run-time link forwards functions only",
                        link.import
                    ));
                };
                let export = format!("{}#{name}", link.import);
                forward_func(&mut interface, name, &link.exporter, export, &func)?;
            }
            Ok(())
        }
        ComponentItem::ComponentFunc(func) => forward_func(
            &mut linker.root(),
            &link.import,
            &link.exporter,
            link.import.clone(),
            &func,
        ),
        _ => Err(format!(
            "{} is not a function or an instance, and a run-time link forwards functions only",
            link.import
        )),
    }
}

/// Define `name` in `interface` as a function that calls the function
/// `export` of the instance of `exporter`.
fn forward_func(
    interface: &mut LinkerInstance<'_, Host>,
    name: &str,
    exporter: &str,
    export: String,
    func: &ComponentFunc,
) -> core::result::Result<(), String> {
    if !func.async_() {
        return Err(format!(
            "{export} is a synchronous function of component {exporter}, and a run-time link forwards only an `async` one"
        ));
    }
    let exporter = exporter.to_string();
    interface
        .func_new_concurrent(name, move |accessor, _, params, results| {
            let exporter = exporter.clone();
            let export = export.clone();
            Box::pin(async move {
                let func = accessor.with(|mut access| {
                    let instance = access.data_mut().instance(&exporter).ok_or_else(|| {
                        wasmtime::format_err!("component {exporter} is not instantiated yet")
                    })?;
                    lookup(&mut access, &instance, &export).ok_or_else(|| {
                        wasmtime::format_err!(
                            "component {exporter} has no function export {export}"
                        )
                    })
                })?;
                func.call_concurrent(accessor, params, results).await
            })
        })
        .map_err(|error| format!("{error:#}"))
}

/// Make one call and report how it ended.
async fn call(store: &mut Store<Host>, call: &Call) -> Outcome {
    let Some(instance) = store.data().instance(&call.component) else {
        return Outcome::Failure(format!("the scenario has no component {}", call.component));
    };
    let Some(func) = lookup(&mut *store, &instance, &call.export) else {
        return Outcome::Failure(format!(
            "component {} has no function export {}",
            call.component, call.export
        ));
    };
    if call.typed {
        return typed_call(store, &func, &call.arguments).await;
    }
    let arguments: Vec<Val> = call.arguments.iter().map(to_val).collect();
    let mut results = vec![Val::Bool(false); func.ty(&*store).results().len()];
    if let Err(error) = func.call_async(&mut *store, &arguments, &mut results).await {
        return Outcome::Failure(format!("{error:#}"));
    }
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

/// Make one typed call of `func` through Wasmtime's `TypedFunc`, and
/// report how it ended.
///
/// The Rust types of the typed function come from the closed set of
/// [`TypedSignature`]: the parameter types from `arguments`, and the
/// result type from the export. A call outside that set, or one whose
/// export has a result the scenario model cannot hold, fails. So does a
/// call whose arguments do not have the export's parameter types, which
/// `Func::typed` refuses.
async fn typed_call(store: &mut Store<Host>, func: &Func, arguments: &[Value]) -> Outcome {
    let mut results = Vec::new();
    for (index, ty) in func.ty(&*store).results().enumerate() {
        match value_type(&ty) {
            Some(ty) => results.push(ty),
            None => {
                return Outcome::Failure(format!(
                    "result {} is {ty:?}, which is neither a scalar nor a string",
                    index + 1
                ));
            }
        }
    }
    let parameters: Vec<ValueType> = arguments.iter().map(Value::ty).collect();
    let signature = match TypedSignature::new(&parameters, &results) {
        Ok(signature) => signature,
        Err(error) => return Outcome::Failure(error.to_string()),
    };
    let arguments = arguments.to_vec();
    match signature.ty {
        None => typed::<(), ()>(store, func, (), |()| Vec::new()).await,
        Some(ValueType::Bool) => typed_of::<bool>(store, func, signature, arguments).await,
        Some(ValueType::S8) => typed_of::<i8>(store, func, signature, arguments).await,
        Some(ValueType::U8) => typed_of::<u8>(store, func, signature, arguments).await,
        Some(ValueType::S16) => typed_of::<i16>(store, func, signature, arguments).await,
        Some(ValueType::U16) => typed_of::<u16>(store, func, signature, arguments).await,
        Some(ValueType::S32) => typed_of::<i32>(store, func, signature, arguments).await,
        Some(ValueType::U32) => typed_of::<u32>(store, func, signature, arguments).await,
        Some(ValueType::S64) => typed_of::<i64>(store, func, signature, arguments).await,
        Some(ValueType::U64) => typed_of::<u64>(store, func, signature, arguments).await,
        Some(ValueType::F32) => typed_of::<f32>(store, func, signature, arguments).await,
        Some(ValueType::F64) => typed_of::<f64>(store, func, signature, arguments).await,
        Some(ValueType::Char) => typed_of::<char>(store, func, signature, arguments).await,
        Some(ValueType::String) => typed_of::<String>(store, func, signature, arguments).await,
    }
}

/// Make a typed call of `signature` whose parameters and result are
/// all of the Rust type `T`.
async fn typed_of<T>(
    store: &mut Store<Host>,
    func: &Func,
    signature: TypedSignature,
    arguments: Vec<Value>,
) -> Outcome
where
    T: Typed + ComponentType + Lower + Lift + Send + Sync + 'static,
{
    let Some(arguments) = arguments
        .into_iter()
        .map(T::from_value)
        .collect::<Option<Vec<T>>>()
    else {
        return Outcome::Failure(format!("an argument is not a {}", T::TYPE));
    };
    let result = |(result,): (T,)| vec![result.into_value()];
    let none = |()| Vec::new();
    // `TypedSignature` holds a typed call to two arguments at most.
    let mut arguments = arguments.into_iter();
    match (arguments.next(), arguments.next(), signature.result) {
        (None, _, false) => typed::<(), ()>(store, func, (), none).await,
        (None, _, true) => typed::<(), (T,)>(store, func, (), result).await,
        (Some(a), None, false) => typed::<(T,), ()>(store, func, (a,), none).await,
        (Some(a), None, true) => typed::<(T,), (T,)>(store, func, (a,), result).await,
        (Some(a), Some(b), false) => typed::<(T, T), ()>(store, func, (a, b), none).await,
        (Some(a), Some(b), true) => typed::<(T, T), (T,)>(store, func, (a, b), result).await,
    }
}

/// Make a typed call of `func` with the Rust types `P` and `R`, and
/// turn its result into the scenario model's values with `values`.
async fn typed<P, R>(
    store: &mut Store<Host>,
    func: &Func,
    arguments: P,
    values: impl FnOnce(R) -> Vec<Value>,
) -> Outcome
where
    P: ComponentNamedList + Lower + Send + Sync + 'static,
    R: ComponentNamedList + Lift + Send + Sync + 'static,
{
    let typed = match func.typed::<P, R>(&*store) {
        Ok(typed) => typed,
        Err(error) => return Outcome::Failure(format!("{error:#}")),
    };
    match typed.call_async(&mut *store, arguments).await {
        Ok(results) => Outcome::Results(values(results)),
        Err(error) => Outcome::Failure(format!("{error:#}")),
    }
}

/// The function an export name names: `add` at the root of the
/// component, or `local:demo/api#greet` inside an exported interface.
fn lookup(mut store: impl AsContextMut, instance: &Instance, export: &str) -> Option<Func> {
    let (interface, name) = match export.split_once('#') {
        Some((interface, name)) => (Some(interface), name),
        None => (None, export),
    };
    let parent = match interface {
        Some(interface) => Some(instance.get_export_index(&mut store, None, interface)?),
        None => None,
    };
    let index = instance.get_export_index(&mut store, parent.as_ref(), name)?;
    instance.get_func(&mut store, index)
}

/// What a scenario's store holds: the WASI context and its resources,
/// and the scenario's instances, which the functions of a run-time link
/// call.
struct Host {
    ctx: WasiCtx,
    table: ResourceTable,
    /// Each component instantiated so far, under its program's name.
    instances: Vec<(String, Instance)>,
}

impl Host {
    /// A WASI context that writes standard output to `stdout` and gives
    /// the guest nothing else of the host: no arguments, no environment,
    /// no files, and no network address. Standard input is closed, and
    /// standard error discards what the guest writes.
    fn new(stdout: MemoryOutputPipe) -> Self {
        Host {
            ctx: WasiCtxBuilder::new().stdout(stdout).build(),
            table: ResourceTable::new(),
            instances: Vec::new(),
        }
    }

    /// The instance of the component `name`, once it is instantiated.
    fn instance(&self, name: &str) -> Option<Instance> {
        self.instances
            .iter()
            .find(|(component, _)| component == name)
            .map(|(_, instance)| *instance)
    }
}

impl WasiView for Host {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.ctx,
            table: &mut self.table,
        }
    }
}

#[cfg(test)]
mod tests {
    use wcmp_scenario::{Expectations, Verdict, Wiring};

    use super::*;
    use crate::program::Program;

    /// A component with a scalar export at its root, the same function
    /// inside an exported interface, and an export that traps.
    const CALCULATOR: &[u8] = wcmp_macros::component!(
        r#"
        (component
          (core module $m
            (func (export "add") (param i32 i32) (result i32)
              local.get 0
              local.get 1
              i32.add)
            (func (export "boom") unreachable))
          (core instance $i (instantiate $m))
          (func $add (param "a" s32) (param "b" s32) (result s32)
            (canon lift (core func $i "add")))
          (func $boom (canon lift (core func $i "boom")))
          (export "add" (func $add))
          (export "boom" (func $boom))
          (instance $api (export "add" (func $add)))
          (export "local:demo/api" (instance $api)))
        "#
    );

    /// A component whose `shout` passes its string through the test
    /// interface's `echo` and returns what `echo` returned.
    const ECHOER: &[u8] = wcmp_macros::component!(
        r#"
        (component
          (import "wcmp:scenario/host" (instance $host
            (export "echo" (func (param "text" string) (result string)))))
          (core module $memory
            (memory (export "memory") 1)
            (global $next (mut i32) (i32.const 1024))
            (func (export "realloc") (param i32 i32 i32 i32) (result i32)
              (local $pointer i32)
              global.get $next
              local.get 2
              i32.const 1
              i32.sub
              i32.add
              i32.const 0
              local.get 2
              i32.sub
              i32.and
              local.tee $pointer
              local.get 3
              i32.add
              global.set $next
              local.get $pointer))
          (core instance $memory (instantiate $memory))
          (alias core export $memory "memory" (core memory $mem))
          (alias core export $memory "realloc" (core func $realloc))
          (alias export $host "echo" (func $echo))
          (core func $echo_lowered
            (canon lower (func $echo) (memory $mem) (realloc $realloc)))
          (core module $main
            (import "host" "echo" (func $echo (param i32 i32 i32)))
            (func (export "shout") (param i32 i32) (result i32)
              local.get 0
              local.get 1
              i32.const 16
              call $echo
              i32.const 16))
          (core instance $main (instantiate $main
            (with "host" (instance (export "echo" (func $echo_lowered))))))
          (func (export "shout") (param "text" string) (result string)
            (canon lift (core func $main "shout") (memory $mem) (realloc $realloc))))
        "#
    );

    /// A component that exports the interface `local:demo/greeter`,
    /// whose `async` function `greet` takes a name and returns `hello, `
    /// followed by the name, written to fresh memory. It is lifted
    /// synchronously, which an `async` function allows.
    const GREETER: &[u8] = wcmp_macros::component!(
        r#"
        (component
          (core module $m
            (memory (export "memory") 1)
            (data (i32.const 0) "hello, ")
            (global $next (mut i32) (i32.const 1024))
            (func $realloc (export "realloc") (param i32 i32 i32 i32) (result i32)
              (local $pointer i32)
              global.get $next
              local.get 2
              i32.const 1
              i32.sub
              i32.add
              i32.const 0
              local.get 2
              i32.sub
              i32.and
              local.tee $pointer
              local.get 3
              i32.add
              global.set $next
              local.get $pointer)
            (func (export "greet") (param $name i32) (param $length i32) (result i32)
              (local $greeting i32)
              (local.set $length (i32.add (local.get $length) (i32.const 7)))
              (local.set $greeting
                (call $realloc (i32.const 0) (i32.const 0) (i32.const 1) (local.get $length)))
              (memory.copy (local.get $greeting) (i32.const 0) (i32.const 7))
              (memory.copy
                (i32.add (local.get $greeting) (i32.const 7))
                (local.get $name)
                (i32.sub (local.get $length) (i32.const 7)))
              (i32.store (i32.const 16) (local.get $greeting))
              (i32.store (i32.const 20) (local.get $length))
              i32.const 16))
          (core instance $i (instantiate $m))
          (type $greet (func async (param "name" string) (result string)))
          (func $greet (type $greet)
            (canon lift (core func $i "greet")
              (memory (core memory $i "memory")) (realloc (core func $i "realloc"))))
          (instance $greeter (export "greet" (func $greet)))
          (export "local:demo/greeter" (instance $greeter)))
        "#
    );

    /// [`GREETER`] lifted as Zena lifts an `async` export: `async` with a
    /// callback. `greet` writes its greeting and yields once instead of
    /// returning, so a caller cannot have the result from the first poll;
    /// the callback it is given back returns the greeting through
    /// `task.return`.
    const CALLBACK_GREETER: &[u8] = wcmp_macros::component!(
        r#"
        (component
          (core module $libc
            (memory (export "memory") 1)
            (data (i32.const 0) "hello, ")
            (global $next (mut i32) (i32.const 1024))
            (func (export "realloc") (param i32 i32 i32 i32) (result i32)
              (local $pointer i32)
              global.get $next
              local.get 2
              i32.const 1
              i32.sub
              i32.add
              i32.const 0
              local.get 2
              i32.sub
              i32.and
              local.tee $pointer
              local.get 3
              i32.add
              global.set $next
              local.get $pointer))
          (core instance $libc (instantiate $libc))
          (alias core export $libc "memory" (core memory $mem))
          (alias core export $libc "realloc" (core func $realloc))
          (core func $task-return (canon task.return (result string) (memory $mem)))
          (core module $main
            (import "libc" "memory" (memory 1))
            (import "libc" "realloc" (func $realloc (param i32 i32 i32 i32) (result i32)))
            (import "" "task.return" (func $task-return (param i32 i32)))
            (global $greeting (mut i32) (i32.const 0))
            (global $length (mut i32) (i32.const 0))
            (func (export "greet") (param $name i32) (param $name-length i32) (result i32)
              (global.set $length (i32.add (local.get $name-length) (i32.const 7)))
              (global.set $greeting
                (call $realloc (i32.const 0) (i32.const 0) (i32.const 1) (global.get $length)))
              (memory.copy (global.get $greeting) (i32.const 0) (i32.const 7))
              (memory.copy
                (i32.add (global.get $greeting) (i32.const 7))
                (local.get $name)
                (local.get $name-length))
              ;; YIELD.
              (i32.const 1))
            (func (export "greet-callback") (param i32 i32 i32) (result i32)
              (call $task-return (global.get $greeting) (global.get $length))
              ;; EXIT.
              (i32.const 0)))
          (core instance $main (instantiate $main
            (with "libc" (instance $libc))
            (with "" (instance (export "task.return" (func $task-return))))))
          (type $greet (func async (param "name" string) (result string)))
          (func $greet (type $greet)
            (canon lift (core func $main "greet") async
              (callback (core func $main "greet-callback")) (memory $mem) (realloc $realloc)))
          (instance $greeter (export "greet" (func $greet)))
          (export "local:demo/greeter" (instance $greeter)))
        "#
    );

    /// [`GREETER`] with a `greet` that is not `async`.
    const SYNC_GREETER: &[u8] = wcmp_macros::component!(
        r#"
        (component
          (core module $m
            (memory (export "memory") 1)
            (func (export "realloc") (param i32 i32 i32 i32) (result i32)
              i32.const 1024)
            (func (export "greet") (param i32 i32) (result i32)
              (i32.store (i32.const 16) (i32.const 0))
              (i32.store (i32.const 20) (i32.const 0))
              i32.const 16))
          (core instance $i (instantiate $m))
          (func $greet (param "name" string) (result string)
            (canon lift (core func $i "greet")
              (memory (core memory $i "memory")) (realloc (core func $i "realloc"))))
          (instance $greeter (export "greet" (func $greet)))
          (export "local:demo/greeter" (instance $greeter)))
        "#
    );

    /// A component that imports `local:demo/greeter` and exports
    /// `welcome`, which passes its name to `greet` and returns what
    /// `greet` returned. It lowers `greet` synchronously.
    const CALLER: &[u8] = wcmp_macros::component!(
        r#"
        (component
          (import "local:demo/greeter" (instance $greeter
            (export "greet" (func async (param "name" string) (result string)))))
          (core module $memory
            (memory (export "memory") 1)
            (global $next (mut i32) (i32.const 1024))
            (func (export "realloc") (param i32 i32 i32 i32) (result i32)
              (local $pointer i32)
              global.get $next
              local.get 2
              i32.const 1
              i32.sub
              i32.add
              i32.const 0
              local.get 2
              i32.sub
              i32.and
              local.tee $pointer
              local.get 3
              i32.add
              global.set $next
              local.get $pointer))
          (core instance $memory (instantiate $memory))
          (alias core export $memory "memory" (core memory $mem))
          (alias core export $memory "realloc" (core func $realloc))
          (alias export $greeter "greet" (func $greet))
          (core func $greet_lowered
            (canon lower (func $greet) (memory $mem) (realloc $realloc)))
          (core module $main
            (import "greeter" "greet" (func $greet (param i32 i32 i32)))
            (func (export "welcome") (param i32 i32) (result i32)
              local.get 0
              local.get 1
              i32.const 16
              call $greet
              i32.const 16))
          (core instance $main (instantiate $main
            (with "greeter" (instance (export "greet" (func $greet_lowered))))))
          (type $welcome (func async (param "name" string) (result string)))
          (func $welcome (type $welcome)
            (canon lift (core func $main "welcome") (memory $mem) (realloc $realloc)))
          (export "welcome" (func $welcome)))
        "#
    );

    /// [`CALLER`] as Zena builds an importer: it lowers `greet` with the
    /// `async` option and lifts `welcome` `async` with a callback.
    ///
    /// `welcome` calls `greet` with the return area at address 16. A call
    /// that returned has its greeting there already, and `welcome` returns
    /// it at once. A call that started joins its subtask to a waitable set
    /// and waits on it; the callback waits again until the subtask has
    /// returned, then drops the subtask and the set and returns the
    /// greeting. `waited` says whether the callback ran, so a test can
    /// tell which of the two ways the call took.
    const ASYNC_CALLER: &[u8] = wcmp_macros::component!(
        r#"
        (component
          (import "local:demo/greeter" (instance $greeter
            (export "greet" (func async (param "name" string) (result string)))))
          (core module $libc
            (memory (export "memory") 1)
            (global $next (mut i32) (i32.const 1024))
            (func (export "realloc") (param i32 i32 i32 i32) (result i32)
              (local $pointer i32)
              global.get $next
              local.get 2
              i32.const 1
              i32.sub
              i32.add
              i32.const 0
              local.get 2
              i32.sub
              i32.and
              local.tee $pointer
              local.get 3
              i32.add
              global.set $next
              local.get $pointer))
          (core instance $libc (instantiate $libc))
          (alias core export $libc "memory" (core memory $mem))
          (alias core export $libc "realloc" (core func $realloc))
          (alias export $greeter "greet" (func $greet))
          (core func $greet
            (canon lower (func $greet) async (memory $mem) (realloc $realloc)))
          (core func $task-return (canon task.return (result string) (memory $mem)))
          (core func $set-new (canon waitable-set.new))
          (core func $join (canon waitable.join))
          (core func $subtask-drop (canon subtask.drop))
          (core func $set-drop (canon waitable-set.drop))
          (core module $main
            (import "libc" "memory" (memory 1))
            (import "" "greet" (func $greet (param i32 i32 i32) (result i32)))
            (import "" "task.return" (func $task-return (param i32 i32)))
            (import "" "waitable-set.new" (func $set-new (result i32)))
            (import "" "waitable.join" (func $join (param i32 i32)))
            (import "" "subtask.drop" (func $subtask-drop (param i32)))
            (import "" "waitable-set.drop" (func $set-drop (param i32)))
            (global $set (mut i32) (i32.const 0))
            (global $waited (mut i32) (i32.const 0))
            (func $return-greeting
              (call $task-return (i32.load (i32.const 16)) (i32.load (i32.const 20))))
            (func (export "welcome") (param i32 i32) (result i32)
              (local $status i32)
              (local.set $status (call $greet (local.get 0) (local.get 1) (i32.const 16)))
              ;; RETURNED.
              (if (i32.eq (i32.and (local.get $status) (i32.const 0xf)) (i32.const 2))
                (then
                  (call $return-greeting)
                  (return (i32.const 0))))
              (global.set $set (call $set-new))
              (call $join (i32.shr_u (local.get $status) (i32.const 4)) (global.get $set))
              ;; WAIT on the set.
              (i32.or (i32.shl (global.get $set) (i32.const 4)) (i32.const 2)))
            (func (export "welcome-callback")
              (param $event i32) (param $subtask i32) (param $state i32) (result i32)
              (global.set $waited (i32.const 1))
              ;; Only the subtask's events, SUBTASK, reach the set.
              (if (i32.ne (local.get $event) (i32.const 1)) (then unreachable))
              ;; Until the subtask has RETURNED, WAIT on the set again.
              (if (i32.ne (local.get $state) (i32.const 2))
                (then (return (i32.or (i32.shl (global.get $set) (i32.const 4)) (i32.const 2)))))
              (call $subtask-drop (local.get $subtask))
              (call $set-drop (global.get $set))
              (call $return-greeting)
              ;; EXIT.
              (i32.const 0))
            (func (export "waited") (result i32) (global.get $waited)))
          (core instance $main (instantiate $main
            (with "libc" (instance $libc))
            (with "" (instance
              (export "greet" (func $greet))
              (export "task.return" (func $task-return))
              (export "waitable-set.new" (func $set-new))
              (export "waitable.join" (func $join))
              (export "subtask.drop" (func $subtask-drop))
              (export "waitable-set.drop" (func $set-drop))))))
          (type $welcome (func async (param "name" string) (result string)))
          (func $welcome (type $welcome)
            (canon lift (core func $main "welcome") async
              (callback (core func $main "welcome-callback")) (memory $mem) (realloc $realloc)))
          (export "welcome" (func $welcome))
          (func (export "waited") (result bool) (canon lift (core func $main "waited"))))
        "#
    );

    /// The wiring of [`CALLER`] to a greeter.
    const CALLER_TO_GREETER: &str = "run-time caller local:demo/greeter greeter";

    /// A component that imports a function no linker here supplies.
    const UNLINKABLE: &[u8] = wcmp_macros::component!(
        r#"
        (component
          (import "wcmp:scenario/missing" (instance
            (export "nothing" (func)))))
        "#
    );

    /// A component whose core module traps in its start function.
    const TRAPS_ON_START: &[u8] = wcmp_macros::component!(
        r#"
        (component
          (core module $m
            (func $start unreachable)
            (start $start))
          (core instance (instantiate $m)))
        "#
    );

    fn program(name: &str, component: &[u8]) -> Program {
        Program {
            name: name.to_string(),
            status: 0,
            log: String::new(),
            component: Some(component.to_vec()),
        }
    }

    fn scenario(programs: Vec<Program>, wiring: &str, expectations: &str) -> Scenario {
        Scenario {
            name: "test".to_string(),
            expectations: expectations.parse::<Expectations>().unwrap(),
            wiring: wiring.parse::<Wiring>().unwrap(),
            programs,
        }
    }

    async fn run(programs: Vec<Program>, expectations: &str) -> Observations {
        run_wired(programs, "", expectations).await
    }

    async fn run_wired(programs: Vec<Program>, wiring: &str, expectations: &str) -> Observations {
        WasmtimeRun::new()
            .unwrap()
            .run(&scenario(programs, wiring, expectations))
            .await
            .unwrap()
    }

    async fn calculate(expectations: &str) -> Observations {
        run(vec![program("calc", CALCULATOR)], expectations).await
    }

    #[wcmp_macros::test]
    async fn it_passes_a_scenario_whose_calls_meet_their_expectations_and_records_what_it_saw() {
        let observations = calculate(
            "
            call calc add(1s32, 2s32) -> 3s32
            call calc local:demo/api#add(2s32, 2s32) -> 4s32
            call calc boom() -> fail
            call calc boom()
            ",
        )
        .await;
        assert_eq!(observations.verdict, Verdict::pass());
        let outcomes: Vec<String> = observations
            .calls
            .iter()
            .map(|observation| observation.to_string())
            .collect();
        assert_eq!(outcomes[0], "call calc add(1s32, 2s32) -> 3s32");
        assert_eq!(
            outcomes[1],
            "call calc local:demo/api#add(2s32, 2s32) -> 4s32"
        );
        // The entry with no outcome keeps the failure Wasmtime saw.
        assert!(matches!(observations.calls[3].outcome, Outcome::Failure(_)));
    }

    #[wcmp_macros::test]
    async fn it_supplies_the_test_interface_that_returns_its_string() {
        let observations = run(
            vec![program("echoer", ECHOER)],
            r#"call echoer shout("hello, wasmtime") -> "hello, wasmtime""#,
        )
        .await;
        assert_eq!(observations.verdict, Verdict::pass());
    }

    #[wcmp_macros::test]
    async fn it_records_a_stage_before_pass_when_the_program_fails_its_expectations() {
        let different = calculate("call calc add(1s32, 2s32) -> 4s32").await;
        assert_eq!(different.verdict.stage, Stage::Mismatch);
        assert_eq!(
            different.verdict.reason,
            "call 1 `calc add(1s32, 2s32)` returned 3s32 where 4s32 was expected"
        );
        let printed = calculate("call calc add(1s32, 2s32) -> 3s32\noutput \"hello\"").await;
        assert_eq!(printed.verdict.stage, Stage::Mismatch);
        let trapped = calculate("call calc boom() -> ()").await;
        assert_eq!(trapped.verdict.stage, Stage::Call);
    }

    #[wcmp_macros::test]
    async fn it_fails_a_call_to_a_missing_component_or_export() {
        let observations = calculate(
            "
            call other add(1s32, 2s32)
            call calc subtract(1s32, 2s32)
            call typed calc local:demo/other#add(1s32, 2s32)
            ",
        )
        .await;
        let reasons: Vec<&str> = observations
            .calls
            .iter()
            .map(|observation| match &observation.outcome {
                Outcome::Failure(reason) => reason.as_str(),
                Outcome::Results(_) => panic!("{observation} succeeded"),
            })
            .collect();
        assert_eq!(
            reasons,
            [
                "the scenario has no component other",
                "component calc has no function export subtract",
                "component calc has no function export local:demo/other#add",
            ]
        );
    }

    #[wcmp_macros::test]
    async fn it_makes_a_typed_call_that_ends_as_the_untyped_call_does() {
        let observations = calculate(
            "
            call calc add(1s32, 2s32) -> 3s32
            call typed calc add(1s32, 2s32) -> 3s32
            call typed calc local:demo/api#add(2s32, 2s32) -> 4s32
            call calc boom() -> fail
            call typed calc boom() -> fail
            ",
        )
        .await;
        assert_eq!(observations.verdict, Verdict::pass());
        let strings = run(
            vec![program("echoer", ECHOER)],
            r#"
            call echoer shout("hello, wasmtime") -> "hello, wasmtime"
            call typed echoer shout("hello, wasmtime") -> "hello, wasmtime"
            "#,
        )
        .await;
        assert_eq!(strings.verdict, Verdict::pass());
    }

    #[wcmp_macros::test]
    async fn it_fails_a_typed_call_outside_the_closed_set_or_of_other_types() {
        let observations = calculate(
            "
            call typed calc add(1s32, 2u32)
            call typed calc add(1u32, 2u32)
            ",
        )
        .await;
        let reasons: Vec<&str> = observations
            .calls
            .iter()
            .map(|observation| match &observation.outcome {
                Outcome::Failure(reason) => reason.as_str(),
                Outcome::Results(_) => panic!("{observation} succeeded"),
            })
            .collect();
        assert_eq!(
            reasons[0],
            wcmp_scenario::Error::Untyped {
                signature: "(s32, u32) -> s32".to_string()
            }
            .to_string()
        );
        // The arguments are `u32`, and `Func::typed` refuses them for an
        // export that takes `s32`.
        assert!(!reasons[1].is_empty());
    }

    #[wcmp_macros::test]
    async fn it_runs_nothing_when_a_program_did_not_compile() {
        let refused = Program {
            name: "refused".to_string(),
            status: 1,
            log: "refused.zena:4:3 - Error: Type mismatch\n".to_string(),
            component: None,
        };
        let observations = run(
            vec![program("calc", CALCULATOR), refused],
            "call calc add(1s32, 2s32) -> 3s32",
        )
        .await;
        assert_eq!(
            observations.verdict,
            Verdict::new(
                Stage::Compile,
                "program refused did not compile (exit 1): refused.zena:4:3 - Error: Type mismatch"
            )
        );
        assert!(observations.calls.is_empty());
    }

    #[wcmp_macros::test]
    async fn it_stops_at_the_first_step_a_component_fails() {
        let parse = run(vec![program("bad", b"\0asm not a component")], "").await;
        assert_eq!(parse.verdict.stage, Stage::Parse);
        assert!(parse.verdict.reason.starts_with("component bad: "));
        let link = run(vec![program("unlinkable", UNLINKABLE)], "").await;
        assert_eq!(link.verdict.stage, Stage::Link);
        assert!(
            link.verdict.reason.contains("wcmp:scenario/missing"),
            "{}",
            link.verdict.reason
        );
        let instantiate = run(vec![program("traps", TRAPS_ON_START)], "").await;
        assert_eq!(instantiate.verdict.stage, Stage::Instantiate);
        // A component that fails an earlier step stops the scenario
        // there, whatever order the programs come in.
        let earliest = run(
            vec![program("traps", TRAPS_ON_START), program("bad", b"\0asm")],
            "",
        )
        .await;
        assert_eq!(earliest.verdict.stage, Stage::Parse);
    }

    #[wcmp_macros::test]
    async fn it_links_async_functions_of_one_component_to_imports_of_another_at_run_time() {
        // The importer's name sorts first, so the wiring orders the
        // exporter before it. The greeting holds the name, so an
        // argument lost or corrupted on its way across the link fails
        // the call.
        let linked = run_wired(
            vec![program("caller", CALLER), program("greeter", GREETER)],
            CALLER_TO_GREETER,
            r#"
            call caller welcome("world") -> "hello, world"
            call caller welcome("") -> "hello, "
            call greeter local:demo/greeter#greet("zena") -> "hello, zena"
            "#,
        )
        .await;
        assert_eq!(linked.verdict, Verdict::pass());

        let unwired = run(
            vec![program("caller", CALLER), program("greeter", GREETER)],
            r#"call caller welcome("world") -> "hello, world""#,
        )
        .await;
        assert_eq!(unwired.verdict.stage, Stage::Link);
        assert!(
            unwired.verdict.reason.starts_with("component caller: ")
                && unwired.verdict.reason.contains("local:demo/greeter"),
            "{}",
            unwired.verdict.reason
        );
    }

    #[wcmp_macros::test]
    async fn it_links_an_async_lowered_import_to_a_callback_lifted_export_as_zena_builds_them() {
        // The greeter yields before it returns, so the caller's call
        // has no result on its first poll: the caller waits on the
        // subtask and its callback runs.
        let linked = run_wired(
            vec![
                program("caller", ASYNC_CALLER),
                program("greeter", CALLBACK_GREETER),
            ],
            CALLER_TO_GREETER,
            r#"
            call caller welcome("world") -> "hello, world"
            call caller waited() -> true
            call caller welcome("zena") -> "hello, zena"
            call greeter local:demo/greeter#greet("zena") -> "hello, zena"
            "#,
        )
        .await;
        assert_eq!(linked.verdict, Verdict::pass());
    }

    #[wcmp_macros::test]
    async fn it_makes_typed_and_untyped_calls_across_a_run_time_link() {
        let linked = run_wired(
            vec![program("caller", CALLER), program("greeter", GREETER)],
            CALLER_TO_GREETER,
            r#"
            call caller welcome("world") -> "hello, world"
            call typed caller welcome("world") -> "hello, world"
            call greeter local:demo/greeter#greet("zena") -> "hello, zena"
            call typed greeter local:demo/greeter#greet("zena") -> "hello, zena"
            "#,
        )
        .await;
        assert_eq!(linked.verdict, Verdict::pass());
    }

    #[wcmp_macros::test]
    async fn it_stops_at_link_when_the_exported_function_is_not_async() {
        let synchronous = run_wired(
            vec![program("caller", CALLER), program("greeter", SYNC_GREETER)],
            CALLER_TO_GREETER,
            "",
        )
        .await;
        assert_eq!(
            synchronous.verdict,
            Verdict::new(
                Stage::Link,
                "component caller: local:demo/greeter#greet is a synchronous function of component greeter, and a run-time link forwards only an `async` one"
            )
        );
    }

    #[wcmp_macros::test]
    async fn it_refuses_a_link_whose_import_or_export_a_component_lacks_instead_of_stopping_at_link()
     {
        let refused = async |greeter: &[u8], wiring: &str| {
            let programs = vec![program("caller", CALLER), program("greeter", greeter)];
            match WasmtimeRun::new()
                .unwrap()
                .run(&scenario(programs, wiring, ""))
                .await
            {
                Err(Error::Judge { source, .. }) => source,
                other => panic!("{other:?}"),
            }
        };
        let misspelled = "run-time caller local:demo/greter greeter";
        assert_eq!(
            refused(GREETER, misspelled).await,
            wcmp_scenario::Error::UnknownImport {
                link: misspelled.to_string(),
                component: "caller".to_string(),
            }
        );
        assert_eq!(
            refused(CALCULATOR, CALLER_TO_GREETER).await,
            wcmp_scenario::Error::UnknownExport {
                link: CALLER_TO_GREETER.to_string(),
                component: "greeter".to_string(),
            }
        );
        // A wiring that leaves no order is refused the same way.
        assert_eq!(
            refused(GREETER, "run-time greeter local:demo/greeter greeter").await,
            wcmp_scenario::Error::LinkCycle(vec!["greeter".to_string()])
        );
    }
}
