// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Baseline tests for the store's cap on its live handles, for the
//! handles that are not resources.
//!
//! Every handle-table entry counts against the cap, and so does every
//! thread. A built-in that would take the store past the cap traps with
//! Wasmtime's message, and it makes no record first: the refusal leaves
//! the store's records and handles as they were.
//!
//! The call that runs a built-in has an implicit thread, which counts
//! as one handle while the call runs. It is counted but never refused.

#![cfg(test)]

use crate::store::StoreInternalExt;
use crate::{Component, Engine, HostCall, Linker, Result, Store};
use wcmp_macros::component;

/// A component whose exports each make handles of one kind:
/// `new-stream` the two ends of a stream, `new-future` the two ends of
/// a future, `new-set` a waitable set, and `new-thread` an explicit
/// thread, which it never resumes.
const MAKES_HANDLES: &[u8] = component!(
    r#"
    (component
      (type $s (stream u32))
      (type $f (future u32))
      (core module $libc
        (table (export "__indirect_function_table") 1 funcref))
      (core instance $libc (instantiate $libc))
      (core type $start-ty (func (param i32)))
      (alias core export $libc "__indirect_function_table" (core table $table))
      (core func $stream-new (canon stream.new $s))
      (core func $future-new (canon future.new $f))
      (core func $set-new (canon waitable-set.new))
      (core func $new-indirect (canon thread.new-indirect $start-ty (core table $table)))
      (core module $m
        (import "" "stream.new" (func $stream-new (result i64)))
        (import "" "future.new" (func $future-new (result i64)))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (import "" "thread.new-indirect" (func $new-indirect (param i32 i32) (result i32)))
        (import "libc" "__indirect_function_table" (table 1 funcref))
        (func $entry (param i32))
        (elem (i32.const 0) func $entry)
        (func (export "new-stream") (result i32)
          (drop (call $stream-new))
          (i32.const 1))
        (func (export "new-future") (result i32)
          (drop (call $future-new))
          (i32.const 1))
        (func (export "new-set") (result i32)
          (call $set-new))
        (func (export "new-thread") (result i32)
          (call $new-indirect (i32.const 0) (i32.const 0))))
      (core instance $i (instantiate $m (with "" (instance
        (export "stream.new" (func $stream-new))
        (export "future.new" (func $future-new))
        (export "waitable-set.new" (func $set-new))
        (export "thread.new-indirect" (func $new-indirect))))
        (with "libc" (instance $libc))))
      (func (export "new-stream") (result u32) (canon lift (core func $i "new-stream")))
      (func (export "new-future") (result u32) (canon lift (core func $i "new-future")))
      (func (export "new-set") (result u32) (canon lift (core func $i "new-set")))
      (func (export "new-thread") (result u32) (canon lift (core func $i "new-thread"))))
    "#
);

/// The message of a trap past the cap, as Wasmtime words it.
const LIMIT_EXCEEDED: &str = "store handle limit exceeded";

/// How many live handles and live records the store holds.
fn counts(store: &mut Store<()>) -> (usize, usize) {
    let guard = store.internal().lock_tables().expect("handle tables");
    (guard.live_handles(), guard.tasks.record_count())
}

/// Every message in an error's source chain, joined.
fn chain(error: &crate::Error) -> String {
    let mut out = String::new();
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(link) = current {
        if !out.is_empty() {
            out.push_str(": ");
        }
        out.push_str(&link.to_string());
        current = link.source();
    }
    out
}

#[wcmp_macros::test]
async fn it_refuses_ends_a_set_and_a_thread_past_the_cap_and_leaves_no_record() {
    let mut config = crate::EngineConfig::new();
    config.wasm_component_model_threading(true);
    let engine = Engine::with_backend(crate::runtime_layer::test_backend())
        .and_then(|engine| engine.with_config(&config))
        .expect("engine");
    let component = Component::new(&engine, MAKES_HANDLES)
        .await
        .expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    // How many handles each export makes.
    for (name, made) in [
        ("new-stream", 2),
        ("new-future", 2),
        ("new-set", 1),
        ("new-thread", 1),
    ] {
        // A cap one short of the call's implicit thread and the new
        // handles refuses the call. A cap with room for them admits it.
        for (room, admitted) in [(made, false), (made + 1, true)] {
            let mut store: Store<()> = Store::new(&engine, ()).expect("store");
            let instance = linker
                .instantiate(&mut store, &component)
                .await
                .expect("instantiate");
            let (handles, records) = counts(&mut store);
            store.set_max_component_handles(handles + room);
            let called = instance
                .get_func(name)
                .expect("the export is declared")
                .call(&mut store, &[])
                .await;
            if admitted {
                called.unwrap_or_else(|error| panic!("`{name}` fits: {}", chain(&error)));
                assert_eq!(
                    counts(&mut store).0,
                    handles + made,
                    "`{name}` holds its new handles after the call"
                );
            } else {
                let message = chain(&called.map(drop).expect_err("the handles do not fit"));
                assert!(message.contains(LIMIT_EXCEEDED), "`{name}`: {message}");
                assert_eq!(
                    counts(&mut store),
                    (handles, records),
                    "`{name}` left no handle and no record behind"
                );
            }
        }
    }
}

#[wcmp_macros::test]
async fn it_sets_the_cap_through_the_context_a_host_function_is_handed() {
    const TIGHTENS: &[u8] = component!(
        r#"
        (component
          (import "tighten" (func $tighten))
          (core func $tighten (canon lower (func $tighten)))
          (core module $m
            (import "" "tighten" (func $tighten))
            (func (export "run") (call $tighten)))
          (core instance $i (instantiate $m
            (with "" (instance (export "tighten" (func $tighten))))))
          (func (export "run") (canon lift (core func $i "run"))))
        "#
    );
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, TIGHTENS)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap(
            "tighten",
            |mut call: HostCall<'_, ()>, (): ()| -> Result<()> {
                let store = call.store();
                assert_eq!(store.max_component_handles(), 100_000);
                store.set_max_component_handles(7);
                assert_eq!(store.max_component_handles(), 7);
                Ok(())
            },
        )
        .expect("the registration");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    instance
        .get_func("run")
        .expect("the export")
        .call(&mut store, &[])
        .await
        .expect("the call returns");
    assert_eq!(store.max_component_handles(), 7);
}
