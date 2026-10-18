// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Tests of the async imports `wcmp::bindgen!` writes: a WIT `async
//! func` is a `HostWithStore<T>` method that takes `&Accessor<T, Self>`,
//! registered through `func_wrap_concurrent`.
//!
//! The host here is written as a host crate writes one: its traits are
//! implemented on a view of the host data with two borrows, and on its
//! own `HasData` type for any host data, and the embedder registers it
//! with the projection to the view.

#![cfg(test)]

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use std::sync::Arc;

use wcmp::{
    Accessor, Component, Engine, Error, FutureReader, HasData, Instance, Linker, Resource,
    ResourceTable, Store,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

mod bindings {
    wcmp::bindgen!({ path: "wit-async", world: "async-host" });
}

mod doubler {
    wcmp::bindgen!({
        path: "wit-async",
        world: "doubler",
        exports: { default: async | store },
    });
}

use bindings::test::concurrent::host;

/// What the host crate keeps for itself.
#[derive(Default)]
struct Ctx {
    /// Each value a host function was called with, in order.
    touches: Vec<u32>,
    /// The other component `relay` calls.
    doubler: Option<Arc<doubler::Doubler>>,
}

/// The embedder's host data.
#[derive(Default)]
pub struct HostData {
    ctx: Ctx,
    table: ResourceTable,
}

/// The host crate's view of the host data: two borrows of it.
struct View<'a> {
    ctx: &'a mut Ctx,
    table: &'a mut ResourceTable,
}

/// The host crate's [`HasData`].
struct HasView;

impl HasData for HasView {
    type Data<'a> = View<'a>;
}

/// The projection the embedder registers the host crate with.
fn view(data: &mut HostData) -> View<'_> {
    View {
        ctx: &mut data.ctx,
        table: &mut data.table,
    }
}

/// Record `x` in both borrows of `view`, and answer the rep of the
/// handle the table gave it.
fn record(view: View<'_>, x: u32) -> u32 {
    view.ctx.touches.push(x);
    let kept: Resource<u32> = view.table.push(x).expect("the table keeps the value");
    kept.rep()
}

/// A future that is pending the first time it is polled.
#[derive(Default)]
struct PendingOnce {
    polled: bool,
}

impl Future for PendingOnce {
    type Output = ();

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        if this.polled {
            return Poll::Ready(());
        }
        this.polled = true;
        context.waker().wake_by_ref();
        Poll::Pending
    }
}

impl host::Host for View<'_> {
    fn touch(&mut self, x: u32) -> u32 {
        record(
            View {
                ctx: &mut *self.ctx,
                table: &mut *self.table,
            },
            x,
        )
    }
}

impl<T: 'static> host::HostWithStore<T> for HasView {
    async fn answer(accessor: &Accessor<T, Self>, x: u32) -> u32 {
        PendingOnce::default().await;
        // A host crate reaches its view through an accessor of the same
        // store with the projection it was given.
        let again = accessor.with_getter::<HasView>(accessor.getter());
        again.with(|mut access| {
            access.get().ctx.touches.push(x);
        });
        x * 2
    }

    async fn relay(accessor: &Accessor<T, Self>, x: u32) -> u32 {
        let doubler = accessor
            .with(|mut access| access.get().ctx.doubler.clone())
            .expect("the doubler is set");
        let (doubled,) = doubler
            .func_double()
            .call_concurrent(accessor, (x,))
            .await
            .expect("the doubler's export runs");
        let mut reader = accessor
            .with(|mut access| FutureReader::new(access.store(), async { Ok::<u32, Error>(1) }))
            .expect("a future");
        reader
            .close_with(accessor)
            .expect("the close reaches the store");
        doubled
    }
}

