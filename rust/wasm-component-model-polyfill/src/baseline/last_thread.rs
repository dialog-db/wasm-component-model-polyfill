//! Baseline tests for the end of a task that holds explicit threads.
//!
//! A task ends when its last thread does, which is the reference's
//! `unregister_thread`. Its implicit thread can exit first: the
//! export's core function returns, or its callback exits, while an
//! explicit thread `thread.new-indirect` made is still there, started
//! or not. The task then goes on with that thread. The thread runs in
//! a later turn and may call `task.return`, and the no-result check
//! and the borrow check run when it ends, not when the implicit
//! thread exits.
//!
//! The component keeps what its threads did in a core global, `work`,
//! which a synchronous export reads back. A thread that runs after
//! the call that made it has returned is observed through
//! `read-after-yield`, a callback export that yields once before it
//! reads: its yield queues it behind the thread's start, so the
//! thread runs first.

#![cfg(test)]

use crate::store::StoreInternalExt;
use crate::{
    Component, Engine, EngineConfig, Error, Func, HostResource, Instance, InterfaceIdentifier,
    Linker, ResourceTypeId, Result, Store, Val,
};
use wcmp_macros::component;

/// One component instance whose table holds three thread start
/// functions:
///
/// - 0, `record`, writes its context to `work`.
/// - 1, `return`, writes its context to `work` and returns it through
///   `task.return`.
/// - 2, `drop-and-return`, drops the borrow the task was handed and
///   returns its context through `task.return`.
///
/// The exports each make one thread ready, or leave one suspended,
/// and let the implicit thread exit:
///
/// - `resolve-then-spawn` is stackful. It returns 1, then makes a
///   `record` thread with the context 5 ready.
/// - `spawn-returner` and `spawn-returner-callback` make a `return`
///   thread ready, with the context 7 and 9, and never call
///   `task.return` themselves. The first is stackful, the second exits
///   its callback loop at once.
/// - `spawn-recorder` and `spawn-recorder-callback` make a `record`
///   thread ready, with the context 3 and 4, and no thread of their
///   task ever calls `task.return`.
/// - `leave-suspended` is stackful. It creates a `record` thread,
///   never makes it ready, and returns the thread's index.
/// - `hand-borrow-on` is stackful and takes a borrow of the host's
///   `thing`. It keeps the borrow, makes a `drop-and-return` thread
///   with the context 11 ready, and exits still owing the borrow.
const LAST_THREAD: &[u8] = component!(
    r#"
    (component
      (import "wcmp-tests:host/things@0.1.0" (instance $i
        (export "thing" (type $thing (sub resource)))))
      (alias export $i "thing" (type $thing))

      (core module $libc
        (table (export "__indirect_function_table") 3 funcref))
      (core instance $libc (instantiate $libc))

      (core func $task-return (canon task.return (result u32)))
      (core type $start-ty (func (param i32)))
      (alias core export $libc "__indirect_function_table" (core table $table))
      (core func $new-indirect (canon thread.new-indirect $start-ty (core table $table)))
      (core func $resume-later (canon thread.resume-later))
      (core func $drop-thing (canon resource.drop $thing))

      (core module $m
        (import "" "task.return" (func $task-return (param i32)))
        (import "" "thread.new-indirect" (func $new-indirect (param i32 i32) (result i32)))
        (import "" "thread.resume-later" (func $resume-later (param i32)))
        (import "" "drop-thing" (func $drop-thing (param i32)))
        (import "libc" "__indirect_function_table" (table 3 funcref))

        (global $work (mut i32) (i32.const 0))
        (global $held (mut i32) (i32.const 0))

        (func $record (param i32)
          (global.set $work (local.get 0)))
        (func $return (param i32)
          (global.set $work (local.get 0))
          (call $task-return (local.get 0)))
        (func $drop-and-return (param i32)
          (call $drop-thing (global.get $held))
          (call $task-return (local.get 0)))
        (elem (table 0) (i32.const 0) func $record $return $drop-and-return)

        (func $spawn-ready (param $entry i32) (param $context i32)
          (call $resume-later
            (call $new-indirect (local.get $entry) (local.get $context))))

        (func (export "resolve-then-spawn")
          (call $task-return (i32.const 1))
          (call $spawn-ready (i32.const 0) (i32.const 5)))
        (func (export "spawn-returner")
          (call $spawn-ready (i32.const 1) (i32.const 7)))
        (func (export "spawn-returner-callback") (result i32)
          (call $spawn-ready (i32.const 1) (i32.const 9))
          ;; Exit.
          (i32.const 0))
        (func (export "spawn-recorder")
          (call $spawn-ready (i32.const 0) (i32.const 3)))
        (func (export "spawn-recorder-callback") (result i32)
          (call $spawn-ready (i32.const 0) (i32.const 4))
          ;; Exit.
          (i32.const 0))
        (func (export "leave-suspended")
          (call $task-return (call $new-indirect (i32.const 0) (i32.const 0))))
        (func (export "hand-borrow-on") (param i32)
          (global.set $held (local.get 0))
          (call $spawn-ready (i32.const 2) (i32.const 11)))
        (func (export "read-after-yield") (result i32)
          ;; Yield.
          (i32.const 1))
        (func (export "read-callback") (param i32 i32 i32) (result i32)
          (call $task-return (global.get $work))
          ;; Exit.
          (i32.const 0))
        (func (export "never") (param i32 i32 i32) (result i32) unreachable)
        (func (export "work") (result i32) (global.get $work)))

      (core instance $c (instantiate $m
        (with "" (instance
          (export "task.return" (func $task-return))
          (export "thread.new-indirect" (func $new-indirect))
          (export "thread.resume-later" (func $resume-later))
          (export "drop-thing" (func $drop-thing))))
        (with "libc" (instance $libc))))

      (func (export "resolve-then-spawn") async (result u32)
        (canon lift (core func $c "resolve-then-spawn") async))
      (func (export "spawn-returner") async (result u32)
        (canon lift (core func $c "spawn-returner") async))
      (func (export "spawn-returner-callback") async (result u32)
        (canon lift (core func $c "spawn-returner-callback") async
          (callback (core func $c "never"))))
      (func (export "spawn-recorder") async (result u32)
        (canon lift (core func $c "spawn-recorder") async))
      (func (export "spawn-recorder-callback") async (result u32)
        (canon lift (core func $c "spawn-recorder-callback") async
          (callback (core func $c "never"))))
      (func (export "leave-suspended") async (result u32)
        (canon lift (core func $c "leave-suspended") async))
      (func (export "hand-borrow-on") async (param "h" (borrow $thing)) (result u32)
        (canon lift (core func $c "hand-borrow-on") async))
      (func (export "read-after-yield") async (result u32)
        (canon lift (core func $c "read-after-yield") async
          (callback (core func $c "read-callback"))))
      (func (export "work") (result u32)
        (canon lift (core func $c "work"))))
    "#
);

