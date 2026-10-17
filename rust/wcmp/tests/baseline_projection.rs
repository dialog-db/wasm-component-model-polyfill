// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Baseline tests for the projection of the host data: `HasData`,
//! `HasSelf`, `Access`, `Accessor`, and `AsAccessor`.
//!
//! A host crate that serves an interface implements its functions on a
//! view of its own state, not on the embedder's whole host data. The
//! view here holds two separate borrows of the host data, a context and
//! a `ResourceTable`, as a view of a real host does. A synchronous host
//! function reaches it through an `Access` it builds from its call. A
//! reach of an `Accessor` hands its closure an `Access` over the
//! accessor's projection, and `with_getter` gives an accessor of the
//! same store with another projection, which a host future holds
//! across an await. The calls that reach the store from inside a poll
//! take any accessor, whatever its projection.

#![cfg(test)]

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};

use wcmp::{
    Access, Accessor, Component, Engine, Error, HasData, HostCall, Instance, Linker, Resource,
    ResourceTable, SchedulerCause, Store, TypedFunc,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// A component that imports a synchronous `touch` and exports `run`,
/// which calls it with its argument and returns what it returned.
const CALLS_A_HOST_FUNCTION: &[u8] = component!(
    r#"
    (component
      (import "touch" (func $touch (param "x" u32) (result u32)))
      (core func $lowered (canon lower (func $touch)))
      (core module $m
        (import "" "touch" (func $touch (param i32) (result i32)))
        (func (export "run") (param i32) (result i32)
          (call $touch (local.get 0))))
      (core instance $i (instantiate $m
        (with "" (instance (export "touch" (func $lowered))))))
      (func (export "run") (param "x" u32) (result u32)
        (canon lift (core func $i "run"))))
    "#
);

/// A component that lowers an async-typed `answer` with the `async`
/// option and returns its result from `run`.
///
/// `run` is lifted `async` with a callback. A call that returned gives
/// its result back at once; a call that started joins its subtask to a
/// set and waits on it, and the callback returns the result the
/// lowering wrote.
const CALLS_A_HOST_ASYNC_FUNCTION: &[u8] = component!(
    r#"
    (component
      (import "answer" (func $answer async (param "x" u32) (result u32)))
      (core module $libc (memory (export "memory") 1))
      (core instance $libc (instantiate $libc))
      (core func $lowered
        (canon lower (func $answer) async (memory (core memory $libc "memory"))))
      (core func $task-return (canon task.return (result u32)))
      (core func $set-new (canon waitable-set.new))
      (core func $join (canon waitable.join))
      (core module $m
        (import "libc" "memory" (memory 1))
        (import "" "answer" (func $answer (param i32 i32) (result i32)))
        (import "" "task.return" (func $task-return (param i32)))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (import "" "waitable.join" (func $join (param i32 i32)))
        (func (export "run") (param i32) (result i32)
          (local $status i32)
          (local $set i32)
          (local.set $status (call $answer (local.get 0) (i32.const 0)))
          (if (i32.eq (i32.and (local.get $status) (i32.const 0xf)) (i32.const 2))
            (then
              (call $task-return (i32.load (i32.const 0)))
              (return (i32.const 0))))
          (local.set $set (call $set-new))
          (call $join
            (i32.shr_u (local.get $status) (i32.const 4))
            (local.get $set))
          (i32.or (i32.shl (local.get $set) (i32.const 4)) (i32.const 2)))
        (func (export "run-callback") (param i32 i32 i32) (result i32)
          (call $task-return (i32.load (i32.const 0)))
          (i32.const 0)))
      (core instance $m (instantiate $m
        (with "libc" (instance $libc))
        (with "" (instance
          (export "answer" (func $lowered))
          (export "task.return" (func $task-return))
          (export "waitable-set.new" (func $set-new))
          (export "waitable.join" (func $join))))))
      (func (export "run") async (param "x" u32) (result u32)
        (canon lift (core func $m "run") async
          (callback (core func $m "run-callback")))))
    "#
);

/// A component that exports `double`, which doubles its argument.
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

/// The part of the host data a host crate keeps for itself.
#[derive(Default)]
struct Ctx {
    /// Each value a host function was called with, in order.
    touches: Vec<u32>,
}

/// The embedder's host data: the crate's context and the table its
/// resources live in, side by side.
#[derive(Default)]
struct Host {
    ctx: Ctx,
    table: ResourceTable,
}

/// The crate's view of the host data: two separate borrows of it.
struct View<'a> {
    ctx: &'a mut Ctx,
    table: &'a mut ResourceTable,
}

/// The [`HasData`] that names [`View`].
struct HasView;

impl HasData for HasView {
    type Data<'a> = View<'a>;
}

/// The projection from the host data to the crate's view.
fn view(host: &mut Host) -> View<'_> {
    View {
        ctx: &mut host.ctx,
        table: &mut host.table,
    }
}

/// What the crate does with a value it is handed: record it in the
/// context, and keep it in the table. Answers the rep of the handle
/// the table gave it.
fn touch(view: View<'_>, x: u32) -> Result<u32, Error> {
    view.ctx.touches.push(x);
    let kept: Resource<u32> = view.table.push(x)?;
    Ok(kept.rep())
}

/// A future that is pending the first time it is polled and ready the
/// next, which is a host call that does not finish where it starts.
/// It wakes the waker it was polled with before it parks, so the next
/// turn polls it again.
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

