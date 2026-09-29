//! Baseline tests for the export lifted `canon lift async` with no
//! callback: the stackful form.
//!
//! The translator accepts the form when the engine's
//! `wasm_component_model_async_stackful` gate is on, and refuses it
//! with `Error::Unsupported` when the gate is off, which is the
//! default.
//!
//! A host call into a stackful export is a task. Its core function
//! runs as the task's implicit thread, returns nothing, and delivers
//! the call's result through `task.return`. The task passes the
//! entry gate of its instance but does not take the instance
//! exclusively. When the core function returns, the implicit thread
//! ends, and so does the task when it holds no other thread: a task
//! that has not resolved by then fails the call with the no-result
//! cause. A task that holds an explicit thread ends with the last of
//! its threads instead.

#![cfg(test)]

use core::future::{Future, poll_fn};
use core::task::Poll;

use crate::store::{StoreContextInternalExt, StoreInternalExt};
use crate::{
    Accessor, Component, Engine, EngineConfig, Error, Func, Instance, Linker, Result, Store, Val,
};
use wcmp_macros::component;

/// One core instance behind six exports. `answer`, `after`, `sum`,
/// and `silent` are stackful. `answer` returns twice its argument
/// through `task.return`. `after` returns its argument and then counts
/// one more call in a global, which the synchronous `count` export
/// reads. `sum` takes five flat parameters, one more than an
/// asynchronous lower passes flat, and returns their sum. `silent`
/// returns without calling `task.return`. `held` is lifted with a
/// callback and returns its argument and exits in its first call.
const STACKFUL: &[u8] = component!(
    r#"
    (component
      (core func $task-return (canon task.return (result u32)))
      (core module $m
        (import "" "task.return" (func $task-return (param i32)))
        (global $count (mut i32) (i32.const 0))
        (func (export "answer") (param i32)
          (call $task-return (i32.mul (local.get 0) (i32.const 2))))
        (func (export "after") (param i32)
          (call $task-return (local.get 0))
          (global.set $count (i32.add (global.get $count) (i32.const 1))))
        (func (export "sum") (param i32 i32 i32 i32 i32)
          (call $task-return
            (i32.add (local.get 0)
              (i32.add (local.get 1)
                (i32.add (local.get 2)
                  (i32.add (local.get 3) (local.get 4)))))))
        (func (export "silent") (param i32))
        (func (export "held") (param i32) (result i32)
          (call $task-return (local.get 0))
          (i32.const 0))
        (func (export "held-callback") (param i32 i32 i32) (result i32) unreachable)
        (func (export "count") (result i32) (global.get $count)))
      (core instance $i (instantiate $m
        (with "" (instance (export "task.return" (func $task-return))))))
      (func (export "answer") async (param "x" u32) (result u32)
        (canon lift (core func $i "answer") async))
      (func (export "after") async (param "x" u32) (result u32)
        (canon lift (core func $i "after") async))
      (func (export "sum") async
        (param "a" u32) (param "b" u32) (param "c" u32) (param "d" u32) (param "e" u32)
        (result u32)
        (canon lift (core func $i "sum") async))
      (func (export "silent") async (param "x" u32) (result u32)
        (canon lift (core func $i "silent") async))
      (func (export "held") async (param "x" u32) (result u32)
        (canon lift (core func $i "held") async (callback (core func $i "held-callback"))))
      (func (export "count") (result u32)
        (canon lift (core func $i "count"))))
    "#
);

/// An engine with the stackful lift accepted.
fn stackful_engine() -> Engine {
    let mut config = EngineConfig::new();
    config.wasm_component_model_async_stackful(true);
    Engine::with_config(&config).expect("engine")
}

/// Instantiate [`STACKFUL`] into a fresh store of an engine that
/// accepts the stackful lift.
async fn instantiate() -> (Store<()>, Instance) {
    let engine = stackful_engine();
    let component = Component::new(&engine, STACKFUL)
        .await
        .expect("the stackful lift is accepted with its gate on");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    (store, instance)
}

