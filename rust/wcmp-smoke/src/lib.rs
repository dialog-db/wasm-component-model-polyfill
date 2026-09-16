//! The smoke test: one host program that walks the polyfill from the
//! foundations through a `wac` composition, real-toolchain maps and
//! fixed-length lists, core modules at the boundary, export
//! navigation by name, a 64-bit memory, and engine configuration,
//! and reports each step. It runs
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

use std::collections::HashMap;
use std::fmt::Write as _;

use wasm_component_model_polyfill::{
    Component, CoreExternType, Engine, EngineConfig, Error, HostCall, InterfaceIdentifier, Linker,
    Store, Val, ValueType,
};
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

/// The `maps` fixture: a `wasm-tools` build whose exports take and
/// return a `map<string, u32>`.
const MAPS: &[u8] =
    include_bytes!("../../wasm-component-model-polyfill/tests/corpus/fixtures/maps/maps.wasm");

/// The `fixed-lists` fixture: a `wasm-tools` build whose exports take
/// and return a `list<u32, 4>` and a `list<u8, 16>`.
const FIXED_LISTS: &[u8] = include_bytes!(
    "../../wasm-component-model-polyfill/tests/corpus/fixtures/fixed-lists/fixed-lists.wasm"
);

/// A component that exports a core module for the host to take: one
/// global and one function, and nothing the component instantiates
/// itself.
const MODULE_PROVIDER: &[u8] = component!(
    r#"
    (component
      (core module $m
        (global (export "g") i32 i32.const 100)
        (func (export "f") (result i32) i32.const 101))
      (export "m" (core module $m)))
    "#
);

/// A component that imports that core module, instantiates it, and
/// exports the sum of its function's result and its global.
const MODULE_CONSUMER: &[u8] = component!(
    r#"
    (component
      (import "m" (core module $m
        (export "f" (func (result i32)))
        (export "g" (global i32))))
      (core instance $provided (instantiate $m))
      (core module $sum
        (import "m" "f" (func $f (result i32)))
        (import "m" "g" (global $g i32))
        (func (export "sum") (result i32)
          call $f global.get $g i32.add))
      (core instance $i (instantiate $sum (with "m" (instance $provided))))
      (func (export "sum") (result u32) (canon lift (core func $i "sum"))))
    "#
);

/// A component whose functions live under a plain-named instance
/// export, `a`, and under an instance nested inside it, `a.b`.
const NESTED_EXPORTS: &[u8] = component!(
    r#"
    (component
      (core module $m
        (func (export "f") (result i32) i32.const 42)
        (func (export "g") (param i32) (result i32) local.get 0 i32.const 1 i32.add))
      (core instance $i (instantiate $m))
      (func $f (result u32) (canon lift (core func $i "f")))
      (func $g (param "x" u32) (result u32) (canon lift (core func $i "g")))
      (instance $b (export "g" (func $g)))
      (instance $a (export "f" (func $f)) (export "b" (instance $b)))
      (export "a" (instance $a)))
    "#
);

