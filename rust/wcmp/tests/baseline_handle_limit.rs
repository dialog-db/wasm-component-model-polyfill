// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Baseline tests for the store's cap on its live handles.
//!
//! A store holds at most `max_component_handles` handles at once,
//! across every handle table of every instance in it. A new handle past
//! the cap traps with Wasmtime's message, and a dropped handle gives its
//! place back. The tests are those of Wasmtime 50's
//! `tests/all/component_model/handle_table_limit.rs`.

#![cfg(test)]

use wcmp::{Component, Engine, Error, Instance, Linker, Store, Val};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// Two instances of one component, each with its own handle table.
/// `sprayN(n)` makes `n` live handles. `churnN(n)` makes a handle and
/// drops it `n` times, so one handle at most is live.
const COMPONENT: &[u8] = component!(
    r#"
    (component
      (component $C
        (type $r (resource (rep i32)))
        (core func $new (canon resource.new $r))
        (core func $drop (canon resource.drop $r))
        (core module $m
          (import "" "new" (func $new (param i32) (result i32)))
          (import "" "drop" (func $drop (param i32)))
          (func (export "spray") (param $n i32) (result i32)
            (local $i i32)
            (loop $l
              (if (i32.lt_u (local.get $i) (local.get $n))
                (then
                  (drop (call $new (local.get $i)))
                  (local.set $i (i32.add (local.get $i) (i32.const 1)))
                  (br $l))))
            (local.get $i))
          (func (export "churn") (param $n i32) (result i32)
            (local $i i32)
            (loop $l
              (if (i32.lt_u (local.get $i) (local.get $n))
                (then
                  (call $drop (call $new (local.get $i)))
                  (local.set $i (i32.add (local.get $i) (i32.const 1)))
                  (br $l))))
            (local.get $i)))
        (core instance $i (instantiate $m
          (with "" (instance
            (export "new" (func $new))
            (export "drop" (func $drop))))))
        (func (export "spray") (param "n" u32) (result u32)
          (canon lift (core func $i "spray")))
        (func (export "churn") (param "n" u32) (result u32)
          (canon lift (core func $i "churn"))))
      (instance $i0 (instantiate $C))
      (export "spray0" (func $i0 "spray"))
      (export "churn0" (func $i0 "churn"))
      (instance $i1 (instantiate $C))
      (export "spray1" (func $i1 "spray"))
      (export "churn1" (func $i1 "churn")))
    "#
);

/// The message of a trap past the cap, as Wasmtime words it.
const LIMIT_EXCEEDED: &str = "store handle limit exceeded";

/// A store with its cap at `limit`, and the component's instance in it.
async fn instantiate(limit: usize) -> (Store<()>, Instance) {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("the component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    store.set_max_component_handles(limit);
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiation succeeds");
    (store, instance)
}

/// Call the export `name` with `n`, and answer what it returned.
async fn call(
    store: &mut Store<()>,
    instance: &Instance,
    name: &str,
    n: u32,
) -> Result<u32, Error> {
    let results = instance
        .get_func(name)
        .expect("the export")
        .call(store, &[Val::U32(n)])
        .await?;
    let [Val::U32(count)] = *results else {
        panic!("`{name}` answered {results:?}");
    };
    Ok(count)
}

/// Every message in the source chain of `error`, joined.
fn chain(error: &Error) -> String {
    let mut messages = vec![error.to_string()];
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        messages.push(cause.to_string());
        source = cause.source();
    }
    messages.join(": ")
}

#[wcmp_macros::test]
async fn it_caps_the_handles_of_the_whole_store_and_not_of_one_table() {
    let (mut store, instance) = instantiate(20).await;

    let made = call(&mut store, &instance, "spray0", 12)
        .await
        .expect("12 handles are under the cap of 20");
    assert_eq!(made, 12);

    let error = call(&mut store, &instance, "spray1", 12)
        .await
        .expect_err("12 more in the second instance's table pass the store's cap");
    assert!(
        chain(&error).contains(LIMIT_EXCEEDED),
        "the trap names the cap: {}",
        chain(&error)
    );
}

#[wcmp_macros::test]
async fn it_admits_a_spray_that_a_lower_cap_refuses_once_the_cap_is_raised() {
    let (mut store, instance) = instantiate(10).await;
    assert_eq!(store.max_component_handles(), 10);
    let error = call(&mut store, &instance, "spray0", 50)
        .await
        .expect_err("50 handles pass a cap of 10");
    assert!(
        chain(&error).contains(LIMIT_EXCEEDED),
        "the trap names the cap: {}",
        chain(&error)
    );

    let (mut store, instance) = instantiate(1_000).await;
    assert_eq!(store.max_component_handles(), 1_000);
    let made = call(&mut store, &instance, "spray0", 50)
        .await
        .expect("50 handles are under a cap of 1,000");
    assert_eq!(made, 50);
}

#[wcmp_macros::test]
async fn it_gives_a_dropped_handle_its_place_back() {
    let limit = 100;
    let (mut store, instance) = instantiate(limit).await;

    let total = 100 * limit as u32;
    let churned = call(&mut store, &instance, "churn0", total)
        .await
        .expect("one handle at most is live at a time");
    assert_eq!(churned, total);

    let made = call(&mut store, &instance, "spray0", 50)
        .await
        .expect("the whole cap is free again");
    assert_eq!(made, 50);
}

#[wcmp_macros::test]
fn it_starts_a_store_at_a_cap_of_100_000() {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let store: Store<()> = Store::new(&engine, ()).expect("store");
    assert_eq!(store.max_component_handles(), 100_000);
}

#[path = "support/backend.rs"]
mod test_backend;
