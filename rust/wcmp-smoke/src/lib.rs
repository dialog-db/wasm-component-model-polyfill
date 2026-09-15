//! The smoke test: one host program that walks the polyfill from the
//! foundations to a `wac` composition and reports each step. It runs
//! as a native binary (`smoke native`) and as a page in the browser
//! (`smoke web`) from the same source, so a reader can check the
//! polyfill by reading this file and by running it on both targets.
//!
//! Each step is self-contained: it builds its own store, runs a
//! component, and returns the evidence it observed. A failure in one
//! step does not stop the others.

mod host_state;
mod outcome;
mod step;

use std::fmt::Write as _;

use wasm_component_model_polyfill::{Component, Engine, InterfaceIdentifier, Linker, Store, Val};
use wcmp_macros::component;

pub use crate::host_state::HostState;
pub use crate::outcome::Outcome;
pub use crate::step::Step;

/// The `guest` fixture: a component `wasm-tools component new` built
/// from a core module and its WIT. It exports `double`.
const GUEST: &[u8] =
    include_bytes!("../../wasm-component-model-polyfill/tests/corpus/fixtures/guest/guest.wasm");

/// The `composition` fixture: two such components joined by `wac plug`.
/// The socket's `run` calls the plug's `double` through an adapter.
const COMPOSITION: &[u8] = include_bytes!(
    "../../wasm-component-model-polyfill/tests/corpus/fixtures/composition/composed.wasm"
);

/// A component that imports a host function and exports functions
/// whose signatures cross the Canonical ABI in both directions:
/// strings and a list lowered into guest memory through
/// `cabi_realloc`, a string lifted back out, and a scalar handed to
/// the host.
const GREETER: &[u8] = component!(
    r#"
    (component
      (type $host (instance
        (export "tally" (func (param "n" u32)))))
      (import "wcmp:smoke/host@0.1.0" (instance $host (type $host)))
      (alias export $host "tally" (func $tally))
      (core func $core-tally (canon lower (func $tally)))
      (core module $m
        (import "host" "tally" (func $tally (param i32)))
        (memory (export "memory") 1)
        (global $bump (mut i32) (i32.const 1024))
        ;; A bump allocator that honors alignment and moves a block on
        ;; reallocation, which is what the string lowering needs.
        (func $realloc (export "cabi_realloc")
              (param $old i32) (param $old-size i32) (param $align i32) (param $size i32)
              (result i32)
          (local $ptr i32)
          global.get $bump local.get $align i32.add i32.const 1 i32.sub
          local.get $align i32.const 1 i32.sub i32.const -1 i32.xor i32.and
          local.set $ptr
          local.get $ptr local.get $size i32.add global.set $bump
          local.get $old
          i32.eqz
          if
            local.get $ptr
            return
          end
          local.get $ptr
          local.get $old
          local.get $old-size local.get $size
          local.get $old-size local.get $size i32.lt_u
          select
          memory.copy
          local.get $ptr)
        ;; `len(s: string) -> s32`: the byte length of a lowered string.
        (func (export "len") (param i32 i32) (result i32)
          local.get 1)
        ;; `echo(s: string) -> string`: return the lowered string as the
        ;; (pointer, length) pair the lift reads.
        (func (export "echo") (param i32 i32) (result i32)
          (local $ret i32)
          i32.const 0 i32.const 0 i32.const 4 i32.const 8 call $realloc local.set $ret
          local.get $ret local.get 0 i32.store
          local.get $ret local.get 1 i32.store offset=4
          local.get $ret)
        ;; `sum(xs: list<u32>) -> u32`: add up a lowered list.
        (func (export "sum") (param $ptr i32) (param $len i32) (result i32)
          (local $i i32) (local $acc i32)
          block $done
            loop $more
              local.get $i local.get $len i32.ge_u br_if $done
              local.get $acc
              local.get $ptr local.get $i i32.const 4 i32.mul i32.add i32.load
              i32.add local.set $acc
              local.get $i i32.const 1 i32.add local.set $i
              br $more
            end
          end
          local.get $acc)
        ;; `notify(n: u32)`: call the host with twice the argument.
        (func (export "notify") (param i32)
          local.get 0 i32.const 2 i32.mul call $tally))
      (core instance $i (instantiate $m
        (with "host" (instance (export "tally" (func $core-tally))))))
      (func (export "len") (param "s" string) (result s32)
        (canon lift (core func $i "len")
          (memory (core memory $i "memory"))
          (realloc (core func $i "cabi_realloc"))))
      (func (export "echo") (param "s" string) (result string)
        (canon lift (core func $i "echo")
          (memory (core memory $i "memory"))
          (realloc (core func $i "cabi_realloc"))))
      (func (export "sum") (param "xs" (list u32)) (result u32)
        (canon lift (core func $i "sum")
          (memory (core memory $i "memory"))
          (realloc (core func $i "cabi_realloc"))))
      (func (export "notify") (param "n" u32)
        (canon lift (core func $i "notify"))))
    "#
);