/// A 32-bit component that forwards a string to a 64-bit component,
/// which copies it inside its `i64`-addressed memory and hands it
/// back, so the string crosses an adapter in each direction.
const MEMORY64_COMPOSITION: &[u8] = component!(
    r#"
    (component
      (component $c64
        (core module $m
          (memory (export "memory") i64 1)
          (global $next (mut i64) (i64.const 8))
          (func $realloc (export "realloc")
            (param $old i64) (param $old-size i64) (param $align i64) (param $size i64)
            (result i64)
            (local $ret i64)
            (local.set $ret
              (i64.and (i64.add (global.get $next) (i64.const 7)) (i64.const -8)))
            (global.set $next (i64.add (local.get $ret) (local.get $size)))
            (local.get $ret))
          (func (export "roundtrip") (param $ptr i64) (param $len i64) (result i64)
            (local $dst i64)
            (local $ret i64)
            (local.set $dst
              (call $realloc (i64.const 0) (i64.const 0) (i64.const 1) (local.get $len)))
            (memory.copy (local.get $dst) (local.get $ptr) (local.get $len))
            (local.set $ret
              (call $realloc (i64.const 0) (i64.const 0) (i64.const 8) (i64.const 16)))
            (i64.store (local.get $ret) (local.get $dst))
            (i64.store offset=8 (local.get $ret) (local.get $len))
            (local.get $ret)))
        (core instance $m (instantiate $m))
        (func (export "roundtrip") (param "a" string) (result string)
          (canon lift (core func $m "roundtrip")
            (memory (core memory $m "memory"))
            (realloc (core func $m "realloc")))))
      (instance $c64 (instantiate $c64))
      (component $c32
        (import "backend" (instance $i
          (export "roundtrip" (func (param "a" string) (result string)))))
        (core module $libc
          (memory (export "memory") 1)
          (global $next (mut i32) (i32.const 8))
          (func (export "realloc")
            (param $old i32) (param $old-size i32) (param $align i32) (param $size i32)
            (result i32)
            (local $ret i32)
            (local.set $ret
              (i32.and (i32.add (global.get $next) (i32.const 7)) (i32.const -8)))
            (global.set $next (i32.add (local.get $ret) (local.get $size)))
            (local.get $ret)))
        (core instance $libc (instantiate $libc))
        (core func $roundtrip
          (canon lower (func $i "roundtrip")
            (memory (core memory $libc "memory"))
            (realloc (core func $libc "realloc"))))
        (core module $m
          (import "" "memory" (memory 1))
          (import "" "realloc" (func $realloc (param i32 i32 i32 i32) (result i32)))
          (import "" "roundtrip" (func $roundtrip (param i32 i32 i32)))
          (func (export "roundtrip") (param $ptr i32) (param $len i32) (result i32)
            (local $ret i32)
            (local.set $ret
              (call $realloc (i32.const 0) (i32.const 0) (i32.const 1) (i32.const 8)))
            (call $roundtrip (local.get $ptr) (local.get $len) (local.get $ret))
            (local.get $ret)))
        (core instance $m (instantiate $m
          (with "" (instance
            (export "memory" (memory $libc "memory"))
            (export "realloc" (func $libc "realloc"))
            (export "roundtrip" (func $roundtrip))))))
        (func (export "roundtrip") (param "a" string) (result string)
          (canon lift (core func $m "roundtrip")
            (memory (core memory $libc "memory"))
            (realloc (core func $libc "realloc")))))
      (instance $c32 (instantiate $c32 (with "backend" (instance $c64))))
      (export "roundtrip" (func $c32 "roundtrip")))
    "#
);