/// The guest that calls `test:concurrent/host`: `run-answer` and `run-relay`
/// call the async imports through an asynchronous lower and wait for a
/// call that starts, and `run-touch` calls the plain one.
const CALLS_THE_HOST: &[u8] = component!(
    r#"
    (component
      (import "test:concurrent/host" (instance $host
        (export "answer" (func async (param "x" u32) (result u32)))
        (export "touch" (func (param "x" u32) (result u32)))
        (export "relay" (func async (param "x" u32) (result u32)))))
      (alias export $host "answer" (func $answer))
      (alias export $host "touch" (func $touch))
      (alias export $host "relay" (func $relay))
      (core module $libc (memory (export "memory") 1))
      (core instance $libc (instantiate $libc))
      (core func $answer-lower
        (canon lower (func $answer) async (memory (core memory $libc "memory"))))
      (core func $relay-lower
        (canon lower (func $relay) async (memory (core memory $libc "memory"))))
      (core func $touch-lower (canon lower (func $touch)))
      (core func $task-return (canon task.return (result u32)))
      (core func $set-new (canon waitable-set.new))
      (core func $join (canon waitable.join))
      (core module $m
        (import "libc" "memory" (memory 1))
        (import "" "answer" (func $answer (param i32 i32) (result i32)))
        (import "" "relay" (func $relay (param i32 i32) (result i32)))
        (import "" "touch" (func $touch (param i32) (result i32)))
        (import "" "task.return" (func $task-return (param i32)))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (import "" "waitable.join" (func $join (param i32 i32)))
        (func $wait (param $status i32) (param $area i32) (result i32)
          (local $set i32)
          (if (i32.eq (i32.and (local.get $status) (i32.const 0xf)) (i32.const 2))
            (then
              (call $task-return (i32.load (local.get $area)))
              (return (i32.const 0))))
          (local.set $set (call $set-new))
          (call $join (i32.shr_u (local.get $status) (i32.const 4)) (local.get $set))
          (i32.or (i32.shl (local.get $set) (i32.const 4)) (i32.const 2)))
        (func (export "run-answer") (param i32) (result i32)
          (call $wait (call $answer (local.get 0) (i32.const 0)) (i32.const 0)))
        (func (export "answer-callback") (param i32 i32 i32) (result i32)
          (call $task-return (i32.load (i32.const 0)))
          (i32.const 0))
        (func (export "run-relay") (param i32) (result i32)
          (call $wait (call $relay (local.get 0) (i32.const 8)) (i32.const 8)))
        (func (export "relay-callback") (param i32 i32 i32) (result i32)
          (call $task-return (i32.load (i32.const 8)))
          (i32.const 0))
        (func (export "run-touch") (param i32) (result i32) (call $touch (local.get 0))))
      (core instance $m (instantiate $m
        (with "libc" (instance $libc))
        (with "" (instance
          (export "answer" (func $answer-lower))
          (export "relay" (func $relay-lower))
          (export "touch" (func $touch-lower))
          (export "task.return" (func $task-return))
          (export "waitable-set.new" (func $set-new))
          (export "waitable.join" (func $join))))))
      (func (export "run-answer") async (param "x" u32) (result u32)
        (canon lift (core func $m "run-answer") async
          (callback (core func $m "answer-callback"))))
      (func (export "run-relay") async (param "x" u32) (result u32)
        (canon lift (core func $m "run-relay") async
          (callback (core func $m "relay-callback"))))
      (func (export "run-touch") (param "x" u32) (result u32)
        (canon lift (core func $m "run-touch"))))
    "#
);

/// The component `relay` calls: `double` doubles.
const DOUBLES: &[u8] = component!(
    r#"
    (component
      (core module $m
        (func (export "double") (param i32) (result i32)
          (i32.mul (local.get 0) (i32.const 2))))
      (core instance $i (instantiate $m))
      (func (export "double") (param "x" u32) (result u32)
        (canon lift (core func $i "double"))))
    "#
);

/// Instantiate the doubler and the guest into one store, with the host
/// registered through the projection to its view.
async fn instantiate() -> (Store<HostData>, Instance) {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let mut store = Store::new(&engine, HostData::default()).expect("store");

    let doubles = Component::new(&engine, DOUBLES)
        .await
        .expect("the doubler parses");
    let empty: Linker<HostData> = Linker::new(&engine);
    let world = doubler::Doubler::instantiate_async(&mut store, &doubles, &empty)
        .await
        .expect("the doubler instantiates");
    store.data_mut().ctx.doubler = Some(Arc::new(world));

    let component = Component::new(&engine, CALLS_THE_HOST)
        .await
        .expect("the guest parses");
    let mut linker: Linker<HostData> = Linker::new(&engine);
    host::add_to_linker::<HostData, HasView>(&mut linker, view).expect("the registration");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the async imports link as concurrent registrations");
    (store, instance)
}