/// The named export of `instance`.
fn func(instance: &Instance, name: &str) -> Func {
    instance
        .get_func(name)
        .unwrap_or_else(|| panic!("export `{name}` not found"))
}

/// How many task records the store holds.
fn task_count(store: &Store<()>) -> usize {
    store
        .internal_ref()
        .tables()
        .lock()
        .expect("handle tables")
        .tasks
        .task_count()
}

/// Whether some thread holds the store's one component instance
/// exclusively.
fn instance_is_held(store: &Store<()>) -> bool {
    store
        .internal_ref()
        .tables()
        .lock()
        .expect("handle tables")
        .tasks
        .instances()
        .first()
        .expect("one component instance record")
        .exclusive_thread
        .is_some()
}

/// Every message in an error's source chain, joined so that a trap
/// can be matched wherever the substrate put it.
fn chain(error: &Error) -> String {
    let mut out = String::new();
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(link) = current {
        if !out.is_empty() {
            out.push_str(": ");
        }
        out.push_str(&link.to_string());
        current = link.source();
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[wcmp_macros::test]
async fn it_refuses_a_stackful_lift_while_its_gate_is_off() {
    let engine = Engine::new().expect("engine");
    let err = Component::new(&engine, STACKFUL)
        .await
        .expect_err("the default engine leaves the stackful gate off");
    assert!(
        matches!(&err, Error::Unsupported { feature } if feature.contains("stackful")),
        "expected Error::Unsupported naming the stackful lift, got {err:?}"
    );
}

#[wcmp_macros::test]
async fn it_reports_async_on_a_stackful_export() {
    let (_store, instance) = instantiate().await;
    assert!(func(&instance, "answer").ty().async_);
}

#[wcmp_macros::test]
async fn it_returns_the_result_a_stackful_export_hands_to_task_return() {
    let (mut store, instance) = instantiate().await;

    let result = func(&instance, "answer")
        .call(&mut store, &[Val::U32(21)])
        .await
        .expect("the stackful export returns through `task.return`");

    assert_eq!(result.as_ref(), [Val::U32(42)]);
    assert_eq!(
        task_count(&store),
        0,
        "the return of the core function ended the task"
    );
    assert!(!instance_is_held(&store));
}

#[wcmp_macros::test]
async fn it_passes_a_stackful_export_its_parameters_flat_under_the_lift_limit() {
    // Five flat parameters spill for an asynchronous lower, whose
    // limit is four, and stay flat for a lift, whose limit is sixteen
    // whether or not the lift is asynchronous.
    let (mut store, instance) = instantiate().await;

    let result = func(&instance, "sum")
        .call(
            &mut store,
            &[
                Val::U32(1),
                Val::U32(2),
                Val::U32(3),
                Val::U32(4),
                Val::U32(5),
            ],
        )
        .await
        .expect("the core function receives five flat parameters");

    assert_eq!(result.as_ref(), [Val::U32(15)]);
}

#[wcmp_macros::test]
async fn it_runs_the_rest_of_a_stackful_export_after_it_resolves() {
    let (mut store, instance) = instantiate().await;

    let result = func(&instance, "after")
        .call(&mut store, &[Val::U32(7)])
        .await
        .expect("the call resolves at `task.return`");
    assert_eq!(result.as_ref(), [Val::U32(7)]);

    let count = func(&instance, "count")
        .call(&mut store, &[])
        .await
        .expect("read the count");
    assert_eq!(
        count.as_ref(),
        [Val::U32(1)],
        "the core function ran on past `task.return` to its end"
    );
    assert_eq!(task_count(&store), 0);
}

#[wcmp_macros::test]
async fn it_fails_a_stackful_export_that_returns_without_task_return() {
    let (mut store, instance) = instantiate().await;

    let err = func(&instance, "silent")
        .call(&mut store, &[Val::U32(1)])
        .await
        .expect_err("the call has no result to give");

    assert!(
        chain(&err).contains("async-lifted export failed to produce a result"),
        "expected the no-result cause, got {}",
        chain(&err)
    );
    assert_eq!(task_count(&store), 0, "the failed task left the store");
    assert!(!instance_is_held(&store));
}

#[wcmp_macros::test]
async fn it_returns_a_stackful_result_through_a_concurrent_call() {
    let (mut store, instance) = instantiate().await;
    let answer = func(&instance, "answer");

    let result = store
        .run_concurrent(async |accessor| answer.call_concurrent(accessor, &[Val::U32(5)]).await)
        .await
        .expect("run the closure")
        .expect("the stackful export returns through `task.return`");

    assert_eq!(result.as_ref(), [Val::U32(10)]);
    assert_eq!(task_count(&store), 0);
}

#[wcmp_macros::test]
async fn it_ends_the_entry_around_a_concurrent_call_into_a_stackful_export_with_no_result() {
    let (mut store, instance) = instantiate().await;
    let silent = func(&instance, "silent");

    // The no-result trap is not the call's to report: it ends the
    // entry that was polling, and the closure goes with the call.
    let err = match store
        .run_concurrent(async |accessor| silent.call_concurrent(accessor, &[Val::U32(5)]).await)
        .await
    {
        Ok(_) => panic!("the call has no result to give, so the entry fails"),
        Err(err) => err,
    };

    assert!(
        chain(&err).contains("async-lifted export failed to produce a result"),
        "expected the no-result cause, got {}",
        chain(&err)
    );
    assert_eq!(task_count(&store), 0);
}

#[wcmp_macros::test]
async fn it_starts_a_stackful_task_beside_a_callback_task_that_holds_the_instance() {
    let (mut store, instance) = instantiate().await;
    let held = func(&instance, "held");
    let answer = func(&instance, "answer");

    let (at_gate, first, second) = store
        .run_concurrent(async |accessor| two_calls(accessor, (&held, 3), (&answer, 4)).await)
        .await
        .expect("run the closure");

    assert_eq!(
        at_gate, 0,
        "the callback task claimed the instance as its start was queued, and \
         the stackful task, which does not need it, passed the gate anyway"
    );
    assert_eq!(first.expect("the callback call").as_ref(), [Val::U32(3)]);
    assert_eq!(second.expect("the stackful call").as_ref(), [Val::U32(8)]);
    assert_eq!(task_count(&store), 0);
}

/// Start two concurrent calls, note how many tasks wait at the entry
/// gate once both starts are queued and before any turn runs, and
/// drive both until they resolve.
async fn two_calls(
    accessor: &Accessor<()>,
    first_call: (&Func, u32),
    second_call: (&Func, u32),
) -> (usize, Result<Box<[Val]>>, Result<Box<[Val]>>) {
    let first_args = [Val::U32(first_call.1)];
    let second_args = [Val::U32(second_call.1)];
    let mut first = Box::pin(first_call.0.call_concurrent(accessor, &first_args));
    let mut second = Box::pin(second_call.0.call_concurrent(accessor, &second_args));
    let mut first_done = None;
    let mut second_done = None;
    let mut at_gate = None;

    poll_fn(|context| {
        if first_done.is_none()
            && let Poll::Ready(value) = first.as_mut().poll(context)
        {
            first_done = Some(value);
        }
        if second_done.is_none()
            && let Poll::Ready(value) = second.as_mut().poll(context)
        {
            second_done = Some(value);
        }
        if at_gate.is_none() {
            at_gate = Some(
                accessor
                    .with(|store| store.internal().scheduler().waiting_at_gate())
                    .expect("reach the store"),
            );
        }
        if first_done.is_some() && second_done.is_some() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await;

    (
        at_gate.expect("the store was read on the first poll"),
        first_done.expect("the first call resolved"),
        second_done.expect("the second call resolved"),
    )
}
