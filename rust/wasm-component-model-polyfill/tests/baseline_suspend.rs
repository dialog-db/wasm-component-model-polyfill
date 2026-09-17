//! Baseline tests for reaching the scheduler's state from a host
//! function the guest called.
//!
//! A provider of the scheduler's suspend capability resumes a
//! suspended guest thread outside any poll of a driver: in the
//! browser the resumption lands on a microtask, and the driver that
//! started the guest has already returned. The scheduler's state is
//! therefore reachable through the store's handle tables rather than
//! through a driver, and a host trampoline finds it there with no
//! driver of its own. These tests read it from a host function the
//! guest called, which is the frame a blocking built-in runs in, and
//! again after the call, when no driver is on the stack at all.

#![cfg(test)]

use std::sync::{Arc, Mutex};

use wasm_component_model_polyfill::{Component, Engine, HostCall, Linker, Result, Store, Val};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// What a reader saw of the scheduler's state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Seen {
    /// Whether a turn of the store was running.
    in_turn: bool,
    /// Whether the store held the waker of a running turn, which is
    /// what a trampoline polls a host task with.
    has_waker: bool,
}

/// A component whose export calls a host function, so the host can
/// read the scheduler's state from the frame a blocking built-in
/// would run in.
const CALLS_THE_HOST: &[u8] = component!(
    r#"
    (component
      (import "probe" (func $probe (param "x" u32) (result u32)))
      (core func $probe' (canon lower (func $probe)))
      (core module $m
        (import "" "probe" (func $probe (param i32) (result i32)))
        (func (export "run") (param i32) (result i32)
          local.get 0 call $probe i32.const 1 i32.add))
      (core instance $i (instantiate $m
        (with "" (instance (export "probe" (func $probe'))))))
      (func (export "run") (param "x" u32) (result u32)
        (canon lift (core func $i "run"))))
    "#
);

#[wcmp_macros::test]
async fn it_reads_the_schedulers_state_from_a_host_function_called_by_the_guest() {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, CALLS_THE_HOST)
        .await
        .expect("component parses");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");

    // The one handle a trampoline has to the store. Nothing else of
    // the call reaches the host function: no driver, no store
    // reference, only this.
    let tables = store.tables.clone();
    let during: Arc<Mutex<Seen>> = Arc::new(Mutex::new(Seen::default()));
    let recorded = during.clone();

    let mut linker: Linker<()> = Linker::new(&engine);
    linker.root().func_wrap(
        "probe",
        move |_: HostCall<'_, ()>, (x,): (u32,)| -> Result<u32> {
            let guard = tables.lock().expect("handle tables");
            *recorded.lock().expect("record") = Seen {
                in_turn: guard.scheduler.in_turn(),
                has_waker: guard.scheduler.active_waker().is_some(),
            };
            Ok(x)
        },
    );

    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let run = instance.get_func("run").expect("run export");
    let result = run
        .call(&mut store, &[Val::U32(41)])
        .await
        .expect("call run");

    assert_eq!(result.first(), Some(&Val::U32(42)), "the export returned");
    assert_eq!(
        *during.lock().expect("record"),
        Seen {
            in_turn: true,
            has_waker: true,
        },
        "the host function reached the scheduler's state from inside the \
         trampoline, and found the turn that is running and its waker"
    );

    let guard = store.tables.lock().expect("handle tables");
    let after = Seen {
        in_turn: guard.scheduler.in_turn(),
        has_waker: guard.scheduler.active_waker().is_some(),
    };
    drop(guard);

    assert_eq!(
        after,
        Seen::default(),
        "the same state is reachable with no driver on the stack, and reports \
         that no turn is running"
    );
}