/// Call the export `name`, which takes and returns a `u32`.
async fn call(store: &mut Store<HostData>, instance: &Instance, name: &str, x: u32) -> u32 {
    let (value,) = instance
        .get_func(name)
        .unwrap_or_else(|| panic!("the guest exports `{name}`"))
        .typed::<(u32,), (u32,)>()
        .expect("the export's type")
        .call(store, (x,))
        .await
        .unwrap_or_else(|error| panic!("call `{name}`: {error}"));
    value
}

#[wcmp_macros::test]
async fn it_awaits_the_future_of_a_generated_async_method() {
    let (mut store, instance) = instantiate().await;
    assert_eq!(
        call(&mut store, &instance, "run-answer", 21).await,
        42,
        "the guest received what the method's future returned after it was pending once"
    );
    assert_eq!(
        store.data().ctx.touches,
        [21],
        "the method reached the view after its await"
    );
}

#[wcmp_macros::test]
async fn it_reaches_both_borrows_of_the_view_a_host_crate_registered_with() {
    let (mut store, instance) = instantiate().await;
    let rep = call(&mut store, &instance, "run-touch", 5).await;
    assert_eq!(
        store.data().ctx.touches,
        [5],
        "the call reached the context"
    );
    assert_eq!(
        store
            .data()
            .table
            .get(&Resource::<u32>::new_own(rep))
            .ok()
            .copied(),
        Some(5),
        "the call reached the table"
    );
}

#[wcmp_macros::test]
async fn it_calls_an_export_concurrently_and_closes_a_future_through_a_methods_accessor() {
    let (mut store, instance) = instantiate().await;
    assert_eq!(
        call(&mut store, &instance, "run-relay", 8).await,
        16,
        "the method called the doubler through its accessor and returned its result"
    );
}

/// The host of the browser test: its `answer` awaits a JavaScript
/// promise and holds the `JsValue` across the await.
#[cfg(target_arch = "wasm32")]
mod promises {
    use super::*;

    pub struct HasPromises;

    impl HasData for HasPromises {
        type Data<'a> = &'a mut HostData;
    }

    impl host::Host for HostData {
        fn touch(&mut self, x: u32) -> u32 {
            x
        }
    }

    impl<T: 'static> host::HostWithStore<T> for HasPromises {
        async fn answer(_accessor: &Accessor<T, Self>, x: u32) -> u32 {
            let promise = js_sys::Promise::resolve(&wasm_bindgen::JsValue::from_f64(f64::from(x)));
            let held = wasm_bindgen::JsValue::from_str("held across the await");
            let resolved = wasm_bindgen_futures::JsFuture::from(promise)
                .await
                .expect("the promise resolves");
            assert!(
                held.is_string(),
                "the value that is not `Send` is still held"
            );
            resolved.as_f64().expect("a number") as u32 * 3
        }

        async fn relay(_accessor: &Accessor<T, Self>, x: u32) -> u32 {
            x
        }
    }
}

#[cfg(target_arch = "wasm32")]
#[wcmp_macros::test]
async fn it_awaits_a_javascript_promise_in_a_generated_async_method() {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, CALLS_THE_HOST)
        .await
        .expect("the guest parses");
    let mut linker: Linker<HostData> = Linker::new(&engine);
    host::add_to_linker::<HostData, promises::HasPromises>(&mut linker, |data| data)
        .expect("the registration");
    let mut store = Store::new(&engine, HostData::default()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the guest instantiates");
    assert_eq!(
        call(&mut store, &instance, "run-answer", 7).await,
        21,
        "the promise's value reached the guest"
    );
}

#[path = "../../wcmp/tests/support/backend.rs"]
mod test_backend;