/// A component that imports a host resource type and exports a
/// function that drops two owned handles, so the host can watch its
/// destructor run once per handle, in order.
const DROPPER: &[u8] = component!(
    r#"
    (component
      (import "wcmp:smoke/resources@0.1.0" (instance $i
        (export "thing" (type (sub resource)))))
      (alias export $i "thing" (type $thing))
      (core func $thing-drop (canon resource.drop $thing))
      (core module $m
        (func (import "host" "drop") (param i32))
        (func (export "drop2") (param i32 i32)
          local.get 0 call 0
          local.get 1 call 0))
      (core instance $core (instantiate $m
        (with "host" (instance (export "drop" (func $thing-drop))))))
      (func (export "drop2") (param "a" (own $thing)) (param "b" (own $thing))
        (canon lift (core func $core "drop2"))))
    "#
);

/// Run every step and return the report.
pub fn run() -> Vec<Step> {
    let engine = match Engine::new() {
        Ok(engine) => engine,
        Err(err) => {
            return vec![Step {
                name: "foundations",
                outcome: Outcome::Failed(format!("Engine::new failed: {err}")),
            }];
        }
    };
    vec![
        Step::run("foundations", || foundations(&engine)),
        Step::run("real guest from wasm-tools", || real_guest(&engine)),
        Step::run("host function and canonical ABI values", || {
            greeter(&engine)
        }),
        Step::run("host resource with a destructor", || dropper(&engine)),
        composition(&engine),
    ]
}

/// The report as text: one line per step and a summary line.
pub fn render(steps: &[Step]) -> String {
    let mut out = String::new();
    for step in steps {
        let _ = writeln!(
            out,
            "{:<4} {}: {}",
            step.outcome.label(),
            step.name,
            step.outcome.detail()
        );
    }
    let count = |wanted: &str| {
        steps
            .iter()
            .filter(|step| step.outcome.label() == wanted)
            .count()
    };
    let _ = writeln!(
        out,
        "smoke: {} passed, {} failed, {} skipped",
        count("ok"),
        count("FAIL"),
        count("skip")
    );
    out
}

pub fn all_passed(steps: &[Step]) -> bool {
    !steps
        .iter()
        .any(|step| matches!(step.outcome, Outcome::Failed(_)))
}

fn fail(err: impl std::fmt::Display) -> String {
    err.to_string()
}

fn expect<T: PartialEq + std::fmt::Debug>(what: &str, got: T, wanted: T) -> Result<(), String> {
    if got == wanted {
        Ok(())
    } else {
        Err(format!("{what}: expected {wanted:?}, got {got:?}"))
    }
}

/// An engine and a store construct through the public API and the
/// store hands its data back.
fn foundations(engine: &Engine) -> Result<String, String> {
    let mut store: Store<HostState> = Store::new(engine, HostState::default()).map_err(fail)?;
    store.data_mut().tallies.push(7);
    expect("store data", store.data().tallies.as_slice(), &[7])?;
    Ok("engine and store constructed; store data readable and writable".to_owned())
}

/// A component built by a real toolchain loads, instantiates, and
/// answers a typed call.
fn real_guest(engine: &Engine) -> Result<String, String> {
    let component = Component::new(engine, GUEST).map_err(fail)?;
    let linker: Linker<HostState> = Linker::new(engine);
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    let instance = linker.instantiate(&mut store, &component).map_err(fail)?;
    let double = instance
        .get_func("double")
        .ok_or("no `double` export")?
        .typed::<(u32,), u32>()
        .map_err(fail)?;
    let result = double.call(&mut store, (21,)).map_err(fail)?;
    expect("double(21)", result, 42)?;
    Ok(format!(
        "{} bytes of wasm-tools output; double(21) = {result}",
        GUEST.len()
    ))
}