/// Instantiate `bytes` into a store of [`Host`], with the imports
/// `register` adds.
async fn instantiate(
    bytes: &[u8],
    register: impl FnOnce(&mut Linker<Host>),
) -> (Store<Host>, Instance) {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, bytes)
        .await
        .expect("the component parses");
    let mut linker: Linker<Host> = Linker::new(&engine);
    register(&mut linker);
    let mut store = Store::new(&engine, Host::default()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the component instantiates");
    (store, instance)
}

/// The export `name` of `instance`, typed as one `u32` in and out.
fn u32_export(instance: &Instance, name: &str) -> TypedFunc<(u32,), (u32,)> {
    instance
        .get_func(name)
        .unwrap_or_else(|| panic!("the component exports `{name}`"))
        .typed::<(u32,), (u32,)>()
        .expect("the export's type")
}

#[wcmp_macros::test]
async fn it_reaches_both_borrows_of_a_view_from_a_synchronous_host_function() {
    // The host function builds an `Access` over the crate's projection
    // from the context of its call, which is how a function with the
    // `store` flag reaches its view with no new kind of registration.
    let (mut store, instance) = instantiate(CALLS_A_HOST_FUNCTION, |linker| {
        linker
            .root()
            .func_wrap("touch", |call: HostCall<'_, Host>, (x,): (u32,)| {
                let mut access = Access::<Host, HasView>::new(call.into_store(), view);
                Ok((touch(access.get(), x)?,))
            })
            .expect("the registration");
    })
    .await;

    let (rep,) = u32_export(&instance, "run")
        .call(&mut store, (7,))
        .await
        .expect("call run");

    assert_eq!(
        store.data().ctx.touches,
        [7],
        "the call reached the context"
    );
    assert_eq!(
        store
            .data()
            .table
            .get(&Resource::<u32>::new_own(rep))
            .ok()
            .copied(),
        Some(7),
        "the call reached the table, through the same view"
    );
}

#[wcmp_macros::test]
async fn it_hands_the_reach_of_an_accessor_the_view_its_projection_gives() {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let mut store = Store::new(&engine, Host::default()).expect("store");

    let (rep, nested) = store
        .run_concurrent(async |accessor| {
            let projected: Accessor<Host, HasView> = accessor.with_getter::<HasView>(view);
            projected
                .with(|mut access| {
                    let rep = touch(access.get(), 3).expect("the view keeps the value");
                    // The outer reach holds the store, so the token used
                    // again from inside it is refused, whatever its
                    // projection.
                    let nested = projected
                        .with(|_access| ())
                        .expect_err("the inner reach is refused");
                    (rep, nested.to_string())
                })
                .expect("the outer reach succeeds")
        })
        .await
        .expect("run the closure");

    assert_eq!(
        store.data().ctx.touches,
        [3],
        "the reach reached the context"
    );
    assert_eq!(
        store
            .data()
            .table
            .get(&Resource::<u32>::new_own(rep))
            .ok()
            .copied(),
        Some(3),
        "the reach reached the table"
    );
    assert_eq!(
        nested,
        Error::Scheduler(SchedulerCause::RecursiveDriver).to_string(),
        "a reach from inside another reach still fails"
    );
}

#[wcmp_macros::test]
async fn it_reaches_the_view_of_another_projection_from_a_host_future_after_an_await() {
    // The registration's accessor projects to the whole host data. The
    // closure gives the future an accessor of the same store over the
    // crate's projection, and the future reaches the view through it
    // on the other side of an await, in a later poll of the store.
    let (mut store, instance) = instantiate(CALLS_A_HOST_ASYNC_FUNCTION, |linker| {
        linker
            .root()
            .func_wrap_concurrent("answer", |accessor: &Accessor<Host>, (x,): (u32,)| {
                let projected = accessor.with_getter::<HasView>(view);
                async move {
                    PendingOnce::default().await;
                    let rep = projected.with(|mut access| touch(access.get(), x))??;
                    Ok((rep,))
                }
            })
            .expect("the registration");
    })
    .await;

    let (rep,) = u32_export(&instance, "run")
        .call(&mut store, (11,))
        .await
        .expect("call run");

    assert_eq!(
        store.data().ctx.touches,
        [11],
        "the future reached the context after its await"
    );
    assert_eq!(
        store
            .data()
            .table
            .get(&Resource::<u32>::new_own(rep))
            .ok()
            .copied(),
        Some(11),
        "the future reached the table after its await, and the guest got \
         the rep it answered"
    );
}

#[wcmp_macros::test]
async fn it_calls_an_export_concurrently_through_an_accessor_of_another_projection() {
    let (mut store, instance) = instantiate(DOUBLES, |_linker| {}).await;
    let double = u32_export(&instance, "double");

    let doubled = store
        .run_concurrent(async |accessor| {
            let projected = accessor.with_getter::<HasView>(view);
            let by_value = double.call_concurrent(projected.clone(), (21,)).await;
            let by_reference = double.call_concurrent(&projected, (4,)).await;
            (by_value, by_reference)
        })
        .await
        .expect("run the closure");

    assert_eq!(
        (
            doubled.0.expect("the call by value"),
            doubled.1.expect("the call by reference")
        ),
        ((42,), (8,)),
        "each call returned the export's result"
    );
}

#[path = "support/backend.rs"]
mod test_backend;
