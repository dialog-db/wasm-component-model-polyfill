//! The Wasmtime run of one scenario.

use wasmtime::component::{Component, Func, Instance, Linker, ResourceTable, Val};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::p2::pipe::MemoryOutputPipe;
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wcmp_scenario::{Call, Observations, Outcome, Run, Stage};

use crate::error::{Error, Result};
use crate::scenario::Scenario;
use crate::value::{from_val, to_val};

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
    /// linked, then instantiated, in the order of its program's name,
    /// all in one store, and the first step that fails for any of them
    /// is the stage. Once every component is instantiated, each call of
    /// the expectations is made in order, even after one fails, and the
    /// lines printed to standard output are kept.
    ///
    /// Every call is untyped: `Func::call_async` with `Val` arguments. A
    /// call the expectations mark as typed fails, since typed calls are
    /// not supported here yet. A result that is neither a scalar nor a
    /// string fails its call too, because the scenario model cannot
    /// hold it.
    ///
    /// # Errors
    ///
    /// [`Error::Judge`] when the scenario model refuses the run, which
    /// means a fault of this runner, not a stage of the scenario.
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

        let mut prepared = Vec::with_capacity(components.len());
        for (name, component) in &components {
            match self.linker.instantiate_pre(component) {
                Ok(pre) => prepared.push((*name, pre)),
                Err(error) => return stopped(Stage::Link, format!("component {name}: {error:#}")),
            }
        }

        let stdout = MemoryOutputPipe::new(STDOUT_CAPACITY);
        let mut store = Store::new(&self.engine, Host::new(stdout.clone()));
        let mut instances = Vec::with_capacity(prepared.len());
        for (name, pre) in prepared {
            match pre.instantiate_async(&mut store).await {
                Ok(instance) => instances.push((name, instance)),
                Err(error) => {
                    return stopped(Stage::Instantiate, format!("component {name}: {error:#}"));
                }
            }
        }

        let mut outcomes = Vec::with_capacity(scenario.expectations.entries.len());
        for entry in &scenario.expectations.entries {
            outcomes.push(call(&mut store, &instances, &entry.call).await);
        }
        let output = String::from_utf8_lossy(&stdout.contents())
            .lines()
            .map(str::to_string)
            .collect();
        Observations::observe(&scenario.expectations, Run { outcomes, output }).map_err(judge)
    }
}

/// Make one call and report how it ended.
async fn call(store: &mut Store<Host>, instances: &[(&str, Instance)], call: &Call) -> Outcome {
    if call.typed {
        return Outcome::Failure("the Wasmtime run does not make typed calls yet".to_string());
    }
    let Some((_, instance)) = instances.iter().find(|(name, _)| *name == call.component) else {
        return Outcome::Failure(format!("the scenario has no component {}", call.component));
    };
    let Some(func) = lookup(store, instance, &call.export) else {
        return Outcome::Failure(format!(
            "component {} has no function export {}",
            call.component, call.export
        ));
    };
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

/// The function an export name names: `add` at the root of the
/// component, or `local:demo/api#greet` inside an exported interface.
fn lookup(store: &mut Store<Host>, instance: &Instance, export: &str) -> Option<Func> {
    let (interface, name) = match export.split_once('#') {
        Some((interface, name)) => (Some(interface), name),
        None => (None, export),
    };
    let parent = match interface {
        Some(interface) => Some(instance.get_export_index(&mut *store, None, interface)?),
        None => None,
    };
    let index = instance.get_export_index(&mut *store, parent.as_ref(), name)?;
    instance.get_func(&mut *store, index)
}

/// What a scenario's store holds: the WASI context and its resources.
struct Host {
    ctx: WasiCtx,
    table: ResourceTable,
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
        }
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
    use wcmp_scenario::{Expectations, Verdict};

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

    fn scenario(programs: Vec<Program>, expectations: &str) -> Scenario {
        Scenario {
            name: "test".to_string(),
            expectations: expectations.parse::<Expectations>().unwrap(),
            programs,
        }
    }

    async fn run(programs: Vec<Program>, expectations: &str) -> Observations {
        WasmtimeRun::new()
            .unwrap()
            .run(&scenario(programs, expectations))
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
    async fn it_fails_a_call_to_a_missing_component_or_export_or_a_typed_call() {
        let observations = calculate(
            "
            call other add(1s32, 2s32)
            call calc subtract(1s32, 2s32)
            call calc local:demo/other#add(1s32, 2s32)
            call typed calc add(1s32, 2s32)
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
                "the Wasmtime run does not make typed calls yet",
            ]
        );
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
}