/// Strings and a list lower into guest memory, a string lifts back
/// out, and a typed host function receives what the guest sends.
fn greeter(engine: &Engine) -> Result<String, String> {
    let component = Component::new(engine, GREETER).map_err(fail)?;
    let mut linker: Linker<HostState> = Linker::new(engine);
    let host: InterfaceIdentifier = "wcmp:smoke/host@0.1.0".parse().map_err(fail)?;
    linker.instance(&host).func_wrap(
        "tally",
        |state: &mut HostState, (n,): (u32,)| -> wasm_component_model_polyfill::Result<()> {
            state.tallies.push(n);
            Ok(())
        },
    );
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    let instance = linker.instantiate(&mut store, &component).map_err(fail)?;

    let len = instance
        .get_func("len")
        .ok_or("no `len` export")?
        .typed::<(String,), i32>()
        .map_err(fail)?;
    let length = len.call(&mut store, ("héllo".to_owned(),)).map_err(fail)?;
    expect("len(\"héllo\") in UTF-8 bytes", length, 6)?;

    let echo = instance
        .get_func("echo")
        .ok_or("no `echo` export")?
        .typed::<(String,), String>()
        .map_err(fail)?;
    let echoed = echo
        .call(&mut store, ("round trip".to_owned(),))
        .map_err(fail)?;
    expect("echo", echoed.as_str(), "round trip")?;

    let sum = instance
        .get_func("sum")
        .ok_or("no `sum` export")?
        .typed::<(Vec<u32>,), u32>()
        .map_err(fail)?;
    let total = sum.call(&mut store, (vec![1, 2, 3, 4, 5],)).map_err(fail)?;
    expect("sum([1..5])", total, 15)?;

    let notify = instance
        .get_func("notify")
        .ok_or("no `notify` export")?
        .typed::<(u32,), ()>()
        .map_err(fail)?;
    notify.call(&mut store, (21,)).map_err(fail)?;
    expect("tallies", store.data().tallies.as_slice(), &[42])?;

    Ok(format!(
        "len = {length}, echo = {echoed:?}, sum = {total}, host saw tally({})",
        store.data().tallies[0]
    ))
}

/// A host resource's destructor runs exactly once per handle the guest
/// drops, in drop order.
fn dropper(engine: &Engine) -> Result<String, String> {
    let component = Component::new(engine, DROPPER).map_err(fail)?;
    let mut linker: Linker<HostState> = Linker::new(engine);
    let resources: InterfaceIdentifier = "wcmp:smoke/resources@0.1.0".parse().map_err(fail)?;
    let thing = linker.instance(&resources).resource(
        "thing",
        |state: &mut HostState, rep: u32| -> wasm_component_model_polyfill::Result<()> {
            state.dropped.push(rep);
            Ok(())
        },
    );
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    let instance = linker.instantiate(&mut store, &component).map_err(fail)?;
    let first = store.resource_new(thing, 11).map_err(fail)?;
    let second = store.resource_new(thing, 22).map_err(fail)?;
    let drop2 = instance.get_func("drop2").ok_or("no `drop2` export")?;
    drop2
        .call(&mut store, &[Val::Own(first), Val::Own(second)])
        .map_err(fail)?;
    expect(
        "destructor order",
        store.data().dropped.as_slice(),
        &[11, 22],
    )?;
    Ok(format!(
        "destructor ran for reps {:?}, in drop order",
        store.data().dropped
    ))
}

/// A `wac` composition of two real guests runs through the adapter
/// the translator emits between them.
fn composition(engine: &Engine) -> Step {
    Step::run("wac composition through an adapter", || {
        let component = Component::new(engine, COMPOSITION).map_err(fail)?;
        let linker: Linker<HostState> = Linker::new(engine);
        let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
        let instance = linker.instantiate(&mut store, &component).map_err(fail)?;
        let run = instance
            .get_func("run")
            .ok_or("no `run` export")?
            .typed::<(u32,), u32>()
            .map_err(fail)?;
        let result = run.call(&mut store, (20,)).map_err(fail)?;
        expect("run(20) = double(20) + 1", result, 41)?;
        Ok(format!(
            "{} bytes of wac output; socket.run(20) -> plug.double -> {result}",
            COMPOSITION.len()
        ))
    })
}