/// An engine with the stackful lift and the thread built-ins allowed.
fn threading_engine() -> Engine {
    let mut config = EngineConfig::new();
    config.wasm_component_model_async_stackful(true);
    config.wasm_component_model_threading(true);
    Engine::with_config(&config).expect("engine")
}

/// Instantiate [`LAST_THREAD`] in a fresh store, with the host's
/// `thing` registered, and answer the resource type the host mints
/// handles of.
async fn instantiate() -> (Store<()>, Instance, ResourceTypeId) {
    let engine = threading_engine();
    let component = Component::new(&engine, LAST_THREAD)
        .await
        .expect("component parses");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let mut linker: Linker<()> = Linker::new(&engine);
    let interface: InterfaceIdentifier =
        "wcmp-tests:host/things@0.1.0".parse().expect("identifier");
    let thing = linker
        .instance(&interface)
        .resource_with(
            "thing",
            HostResource::new(|_: &mut (), _: u32| -> Result<()> { Ok(()) }),
        )
        .expect("the registration");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    (store, instance, thing)
}

/// One export of the instance, by name.
fn func(instance: &Instance, name: &str) -> Func {
    instance.get_func(name).expect("the export is declared")
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

/// Call `name` with `args` and expect it to return one `u32`.
async fn call_u32(store: &mut Store<()>, instance: &Instance, name: &str, args: &[Val]) -> u32 {
    match func(instance, name).call(store, args).await {
        Ok(values) => match values.first() {
            Some(Val::U32(value)) => *value,
            other => panic!("{name} answered {other:?}"),
        },
        Err(error) => panic!("{name} failed: {}", chain(&error)),
    }
}

/// Call `name` and expect it to fail, reporting the message.
async fn call_expecting_a_trap(store: &mut Store<()>, instance: &Instance, name: &str) -> String {
    match func(instance, name).call(store, &[]).await {
        Err(error) => chain(&error),
        Ok(values) => panic!("{name} returned {values:?} rather than trapping"),
    }
}

#[wcmp_macros::test]
async fn it_runs_a_ready_thread_of_a_task_whose_implicit_thread_exited() {
    let (mut store, instance, _) = instantiate().await;

    assert_eq!(
        call_u32(&mut store, &instance, "resolve-then-spawn", &[]).await,
        1,
        "the call returns at `task.return`, before the thread runs"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "read-after-yield", &[]).await,
        5,
        "the thread ran after the implicit thread of its task exited"
    );
    assert_eq!(task_count(&store), 0, "the task ended with its last thread");
}

