// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Tests of the exports `wcmp::bindgen!` writes in the accessor form:
//! under `async | store`, and for every WIT `async func`, a `call_*`
//! method takes an `impl AsAccessor` and owned parameters and calls
//! `TypedFunc::call_concurrent`.

#![cfg(test)]

use wcmp::{Component, Engine, HasData, Linker, Store};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// The world of `wit-exports/`, with every export `async | store`.
mod concurrent {
    wcmp::bindgen!({
        path: "wit-exports",
        exports: { default: async | store },
    });
}

/// A world whose one export is a WIT `async func`, with no flag.
mod async_func {
    wcmp::bindgen!({ path: "wit-async-exports" });
}

/// The host data, and a projection of it that is not `HasSelf`.
#[derive(Default)]
struct HostData {
    calls: u32,
}

/// A projection to the count of calls.
struct HasCalls;

impl HasData for HasCalls {
    type Data<'a> = &'a mut u32;
}

/// A guest whose `compute` is lifted `async` and returns `x + 1` at
/// once through `task.return`.
const COMPUTES: &[u8] = component!(
    r#"
    (component
      (core func $task-return (canon task.return (result u32)))
      (core module $m
        (import "" "task.return" (func $task-return (param i32)))
        (func (export "compute") (param i32) (result i32)
          (call $task-return (i32.add (local.get 0) (i32.const 1)))
          (i32.const 0))
        (func (export "callback") (param i32 i32 i32) (result i32) unreachable))
      (core instance $i (instantiate $m
        (with "" (instance (export "task.return" (func $task-return))))))
      (func (export "compute") async (param "x" u32) (result u32)
        (canon lift (core func $i "compute") async (callback (core func $i "callback")))))
    "#
);

#[wcmp_macros::test]
async fn it_calls_an_async_store_export_through_an_accessor_of_another_projection() {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, guests::IMPLEMENTS_EXPORTS)
        .await
        .expect("the component parses");
    let linker: Linker<HostData> = Linker::new(&engine);
    let mut store = Store::new(&engine, HostData::default()).expect("store");
    let world = concurrent::Exports::instantiate_async(&mut store, &component, &linker)
        .await
        .expect("the world instantiates");

    let (echoed, added) = store
        .run_concurrent(async |accessor| {
            let calls = accessor.with_getter::<HasCalls>(|data| &mut data.calls);
            // Under `async | store` the parameters are owned: `echo`
            // takes a `String`, where its plain form takes a `&str`.
            let text: String = "owned".to_owned();
            let echoed = world.test_exports_shapes().call_echo(&calls, text).await;
            let added = world.call_add(calls.clone(), 2, 3).await;
            calls
                .with(|mut access| *access.get() += 1)
                .expect("reach the projected view");
            (echoed, added)
        })
        .await
        .expect("run the closure");

    assert_eq!(echoed.expect("call echo"), "owned");
    assert_eq!(added.expect("call add"), 5);
    assert_eq!(
        store.data().calls,
        1,
        "the projection reached the host data"
    );
}

#[wcmp_macros::test]
async fn it_calls_an_async_func_export_through_an_accessor_with_no_flag() {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, COMPUTES)
        .await
        .expect("the component parses");
    let linker: Linker<HostData> = Linker::new(&engine);
    let mut store = Store::new(&engine, HostData::default()).expect("store");
    let world = async_func::AsyncExports::instantiate_async(&mut store, &component, &linker)
        .await
        .expect("the world instantiates");

    let computed = store
        .run_concurrent(async |accessor| world.call_compute(accessor, 41).await)
        .await
        .expect("run the closure");
    assert_eq!(computed.expect("call compute"), 42);
}

#[path = "support/guests.rs"]
mod guests;
#[path = "../../wcmp/tests/support/backend.rs"]
mod test_backend;
