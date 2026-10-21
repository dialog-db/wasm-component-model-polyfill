// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Baseline tests for the driver's turn budget.
//!
//! A driver runs turns for as long as they make progress. After 128
//! turns without a yield, it hands control back to the host executor
//! and wakes itself, as Wasmtime 50's event loop does (`e2a58746c`). In
//! the browser the wake crosses a macrotask, so the page can run too.
//!
//! The guest here is the one of Wasmtime 50's
//! `guest_cancellation_loop_does_not_starve_tokio`
//! (`tests/all/component_model/async.rs`): a callback task that starts a
//! host call that never finishes, cancels it, and starts another, over
//! and over, while it waits for a second host call, `probe`. Every turn
//! of that loop makes progress. `probe` finishes only once a task that
//! the test spawned on the host executor has run, and that task runs
//! only when the driver gives the executor a turn. The guest traps after
//! 500 cancellations, so a driver that never gives way fails the call.

#![cfg(test)]

use core::future::poll_fn;
use core::task::{Poll, Waker};
use std::sync::{Arc, Mutex};

use wcmp::{Accessor, Component, Engine, Error, Linker, Store};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// The guest, as Wasmtime's test has it, without the import that
/// spends Tokio's cooperative budget, which only Tokio has.
const CANCELLATION_LOOP: &[u8] = component!(
    r#"
    (component
      (import "probe" (func $probe async))
      (import "pending" (func $pending async))
      (core module $m
        (import "" "probe" (func $probe (result i32)))
        (import "" "pending" (func $pending (result i32)))
        (import "" "cancel" (func $cancel (param i32) (result i32)))
        (import "" "drop" (func $drop (param i32)))
        (import "" "new" (func $new (result i32)))
        (import "" "join" (func $join (param i32 i32)))
        (import "" "task.return" (func $task.return))
        (global $set (mut i32) (i32.const 0))
        (global $probe-handle (mut i32) (i32.const 0))
        (global $pending-handle (mut i32) (i32.const 0))
        (global $cancellations (mut i32) (i32.const 0))
        (func $start (param $status i32) (result i32)
          (if (i32.ne (i32.and (local.get $status) (i32.const 15)) (i32.const 1))
            (then unreachable))
          (i32.shr_u (local.get $status) (i32.const 4)))
        (func $start-pending
          (global.set $pending-handle (call $start (call $pending)))
          (if (i32.ne (call $cancel (global.get $pending-handle)) (i32.const -1))
            (then unreachable))
          (call $join (global.get $pending-handle) (global.get $set)))
        (func (export "run") (result i32)
          (global.set $set (call $new))
          (global.set $probe-handle (call $start (call $probe)))
          (call $join (global.get $probe-handle) (global.get $set))
          (call $start-pending)
          (i32.or (i32.const 2) (i32.shl (global.get $set) (i32.const 4))))
        (func (export "callback") (param $event i32) (param $waitable i32)
          (param $code i32) (result i32)
          (if (i32.ne (local.get $event) (i32.const 1)) (then unreachable))
          (if (i32.eq (local.get $waitable) (global.get $probe-handle))
            (then
              (if (i32.ne (local.get $code) (i32.const 2)) (then unreachable))
              (call $drop (global.get $probe-handle))
              (call $task.return)
              (return (i32.const 0))))
          (if (i32.ne (local.get $waitable) (global.get $pending-handle))
            (then unreachable))
          (if (i32.ne (local.get $code) (i32.const 4)) (then unreachable))
          (call $drop (global.get $pending-handle))
          (global.set $cancellations (i32.add (global.get $cancellations) (i32.const 1)))
          (if (i32.gt_u (global.get $cancellations) (i32.const 500))
            (then unreachable))
          (call $start-pending)
          (i32.or (i32.const 2) (i32.shl (global.get $set) (i32.const 4)))))
      (core func $probe (canon lower (func $probe) async))
      (core func $pending (canon lower (func $pending) async))
      (core func $cancel (canon subtask.cancel async))
      (core func $drop (canon subtask.drop))
      (core func $new (canon waitable-set.new))
      (core func $join (canon waitable.join))
      (core func $task.return (canon task.return))
      (core instance $i (instantiate $m (with "" (instance
        (export "probe" (func $probe))
        (export "pending" (func $pending))
        (export "cancel" (func $cancel))
        (export "drop" (func $drop))
        (export "new" (func $new))
        (export "join" (func $join))
        (export "task.return" (func $task.return))))))
      (func (export "run") async
        (canon lift (core func $i "run") async (callback (core func $i "callback")))))
    "#
);

/// Whether the task on the host executor has run, and the waker of the
/// `probe` call that waits for it.
#[derive(Default)]
struct Gate {
    open: bool,
    waiter: Option<Waker>,
}

/// Run `task` on the test's host executor, which runs it only when the
/// test's own future gives the executor a turn.
fn spawn_on_host(task: impl core::future::Future<Output = ()> + Send + 'static) {
    #[cfg(not(target_arch = "wasm32"))]
    drop(tokio::spawn(task));
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(task);
}

#[wcmp_macros::test]
async fn it_gives_the_host_executor_a_turn_during_a_long_run_of_progress() {
    let mut config = wcmp::EngineConfig::new();
    config.wasm_component_model_more_async_builtins(true);
    let engine = Engine::with_backend(crate::test_backend::backend())
        .and_then(|engine| engine.with_config(&config))
        .expect("engine");
    let component = Component::new(&engine, CANCELLATION_LOOP)
        .await
        .expect("the component parses");
    let gate: Arc<Mutex<Gate>> = Arc::default();
    let mut linker: Linker<()> = Linker::new(&engine);
    let probe = gate.clone();
    linker
        .root()
        .func_wrap_concurrent("probe", move |_accessor: &Accessor<()>, (): ()| {
            let gate = probe.clone();
            poll_fn(move |context| {
                let mut gate = gate.lock().expect("gate");
                if gate.open {
                    Poll::Ready(Ok::<(), Error>(()))
                } else {
                    gate.waiter = Some(context.waker().clone());
                    Poll::Pending
                }
            })
        })
        .expect("the registration of `probe`");
    linker
        .root()
        .func_wrap_concurrent("pending", |_accessor: &Accessor<()>, (): ()| {
            core::future::pending::<Result<(), Error>>()
        })
        .expect("the registration of `pending`");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiation succeeds");

    let opener = gate.clone();
    spawn_on_host(async move {
        let mut gate = opener.lock().expect("gate");
        gate.open = true;
        if let Some(waiter) = gate.waiter.take() {
            waiter.wake();
        }
    });
    instance
        .get_func("run")
        .expect("the export")
        .call(&mut store, &[])
        .await
        .expect("the host task ran before the guest's loop gave up");
}

#[path = "support/backend.rs"]
mod test_backend;
