// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Baseline tests for the context slots of components that share a
//! store.
//!
//! The `context.get` and `context.set` built-ins of every component in
//! a store are the exports of one context module, which caches the
//! slots of the thread that runs. The cache must go back to each
//! thread's record however many components the store instantiates, so
//! a fresh thread of any of them starts with zero slots, before the host
//! runs, so a host function reads what the guest wrote, and before the
//! store switches threads, so each thread keeps its own.

#![cfg(test)]

use crate::store::StoreInternalExt;
use crate::{Component, Engine, EngineConfig, HostCall, Linker, Result, Store, Val};
use wcmp_macros::component;

/// A component whose export answers what its slot 0 held and then sets
/// it to the argument.
const SWAP: &[u8] = component!(
    r#"
    (component
      (core func $cget (canon context.get i32 0))
      (core func $cset (canon context.set i32 0))
      (core module $M
        (import "" "context.get" (func $cget (result i32)))
        (import "" "context.set" (func $cset (param i32)))
        (func (export "swap") (param i32) (result i32)
          (call $cget)
          (call $cset (local.get 0))))
      (core instance $m (instantiate $M (with "" (instance
        (export "context.get" (func $cget))
        (export "context.set" (func $cset))))))
      (func (export "swap") (param "x" u32) (result u32)
        (canon lift (core func $m "swap"))))
    "#
);

#[wcmp_macros::test]
async fn it_starts_every_thread_of_two_components_in_one_store_with_zero_slots() {
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, SWAP)
        .await
        .expect("component parses");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let linker: Linker<()> = Linker::new(&engine);
    let first = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate the first");
    let second = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate the second");
    let first = first.get_func("swap").expect("swap export");
    let second = second.get_func("swap").expect("swap export");

    for (call, (func, argument)) in [
        (&first, 7),
        (&second, 9),
        (&first, 11),
        (&first, 13),
        (&second, 15),
    ]
    .into_iter()
    .enumerate()
    {
        let result = func
            .call(&mut store, &[Val::U32(argument)])
            .await
            .expect("the swap returns");
        assert_eq!(
            result.first().cloned(),
            Some(Val::U32(0)),
            "call {call} runs on a fresh thread, whose slot is zero, \
             whatever an earlier call of either component set"
        );
    }
}

/// A component whose export sets its slot 0 to the argument, calls the
/// host's `probe`, and answers what the probe answered. The export traps
/// where its slot no longer holds the argument after the probe returned.
const PROBE: &[u8] = component!(
    r#"
    (component
      (import "probe" (func $probe (result u32)))
      (core func $probe' (canon lower (func $probe)))
      (core func $cget (canon context.get i32 0))
      (core func $cset (canon context.set i32 0))
      (core module $M
        (import "" "context.get" (func $cget (result i32)))
        (import "" "context.set" (func $cset (param i32)))
        (import "" "probe" (func $probe (result i32)))
        (func (export "run") (param i32) (result i32)
          (local $seen i32)
          (call $cset (local.get 0))
          (local.set $seen (call $probe))
          (if (i32.ne (call $cget) (local.get 0)) (then unreachable))
          (local.get $seen)))
      (core instance $m (instantiate $M (with "" (instance
        (export "context.get" (func $cget))
        (export "context.set" (func $cset))
        (export "probe" (func $probe'))))))
      (func (export "run") (param "x" u32) (result u32)
        (canon lift (core func $m "run"))))
    "#
);

#[wcmp_macros::test]
async fn it_shows_the_host_the_slots_a_guest_wrote_and_gives_them_back_after() {
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, PROBE)
        .await
        .expect("component parses");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let tables = store.internal().tables_handle();
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap(
            "probe",
            move |_: HostCall<'_, ()>, (): ()| -> Result<(u32,)> {
                let guard = tables.lock().expect("handle tables");
                let slot = guard
                    .tasks
                    .current_thread()
                    .and_then(|thread| guard.tasks.thread(thread))
                    .map_or(0, |record| record.context[0]);
                Ok((slot as u32,))
            },
        )
        .expect("the registration");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let run = instance.get_func("run").expect("run export");

    for argument in [7, 9] {
        let result = run
            .call(&mut store, &[Val::U32(argument)])
            .await
            .expect("the guest finds its own slot after the probe");
        assert_eq!(
            result.first().cloned(),
            Some(Val::U32(argument)),
            "the probe read the slot the guest wrote before it called the host"
        );
    }
}

/// A component whose export sets its slot 0 to 100 and yields to a
/// thread it starts. The thread finds its slot 0 empty, sets it to 200,
/// calls the host, and finds 200 still. The export answers what its slot
/// 0 holds once the thread ended. Each thread traps where its slot holds
/// another's value.
const TWO_THREADS: &[u8] = component!(
    r#"
    (component
      (core module $libc
        (table (export "__indirect_function_table") 1 funcref))
      (core instance $libc (instantiate $libc))
      (core func $thread-index (canon thread.index))
      (core type $start-ty (func (param i32)))
      (alias core export $libc "__indirect_function_table" (core table $table))
      (core func $new-indirect (canon thread.new-indirect $start-ty (core table $table)))
      (core func $yield-then-resume (canon thread.yield-then-resume))
      (core func $cget (canon context.get i32 0))
      (core func $cset (canon context.set i32 0))
      (core module $M
        (import "" "thread.index" (func $thread-index (result i32)))
        (import "" "thread.new-indirect" (func $new-indirect (param i32 i32) (result i32)))
        (import "" "thread.yield-then-resume" (func $yield-then-resume (param i32) (result i32)))
        (import "" "context.get" (func $cget (result i32)))
        (import "" "context.set" (func $cset (param i32)))
        (import "libc" "__indirect_function_table" (table 1 funcref))
        (global $ran (mut i32) (i32.const 0))
        (func $other (param i32)
          (if (i32.ne (call $cget) (i32.const 0)) (then unreachable))
          (call $cset (i32.const 200))
          (drop (call $thread-index))
          (if (i32.ne (call $cget) (i32.const 200)) (then unreachable))
          (global.set $ran (i32.const 1)))
        (elem (table 0) (i32.const 0) func $other)
        (func (export "run") (result i32)
          (call $cset (i32.const 100))
          (drop (call $yield-then-resume
            (call $new-indirect (i32.const 0) (i32.const 0))))
          (if (i32.eqz (global.get $ran)) (then unreachable))
          (call $cget)))
      (core instance $m (instantiate $M
        (with "" (instance
          (export "thread.index" (func $thread-index))
          (export "thread.new-indirect" (func $new-indirect))
          (export "thread.yield-then-resume" (func $yield-then-resume))
          (export "context.get" (func $cget))
          (export "context.set" (func $cset))))
        (with "libc" (instance $libc))))
      (func (export "run") (result u32)
        (canon lift (core func $m "run"))))
    "#
);

#[wcmp_macros::test]
async fn it_keeps_each_threads_slots_across_a_switch_between_threads() {
    let mut config = EngineConfig::new();
    config.wasm_component_model_threading(true);
    let engine = Engine::with_backend(crate::runtime_layer::test_backend())
        .and_then(|engine| engine.with_config(&config))
        .expect("engine");
    let component = Component::new(&engine, TWO_THREADS)
        .await
        .expect("component parses");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let linker: Linker<()> = Linker::new(&engine);
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let run = instance.get_func("run").expect("run export");

    let result = run
        .call(&mut store, &[])
        .await
        .expect("each thread finds its own slot");
    assert_eq!(
        result.first().cloned(),
        Some(Val::U32(100)),
        "the export's thread finds the slot it wrote before the switch"
    );
}
