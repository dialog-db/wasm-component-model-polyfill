//! Baseline tests for a destructor that re-enters its own
//! resource's drop.
//!
//! `resource.drop` is a host function, and it runs the destructor
//! from inside itself. A destructor that drops a second handle of
//! the same resource type in the same instance therefore calls that
//! one host function a second time while the first call is still on
//! the stack.
//!
//! A native engine enters a host function at any depth and runs the
//! component to its end. The browser cannot: a host function there
//! is one JavaScript function object over one Rust closure, and the
//! arguments and results of a call belong to that call alone, so the
//! backend refuses the second call. The component is sound either
//! way; only the target differs, and the refusal reaches the host as
//! the scheduler cause that names the limitation rather than as a
//! substrate string, which is what lets a host tell the two apart.
//!
//! The shape needs no nested turn and no concurrency at all — one
//! synchronous export, one resource type, two handles.

#![cfg(test)]

use wasm_component_model_polyfill::{Component, Engine, Linker, Result, Store, Val};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// A component whose resource destructor drops a second handle of
/// its own resource type.
///
/// The destructor of a resource cannot name that resource's
/// `resource.drop` directly: the core function is defined from the
/// resource type, and the resource type is defined from the
/// destructor. The component breaks the cycle the way wit-bindgen
/// breaks the same one for a lowered import — a `$shim` module
/// exports a funcref table and a function that calls through it, the
/// destructor calls that function, and a `$fixups` module writes
/// `resource.drop` into the table once the resource type exists. The
/// call the destructor makes is the drop's own host function either
/// way.
///
/// `run` mints two handles, arms the destructor with the second, and
/// drops the first. The first destructor run drops the second handle
/// and disarms itself, so the second run drops nothing, and `run`
/// answers with how many destructor runs the component counted.
const REENTRANT_DROP: &[u8] = component!(
    r#"
    (component
      (core module $shim
        (table (export "$imports") 1 1 funcref)
        (func (export "0") (param i32)
          local.get 0
          i32.const 0
          call_indirect (param i32)))
      (core instance $shim (instantiate $shim))
      (core module $Dtor
        (import "" "again" (func $again (param i32)))
        (global $runs (mut i32) (i32.const 0))
        (global $armed (mut i32) (i32.const 0))
        (global $extra (mut i32) (i32.const 0))
        (func (export "arm") (param i32)
          (global.set $extra (local.get 0))
          (global.set $armed (i32.const 1)))
        (func (export "runs") (result i32) (global.get $runs))
        (func (export "dtor") (param i32)
          (global.set $runs (i32.add (global.get $runs) (i32.const 1)))
          (if (i32.eqz (global.get $armed)) (then (return)))
          (global.set $armed (i32.const 0))
          (call $again (global.get $extra))))
      (core instance $dtor (instantiate $Dtor (with "" (instance
        (export "again" (func $shim "0"))))))
      (type $r (resource (rep i32) (dtor (core func $dtor "dtor"))))
      (core func $new (canon resource.new $r))
      (core func $drop (canon resource.drop $r))
      (core module $fixups
        (import "" "0" (func (param i32)))
        (import "" "$imports" (table 1 1 funcref))
        (elem (i32.const 0) func 0))
      (core instance (instantiate $fixups (with "" (instance
        (export "0" (func $drop))
        (export "$imports" (table $shim "$imports"))))))
      (core module $M
        (import "" "new" (func $new (param i32) (result i32)))
        (import "" "drop" (func $drop (param i32)))
        (import "" "arm" (func $arm (param i32)))
        (import "" "runs" (func $runs (result i32)))
        (func (export "run") (result i32)
          (local $first i32)
          (local.set $first (call $new (i32.const 100)))
          (call $arm (call $new (i32.const 200)))
          (call $drop (local.get $first))
          (call $runs)))
      (core instance $m (instantiate $M (with "" (instance
        (export "new" (func $new))
        (export "drop" (func $drop))
        (export "arm" (func $dtor "arm"))
        (export "runs" (func $dtor "runs"))))))
      (func (export "run") (result u32)
        (canon lift (core func $m "run"))))
    "#
);

/// Instantiate `REENTRANT_DROP` and call its `run` export.
async fn run_component() -> Result<Box<[Val]>> {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, REENTRANT_DROP)
        .await
        .expect("component parses");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let linker: Linker<()> = Linker::new(&engine);
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let run = instance.get_func("run").expect("run export");
    run.call(&mut store, &[]).await
}

/// What the call answers on a target whose host functions can be
/// called at any depth: the nested drop is an ordinary call, both
/// destructors run, and the export returns the count.
#[cfg(not(target_arch = "wasm32"))]
#[wcmp_macros::test]
async fn it_runs_a_destructor_that_re_enters_its_own_drop() {
    let result = run_component().await;

    assert_eq!(
        result.expect("the call returned").first().cloned(),
        Some(Val::U32(2)),
        "a native engine calls `resource.drop` while a call of it is still on \
         the stack, so the first destructor dropped the second handle and both \
         destructor runs were counted"
    );
}

/// What the call answers in the browser: the destructor's drop is a
/// second call of the host function its own call is inside, which
/// the backend refuses, and the refusal reaches the host as the
/// cause that names the limitation rather than as a substrate
/// failure of the drop.
#[cfg(target_arch = "wasm32")]
#[wcmp_macros::test]
async fn it_refuses_a_destructor_that_re_enters_its_own_drop() {
    use wasm_component_model_polyfill::{Error, SchedulerCause};

    let failure = run_component().await.expect_err("the call fails");

    assert!(
        matches!(failure, Error::Scheduler(SchedulerCause::ReentrantHostCall)),
        "the destructor called the host function its own run is inside, which \
         this target refuses: {failure}"
    );
    assert!(
        failure
            .to_string()
            .contains(&SchedulerCause::ReentrantHostCall.to_string()),
        "the failure carries the cause's own message, which names the \
         limitation: {failure}"
    );
}