/// A component whose import carries the gated `implements`
/// annotation, which an engine accepts only when configured to.
const IMPLEMENTS: &[u8] = component!(
    r#"
    (component (import "a" (implements "a:b/c") (instance)))
    "#
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

/// Run every step and return the report. Compiling, instantiating,
/// and calling are awaited, so the browser can compile through its
/// asynchronous API; natively the futures complete at once.
pub async fn run() -> Vec<Step> {
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
        Step::run("foundations", foundations(&engine)).await,
        Step::run("real guest from wasm-tools", real_guest(&engine)).await,
        Step::run("host function and canonical ABI values", greeter(&engine)).await,
        Step::run("host resource with a destructor", dropper(&engine)).await,
        Step::run("disposal from the host", disposal(&engine)).await,
        composition(&engine).await,
        Step::run(
            "maps and fixed-length lists from wasm-tools",
            real_values(&engine),
        )
        .await,
        Step::run("core modules at the boundary", core_modules(&engine)).await,
        Step::run("export navigation by name", navigation(&engine)).await,
        Step::run("string through a 64-bit memory", memory64(&engine)).await,
        Step::run("engine configuration", engine_configuration()).await,
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
async fn foundations(engine: &Engine) -> Result<String, String> {
    let mut store: Store<HostState> = Store::new(engine, HostState::default()).map_err(fail)?;
    store.data_mut().tallies.push(7);
    expect("store data", store.data().tallies.as_slice(), &[7])?;
    Ok("engine and store constructed; store data readable and writable".to_owned())
}

/// A component built by a real toolchain loads, instantiates, and
/// answers a typed call.
async fn real_guest(engine: &Engine) -> Result<String, String> {
    let component = Component::new(engine, GUEST).await.map_err(fail)?;
    let linker: Linker<HostState> = Linker::new(engine);
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .map_err(fail)?;
    let double = instance
        .get_func("double")
        .ok_or("no `double` export")?
        .typed::<(u32,), u32>()
        .map_err(fail)?;
    let result = double.call(&mut store, (21,)).await.map_err(fail)?;
    expect("double(21)", result, 42)?;
    Ok(format!(
        "{} bytes of wasm-tools output; double(21) = {result}",
        GUEST.len()
    ))
}

/// Strings and a list lower into guest memory, a string lifts back
/// out, and a typed host function receives what the guest sends.
async fn greeter(engine: &Engine) -> Result<String, String> {
    let component = Component::new(engine, GREETER).await.map_err(fail)?;
    let mut linker: Linker<HostState> = Linker::new(engine);
    let host: InterfaceIdentifier = "wcmp:smoke/host@0.1.0".parse().map_err(fail)?;
    linker.instance(&host).func_wrap(
        "tally",
        |mut state: HostCall<'_, HostState>,
         (n,): (u32,)|
         -> wasm_component_model_polyfill::Result<()> {
            state.data_mut().tallies.push(n);
            Ok(())
        },
    );
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .map_err(fail)?;

    let len = instance
        .get_func("len")
        .ok_or("no `len` export")?
        .typed::<(String,), i32>()
        .map_err(fail)?;
    let length = len
        .call(&mut store, ("héllo".to_owned(),))
        .await
        .map_err(fail)?;
    expect("len(\"héllo\") in UTF-8 bytes", length, 6)?;

    let echo = instance
        .get_func("echo")
        .ok_or("no `echo` export")?
        .typed::<(String,), String>()
        .map_err(fail)?;
    let echoed = echo
        .call(&mut store, ("round trip".to_owned(),))
        .await
        .map_err(fail)?;
    expect("echo", echoed.as_str(), "round trip")?;

    let sum = instance
        .get_func("sum")
        .ok_or("no `sum` export")?
        .typed::<(Vec<u32>,), u32>()
        .map_err(fail)?;
    let total = sum
        .call(&mut store, (vec![1, 2, 3, 4, 5],))
        .await
        .map_err(fail)?;
    expect("sum([1..5])", total, 15)?;

    let notify = instance
        .get_func("notify")
        .ok_or("no `notify` export")?
        .typed::<(u32,), ()>()
        .map_err(fail)?;
    notify.call(&mut store, (21,)).await.map_err(fail)?;
    expect("tallies", store.data().tallies.as_slice(), &[42])?;

    Ok(format!(
        "len = {length}, echo = {echoed:?}, sum = {total}, host saw tally({})",
        store.data().tallies[0]
    ))
}

/// A host resource's destructor runs exactly once per handle the guest
/// drops, in drop order.
async fn dropper(engine: &Engine) -> Result<String, String> {
    let component = Component::new(engine, DROPPER).await.map_err(fail)?;
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
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .map_err(fail)?;
    let first = store.resource_new(thing, 11).map_err(fail)?;
    let second = store.resource_new(thing, 22).map_err(fail)?;
    let drop2 = instance.get_func("drop2").ok_or("no `drop2` export")?;
    drop2
        .call(&mut store, &[Val::Own(first), Val::Own(second)])
        .await
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

/// The host releases a handle it never handed to the guest, watches
/// the destructor run, then drops the instance and the store.
async fn disposal(engine: &Engine) -> Result<String, String> {
    let component = Component::new(engine, DROPPER).await.map_err(fail)?;
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
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .map_err(fail)?;
    let kept = store.resource_new(thing, 33).map_err(fail)?;
    let leaked = store.resource_new(thing, 44).map_err(fail)?;
    store.resource_drop(kept).map_err(fail)?;
    expect(
        "destructor after release",
        store.data().dropped.as_slice(),
        &[33],
    )?;
    let again = store.resource_drop(kept);
    expect("a second release is refused", again.is_err(), true)?;
    // The instance can go before the store; the store keeps the
    // tables and the destructor.
    drop(instance);
    let _ = leaked;
    let dropped = store.data().dropped.clone();
    drop(store);
    expect(
        "a dropped store runs no destructor for the leaked handle",
        dropped.as_slice(),
        &[33],
    )?;
    Ok(
        "release ran the destructor once; a second release was refused; the store dropped \
        with one leaked handle and ran nothing"
            .to_owned(),
    )
}

/// A `wac` composition of two real guests runs through the adapter
/// the translator emits between them.
async fn composition(engine: &Engine) -> Step {
    Step::run("wac composition through an adapter", async {
        let component = Component::new(engine, COMPOSITION).await.map_err(fail)?;
        let linker: Linker<HostState> = Linker::new(engine);
        let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
        let instance = linker
            .instantiate(&mut store, &component)
            .await
            .map_err(fail)?;
        let run = instance
            .get_func("run")
            .ok_or("no `run` export")?
            .typed::<(u32,), u32>()
            .map_err(fail)?;
        let result = run.call(&mut store, (20,)).await.map_err(fail)?;
        expect("run(20) = double(20) + 1", result, 41)?;
        Ok(format!(
            "{} bytes of wac output; socket.run(20) -> plug.double -> {result}",
            COMPOSITION.len()
        ))
    })
    .await
}

/// Two components built by a real toolchain move a `map<string, u32>`
/// and fixed-length lists in both directions, typed and untyped. The
/// map's keys are sent as `Val::Map` where their order matters, so
/// the evidence is the same on every target.
async fn real_values(engine: &Engine) -> Result<String, String> {
    let linker: Linker<HostState> = Linker::new(engine);
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;

    let maps = Component::new(engine, MAPS).await.map_err(fail)?;
    let maps = linker.instantiate(&mut store, &maps).await.map_err(fail)?;
    let sum = maps
        .get_func("sum")
        .ok_or("no `sum` export")?
        .typed::<(HashMap<String, u32>,), u32>()
        .map_err(fail)?;
    let map: HashMap<String, u32> = [("a", 1), ("b", 2), ("c", 39)]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect();
    let total = sum.call(&mut store, (map,)).await.map_err(fail)?;
    expect("sum({a: 1, b: 2, c: 39})", total, 42)?;
    let entries = Val::Map(Box::new([
        (Val::String("x".to_owned()), Val::U32(7)),
        (Val::String("y".to_owned()), Val::U32(8)),
    ]));
    let keys = maps
        .get_func("keys")
        .ok_or("no `keys` export")?
        .call(&mut store, std::slice::from_ref(&entries))
        .await
        .map_err(fail)?;
    expect(
        "keys({x: 7, y: 8})",
        keys.as_ref(),
        &[Val::List(Box::new([
            Val::String("x".to_owned()),
            Val::String("y".to_owned()),
        ]))],
    )?;
    let back = maps
        .get_func("identity")
        .ok_or("no `identity` export")?
        .call(&mut store, std::slice::from_ref(&entries))
        .await
        .map_err(fail)?;
    expect("identity({x: 7, y: 8})", back.as_ref(), &[entries])?;

    let lists = Component::new(engine, FIXED_LISTS).await.map_err(fail)?;
    let lists = linker.instantiate(&mut store, &lists).await.map_err(fail)?;
    let sum4 = lists
        .get_func("sum")
        .ok_or("no `sum` export")?
        .typed::<([u32; 4],), u32>()
        .map_err(fail)?;
    let total4 = sum4
        .call(&mut store, ([1, 2, 3, 36],))
        .await
        .map_err(fail)?;
    expect("sum([1, 2, 3, 36])", total4, 42)?;
    let double = lists
        .get_func("double")
        .ok_or("no `double` export")?
        .typed::<([u8; 16],), [u8; 16]>()
        .map_err(fail)?;
    let input: [u8; 16] = core::array::from_fn(|i| i as u8);
    let doubled = double.call(&mut store, (input,)).await.map_err(fail)?;
    expect(
        "double(0..16)",
        doubled,
        core::array::from_fn(|i| 2 * i as u8),
    )?;
    let bytes = Val::FixedLengthList(input.iter().map(|b| Val::U8(*b)).collect());
    let same = lists
        .get_func("identity")
        .ok_or("no `identity` export")?
        .call(&mut store, std::slice::from_ref(&bytes))
        .await
        .map_err(fail)?;
    expect("identity(0..16)", same.as_ref(), &[bytes])?;

    Ok(format!(
        "maps.wasm ({} bytes): sum = {total}, keys = [x, y], identity kept 2 entries; \
         fixed-lists.wasm ({} bytes): sum = {total4}, double(0..16) ends in {}, identity kept 16 bytes",
        MAPS.len(),
        FIXED_LISTS.len(),
        doubled[15]
    ))
}

/// A component exports a core module; the host reads its shape,
/// instantiates it itself, and registers it for a second component
/// that instantiates it in turn.
async fn core_modules(engine: &Engine) -> Result<String, String> {
    let provider = Component::new(engine, MODULE_PROVIDER)
        .await
        .map_err(fail)?;
    let linker: Linker<HostState> = Linker::new(engine);
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    let instance = linker
        .instantiate(&mut store, &provider)
        .await
        .map_err(fail)?;
    let module = instance.get_module("m").ok_or("no `m` module export")?;
    let shape: Vec<String> = module
        .exports()
        .iter()
        .map(|export| {
            let kind = match export.ty {
                CoreExternType::Func { .. } => "func",
                CoreExternType::Global { .. } => "global",
                CoreExternType::Memory { .. } => "memory",
                CoreExternType::Table { .. } => "table",
                CoreExternType::Tag { .. } => "tag",
                _ => "other",
            };
            format!("{} ({kind})", export.name)
        })
        .collect();
    expect("module imports", module.imports().len(), 0)?;
    expect(
        "module exports",
        shape.as_slice(),
        &["g (global)".to_owned(), "f (func)".to_owned()],
    )?;
    let core = module.instantiate(&mut store, &[]).await.map_err(fail)?;
    let g = core.get_export(&store, "g").ok_or("no `g` core export")?;
    expect(
        "the host reads the global's type",
        matches!(g.ty(&store), CoreExternType::Global { .. }),
        true,
    )?;

    let consumer = Component::new(engine, MODULE_CONSUMER)
        .await
        .map_err(fail)?;
    let mut linker: Linker<HostState> = Linker::new(engine);
    linker.root().module("m", &module);
    let instance = linker
        .instantiate(&mut store, &consumer)
        .await
        .map_err(fail)?;
    let sum = instance
        .get_func("sum")
        .ok_or("no `sum` export")?
        .typed::<(), u32>()
        .map_err(fail)?;
    let total = sum.call(&mut store, ()).await.map_err(fail)?;
    expect("f() + g", total, 201)?;
    Ok(format!(
        "exported module m has {} imports and exports {}; the host instantiated it, then a \
         second component instantiated it through the linker: f() + g = {total}",
        module.imports().len(),
        shape.join(", ")
    ))
}

/// Functions inside a plain-named instance export and inside a nested
/// one are reached by name, and a handle reports its signature.
async fn navigation(engine: &Engine) -> Result<String, String> {
    let component = Component::new(engine, NESTED_EXPORTS).await.map_err(fail)?;
    let linker: Linker<HostState> = Linker::new(engine);
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .map_err(fail)?;
    expect("no root-level `f`", instance.get_func("f").is_none(), true)?;
    let a = instance
        .exports()
        .instance("a")
        .ok_or("no `a` instance export")?;
    let f = a.func("f").ok_or("no `a.f` export")?;
    let forty_two = f.call(&mut store, &[]).await.map_err(fail)?;
    expect("a.f()", forty_two.as_ref(), &[Val::U32(42)])?;
    let g = a
        .instance("b")
        .ok_or("no `a.b` instance export")?
        .func("g")
        .ok_or("no `a.b.g` export")?;
    let signature = g.ty().clone();
    let parameters: Vec<String> = signature
        .parameters
        .iter()
        .map(|parameter| format!("{}: {}", parameter.name, type_name(&parameter.ty)))
        .collect();
    let g = g.typed::<(u32,), u32>().map_err(fail)?;
    let result = g.call(&mut store, (41,)).await.map_err(fail)?;
    expect("a.b.g(41)", result, 42)?;
    Ok(format!(
        "a.f() = 42, a.b.g(41) = {result}, a.b.g takes ({}) and returns {}",
        parameters.join(", "),
        signature
            .result
            .as_ref()
            .map_or("nothing".to_owned(), type_name)
    ))
}

/// A value type as WIT spells it, for the primitives the smoke test
/// shows; any other shape falls back to the polyfill's debug form.
fn type_name(ty: &ValueType) -> String {
    match ty {
        ValueType::Primitive(primitive) => format!("{primitive:?}").to_lowercase(),
        other => format!("{other:?}"),
    }
}

/// A string goes from the host into a 32-bit component, through an
/// adapter into a 64-bit component that copies it in an `i64`
/// memory, and back.
async fn memory64(engine: &Engine) -> Result<String, String> {
    let component = Component::new(engine, MEMORY64_COMPOSITION)
        .await
        .map_err(fail)?;
    let linker: Linker<HostState> = Linker::new(engine);
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .map_err(fail)?;
    let roundtrip = instance
        .get_func("roundtrip")
        .ok_or("no `roundtrip` export")?
        .typed::<(String,), String>()
        .map_err(fail)?;
    let text = "héllo from a 64-bit memory";
    let back = roundtrip
        .call(&mut store, (text.to_owned(),))
        .await
        .map_err(fail)?;
    expect("roundtrip", back.as_str(), text)?;
    Ok(format!(
        "{:?} crossed into an i64 memory and back, {} bytes each way",
        text,
        text.len()
    ))
}

/// The default engine validates with Wasmtime's feature gates, and a
/// host opts into a gated feature through the engine configuration.
async fn engine_configuration() -> Result<String, String> {
    let strict = Engine::new().map_err(fail)?;
    let rejection = match Component::new(&strict, IMPLEMENTS).await {
        Ok(_) => return Err("the default engine accepted `implements`".to_owned()),
        Err(Error::InvalidComponentBinary { message, .. }) if message.contains("cm-implements") => {
            "the `cm-implements` feature is not active"
        }
        Err(other) => return Err(format!("unexpected rejection: {other}")),
    };
    let mut config = EngineConfig::new();
    config.wasm_component_model_implements(true);
    let permissive = Engine::with_config(&config).map_err(fail)?;
    let component = Component::new(&permissive, IMPLEMENTS)
        .await
        .map_err(fail)?;
    expect(
        "the annotated import is described",
        component.imports.len(),
        1,
    )?;
    Ok(format!(
        "the default engine rejected an `implements` import ({rejection}); an engine that opts \
         in accepted it"
    ))
}