#[wcmp_macros::test]
async fn it_resolves_a_stackful_task_through_a_thread_that_runs_after_the_implicit_thread_exits() {
    let (mut store, instance, _) = instantiate().await;

    assert_eq!(
        call_u32(&mut store, &instance, "spawn-returner", &[]).await,
        7,
        "the thread's `task.return` resolved the call, with no no-result \
         failure at the exit of the implicit thread"
    );
    assert_eq!(task_count(&store), 0);
}

#[wcmp_macros::test]
async fn it_resolves_a_callback_task_through_a_thread_that_runs_after_the_callback_exits() {
    let (mut store, instance, _) = instantiate().await;

    assert_eq!(
        call_u32(&mut store, &instance, "spawn-returner-callback", &[]).await,
        9,
        "the thread's `task.return` resolved the call, with no no-result \
         failure at the exit code"
    );
    assert_eq!(task_count(&store), 0);
}

#[wcmp_macros::test]
async fn it_fails_a_stackful_task_with_no_result_once_its_last_thread_has_run() {
    let (mut store, instance, _) = instantiate().await;

    let message = call_expecting_a_trap(&mut store, &instance, "spawn-recorder").await;
    assert!(
        message.contains("async-lifted export failed to produce a result"),
        "expected the no-result cause, got {message}"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "work", &[]).await,
        3,
        "the explicit thread ran before the task failed, so the failure \
         came as its last thread ended and not as its implicit thread exited"
    );
    assert_eq!(task_count(&store), 0, "the failed task left the store");
}

#[wcmp_macros::test]
async fn it_fails_a_callback_task_with_no_result_once_its_last_thread_has_run() {
    let (mut store, instance, _) = instantiate().await;

    let message = call_expecting_a_trap(&mut store, &instance, "spawn-recorder-callback").await;
    assert!(
        message.contains("async-lifted export failed to produce a result"),
        "expected the no-result cause, got {message}"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "work", &[]).await,
        4,
        "the explicit thread ran before the task failed, so the failure \
         came as its last thread ended and not at the exit code"
    );
    assert_eq!(task_count(&store), 0, "the failed task left the store");
}

#[wcmp_macros::test]
async fn it_checks_the_borrows_of_a_task_when_its_last_thread_ends() {
    let (mut store, instance, thing) = instantiate().await;
    let handle = store.resource_new(thing, 5).expect("mint an own handle");

    assert_eq!(
        call_u32(
            &mut store,
            &instance,
            "hand-borrow-on",
            &[Val::Borrow(handle)]
        )
        .await,
        11,
        "the implicit thread exited owing the borrow, and the thread that \
         dropped it and returned was the task's last"
    );
    assert_eq!(task_count(&store), 0);
}

#[wcmp_macros::test]
async fn it_keeps_a_task_and_its_thread_index_while_a_thread_it_made_has_not_run() {
    let (mut store, instance, _) = instantiate().await;

    let first = call_u32(&mut store, &instance, "leave-suspended", &[]).await;
    let second = call_u32(&mut store, &instance, "leave-suspended", &[]).await;
    assert_ne!(
        first, second,
        "the first call's thread still holds its index, so the second \
         call's thread takes another"
    );
    assert_eq!(
        task_count(&store),
        2,
        "each task stays in the store while a thread of it has not ended"
    );
}
