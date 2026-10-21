// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Baseline tests for thread-transparent adapters.
//!
//! Wasmtime 50's fused adapter compiler calls neither `enter-sync-call`
//! nor `exit-sync-call` for an adapter that is thread transparent
//! (`crates/environ/src/component/thread_transparency.rs` at
//! `v50.0.0`). Such an adapter is sync on both sides, has no handle in
//! its signature, and calls into an instance that declares nothing that
//! can read or write thread state: no host import, no `context.get` or
//! `context.set`, and no task, subtask, waitable, stream, future,
//! error-context, thread, resource, or backpressure built-in. The callee then runs on the
//! caller's task and thread, and no task is created for the call. No
//! guest can tell the difference, because the callee has no built-in
//! that could see it.
//!
//! The polyfill takes its adapters from that compiler, so it inherits
//! the rule. The tests here read the store's records to show it: a
//! host import at the end of a chain of calls counts the tasks on the
//! stack, and a transparent instance in the middle of the chain adds
//! none, where an opaque one adds one.

use std::sync::{Arc, Mutex};

use crate::store::StoreContextInternalExt;
use crate::{Component, Engine, HostCall, Linker, Result, Store, Val};
use wcmp_macros::component;

/// Three chains of synchronous calls that end in `$C`, whose `f` calls
/// the host's `probe` and then returns its own context slot 0.
///
/// `$A`'s `run` sets its context slot 0 to 7 and calls `g`. Each chain
/// gives `$A` another `g`:
///
/// - `through-transparent`: `$B`'s `g`, which only calls `$C`'s `f`.
///   `$B` declares nothing that can touch thread state, so the adapter
///   from `$A` into `$B` is thread transparent.
/// - `through-opaque`: `$B2`'s `g`, which also calls `context.get`, so
///   the adapter from `$A` into `$B2` is not.
/// - `direct`: `$C`'s `f` itself.
///
/// `$C` imports `probe`, so every adapter into `$C` is opaque, and
/// `$C`'s call runs as a task with a fresh thread.
const CHAINS: &[u8] = component!(
    r#"
    (component
      (import "probe" (func $probe))
      (component $C
        (import "probe" (func $probe))
        (core func $probe (canon lower (func $probe)))
        (core func $get (canon context.get i32 0))
        (core module $m
          (import "" "probe" (func $probe))
          (import "" "context.get" (func $get (result i32)))
          (func (export "f") (result i32)
            (call $probe)
            (call $get)))
        (core instance $i (instantiate $m
          (with "" (instance
            (export "probe" (func $probe))
            (export "context.get" (func $get))))))
        (func (export "f") (result u32) (canon lift (core func $i "f"))))
      (component $B
        (import "f" (func $f (result u32)))
        (core func $f (canon lower (func $f)))
        (core module $m
          (import "" "f" (func $f (result i32)))
          (func (export "g") (result i32) (call $f)))
        (core instance $i (instantiate $m (with "" (instance (export "f" (func $f))))))
        (func (export "g") (result u32) (canon lift (core func $i "g"))))
      (component $B2
        (import "f" (func $f (result u32)))
        (core func $f (canon lower (func $f)))
        (core func $get (canon context.get i32 0))
        (core module $m
          (import "" "f" (func $f (result i32)))
          (import "" "context.get" (func $get (result i32)))
          (func (export "g") (result i32)
            (drop (call $get))
            (call $f)))
        (core instance $i (instantiate $m
          (with "" (instance
            (export "f" (func $f))
            (export "context.get" (func $get))))))
        (func (export "g") (result u32) (canon lift (core func $i "g"))))
      (component $A
        (import "g" (func $g (result u32)))
        (core func $g (canon lower (func $g)))
        (core func $set (canon context.set i32 0))
        (core module $m
          (import "" "g" (func $g (result i32)))
          (import "" "context.set" (func $set (param i32)))
          (func (export "run") (result i32)
            (call $set (i32.const 7))
            (call $g)))
        (core instance $i (instantiate $m
          (with "" (instance
            (export "g" (func $g))
            (export "context.set" (func $set))))))
        (func (export "run") (result u32) (canon lift (core func $i "run"))))
      (instance $c (instantiate $C (with "probe" (func $probe))))
      (instance $b (instantiate $B (with "f" (func $c "f"))))
      (instance $b2 (instantiate $B2 (with "f" (func $c "f"))))
      (instance $through-transparent (instantiate $A (with "g" (func $b "g"))))
      (instance $through-opaque (instantiate $A (with "g" (func $b2 "g"))))
      (instance $direct (instantiate $A (with "g" (func $c "f"))))
      (export "through-transparent" (func $through-transparent "run"))
      (export "through-opaque" (func $through-opaque "run"))
      (export "direct" (func $direct "run")))
    "#
);

/// Call the export `name` of [`CHAINS`] in a store of its own, and
/// answer the number of tasks the store held when `$C` called the host,
/// and the context slot `$C` returned.
async fn run_chain(name: &str) -> (usize, u32) {
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, CHAINS)
        .await
        .expect("component parses");
    let seen: Arc<Mutex<Option<usize>>> = Arc::default();
    let probe = seen.clone();
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap(
            "probe",
            move |mut call: HostCall<'_, ()>, (): ()| -> Result<()> {
                let tasks = call
                    .store()
                    .internal_ref()
                    .tables()
                    .lock()
                    .expect("handle tables")
                    .tasks
                    .task_count();
                *probe.lock().expect("probe") = Some(tasks);
                Ok(())
            },
        )
        .expect("the registration");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let results = instance
        .get_func(name)
        .expect("the export")
        .call(&mut store, &[])
        .await
        .unwrap_or_else(|error| panic!("`{name}` fails: {error}"));
    let [Val::U32(slot)] = *results else {
        panic!("`{name}` answered {results:?}");
    };
    let tasks = seen.lock().expect("probe").expect("`$C` called the host");
    (tasks, slot)
}

#[wcmp_macros::test]
async fn it_runs_a_thread_transparent_callee_with_no_task_of_its_own() {
    let (direct, _) = run_chain("direct").await;
    let (transparent, _) = run_chain("through-transparent").await;
    let (opaque, _) = run_chain("through-opaque").await;
    assert_eq!(
        transparent, direct,
        "the transparent call into `$B` adds no task to the stack"
    );
    assert_eq!(
        opaque,
        direct + 1,
        "the opaque call into `$B2` adds one task, so the count can see one"
    );
}

#[wcmp_macros::test]
async fn it_runs_the_last_callee_of_every_chain_on_a_fresh_thread() {
    for name in ["direct", "through-transparent", "through-opaque"] {
        let (_, slot) = run_chain(name).await;
        assert_eq!(
            slot, 0,
            "`$C` runs on a fresh thread through `{name}`, whose slot is zero, \
             not the 7 that `$A` set on its own"
        );
    }
}
