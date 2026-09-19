//! Baseline tests for the export lifted `canon lift async` with a
//! callback.
//!
//! The translator accepts such an export: its callback is extracted
//! from the core instance into the instance's runtime state beside
//! the reallocs and the post-returns, and the public function type
//! says the function is `async`, under the name Wasmtime gives the
//! fact. A host acquires a typed handle to a callback export with
//! the code a synchronous export takes, because the parameters and
//! the result of the two forms are the same.
//!
//! A host call into one is a task. The call's future resolves when
//! the export calls `task.return`, and the status word the export's
//! core function returned decides what the task does next: exit ends
//! it, yield leaves a callback item on the low-priority queue, and
//! wait parks the task's implicit thread on a waitable set. A task
//! that yielded or waited outlives the call, so its callback runs in
//! the turn of whichever driver comes next, and an error it raises
//! fails that driver rather than the call.
//!
//! The stackful form of the lift, the one with no callback, is
//! refused at translation, and so is an `async` function type on an
//! import: no host function the polyfill registers can satisfy one.

#![cfg(test)]

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};

use wasm_component_model_polyfill::{
    Component, Engine, EngineConfig, Error, ExternType, ExternalName, Func, FunctionType, Instance,
    Linker, Store, Val,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// One core instance behind two exports: `answer` is lifted `async`
/// with a callback, `double` is lifted synchronously. `answer`
/// returns its result through `task.return` and exits in its first
/// call, so its callback is never reached.
const RETURNS_AT_ONCE: &[u8] = component!(
    r#"
    (component
      (core func $task-return (canon task.return (result u32)))
      (core module $m
        (import "" "task.return" (func $task-return (param i32)))
        (func (export "answer-callback") (param i32 i32 i32) (result i32) unreachable)
        (func (export "answer") (param i32) (result i32)
          (call $task-return (i32.mul (local.get 0) (i32.const 2)))
          (i32.const 0))
        (func (export "double") (param i32) (result i32)
          local.get 0 i32.const 2 i32.mul))
      (core instance $i (instantiate $m
        (with "" (instance (export "task.return" (func $task-return))))))
      (func (export "answer") async (param "x" u32) (result u32)
        (canon lift (core func $i "answer") async
          (callback (core func $i "answer-callback"))))
      (func (export "double") (param "x" u32) (result u32)
        (canon lift (core func $i "double"))))
    "#
);

/// An export that returns its result and then gives way. Its
/// callback reads the context slot the export set, records the event
/// it was given, and exits.
const YIELDS_THEN_EXITS: &[u8] = component!(
    r#"
    (component
      (core func $task-return (canon task.return (result u32)))
      (core func $context-get (canon context.get i32 0))
      (core func $context-set (canon context.set i32 0))
      (core module $m
        (import "" "task.return" (func $task-return (param i32)))
        (import "" "context.get" (func $context-get (result i32)))
        (import "" "context.set" (func $context-set (param i32)))
        (global $runs (mut i32) (i32.const 0))
        (global $code (mut i32) (i32.const -1))
        (global $slot (mut i32) (i32.const -1))
        (func (export "later") (param i32) (result i32)
          (call $context-set (i32.add (local.get 0) (i32.const 100)))
          (call $task-return (i32.add (local.get 0) (i32.const 1)))
          (i32.const 1))
        (func (export "later-callback") (param i32 i32 i32) (result i32)
          (global.set $runs (i32.add (global.get $runs) (i32.const 1)))
          (global.set $code (local.get 0))
          (global.set $slot (call $context-get))
          (i32.const 0))
        (func (export "runs") (result i32) (global.get $runs))
        (func (export "code") (result i32) (global.get $code))
        (func (export "slot") (result i32) (global.get $slot))
        (func (export "drive") (result i32) (i32.const 7)))
      (core instance $i (instantiate $m
        (with "" (instance
          (export "task.return" (func $task-return))
          (export "context.get" (func $context-get))
          (export "context.set" (func $context-set))))))
      (func (export "later") async (param "x" u32) (result u32)
        (canon lift (core func $i "later") async
          (callback (core func $i "later-callback"))))
      (func (export "runs") (result u32) (canon lift (core func $i "runs")))
      (func (export "code") (result u32) (canon lift (core func $i "code")))
      (func (export "slot") (result u32) (canon lift (core func $i "slot")))
      (func (export "drive") (result u32) (canon lift (core func $i "drive"))))
    "#
);

/// The same shape with a callback that traps, so that a task which
/// keeps running after its call returned fails the next driver.
const YIELDS_THEN_TRAPS: &[u8] = component!(
    r#"
    (component
      (core func $task-return (canon task.return (result u32)))
      (core module $m
        (import "" "task.return" (func $task-return (param i32)))
        (func (export "later") (param i32) (result i32)
          (call $task-return (i32.add (local.get 0) (i32.const 1)))
          (i32.const 1))
        (func (export "later-callback") (param i32 i32 i32) (result i32) unreachable)
        (func (export "drive") (result i32) (i32.const 7)))
      (core instance $i (instantiate $m
        (with "" (instance (export "task.return" (func $task-return))))))
      (func (export "later") async (param "x" u32) (result u32)
        (canon lift (core func $i "later") async
          (callback (core func $i "later-callback"))))
      (func (export "drive") (result u32) (canon lift (core func $i "drive"))))
    "#
);

/// An export that returns its result and then waits on a waitable
/// set of its own. Its callback records the event of each run, waits
/// once more on the same set, and exits on the second event.
const RETURNS_THEN_WAITS: &[u8] = component!(
    r#"
    (component
      (core func $task-return (canon task.return (result u32)))
      (core func $set-new (canon waitable-set.new))
      (core module $m
        (import "" "task.return" (func $task-return (param i32)))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (global $set (mut i32) (i32.const 0))
        (global $runs (mut i32) (i32.const 0))
        (global $code1 (mut i32) (i32.const -1))
        (global $first1 (mut i32) (i32.const -1))
        (global $second1 (mut i32) (i32.const -1))
        (global $code2 (mut i32) (i32.const -1))
        (global $first2 (mut i32) (i32.const -1))
        (global $second2 (mut i32) (i32.const -1))
        (func $wait-word (result i32)
          (i32.or (i32.shl (global.get $set) (i32.const 4)) (i32.const 2)))
        (func (export "awaited") (param i32) (result i32)
          (call $task-return (local.get 0))
          (global.set $set (call $set-new))
          (call $wait-word))
        (func (export "awaited-callback") (param i32 i32 i32) (result i32)
          (global.set $runs (i32.add (global.get $runs) (i32.const 1)))
          (if (i32.eq (global.get $runs) (i32.const 1))
            (then
              (global.set $code1 (local.get 0))
              (global.set $first1 (local.get 1))
              (global.set $second1 (local.get 2)))
            (else
              (global.set $code2 (local.get 0))
              (global.set $first2 (local.get 1))
              (global.set $second2 (local.get 2))))
          (if (result i32) (i32.eq (global.get $runs) (i32.const 1))
            (then (call $wait-word))
            (else (i32.const 0))))
        (func (export "set") (result i32) (global.get $set))
        (func (export "runs") (result i32) (global.get $runs))
        (func (export "code1") (result i32) (global.get $code1))
        (func (export "first1") (result i32) (global.get $first1))
        (func (export "second1") (result i32) (global.get $second1))
        (func (export "code2") (result i32) (global.get $code2))
        (func (export "first2") (result i32) (global.get $first2))
        (func (export "second2") (result i32) (global.get $second2))
        (func (export "drive") (result i32) (i32.const 7)))
      (core instance $i (instantiate $m
        (with "" (instance
          (export "task.return" (func $task-return))
          (export "waitable-set.new" (func $set-new))))))
      (func (export "awaited") async (param "x" u32) (result u32)
        (canon lift (core func $i "awaited") async
          (callback (core func $i "awaited-callback"))))
      (func (export "set") (result u32) (canon lift (core func $i "set")))
      (func (export "runs") (result u32) (canon lift (core func $i "runs")))
      (func (export "code1") (result u32) (canon lift (core func $i "code1")))
      (func (export "first1") (result u32) (canon lift (core func $i "first1")))
      (func (export "second1") (result u32) (canon lift (core func $i "second1")))
      (func (export "code2") (result u32) (canon lift (core func $i "code2")))
      (func (export "first2") (result u32) (canon lift (core func $i "first2")))
      (func (export "second2") (result u32) (canon lift (core func $i "second2")))
      (func (export "drive") (result u32) (canon lift (core func $i "drive"))))
    "#
);

/// Four exports whose status words each end the call: a wait on a
/// set that no turn ever fills, a wait whose high bits name the index
/// the caller passed rather than a set of the export's own, a code
/// the protocol does not define, and an exit with no result. None of
/// them reaches the callback.
const STATUS_WORD_TRAPS: &[u8] = component!(
    r#"
    (component
      (core func $task-return (canon task.return (result u32)))
      (core func $set-new (canon waitable-set.new))
      (core module $m
        (import "" "task.return" (func $task-return (param i32)))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (func (export "callback") (param i32 i32 i32) (result i32) unreachable)
        (func (export "stuck") (param i32) (result i32)
          (i32.or (i32.shl (call $set-new) (i32.const 4)) (i32.const 2)))
        (func (export "elsewhere") (param i32) (result i32)
          (call $task-return (local.get 0))
          (i32.or (i32.shl (local.get 0) (i32.const 4)) (i32.const 2)))
        (func (export "bogus") (param i32) (result i32)
          (call $task-return (local.get 0))
          (i32.const 3))
        (func (export "empty") (param i32) (result i32)
          (i32.const 0)))
      (core instance $i (instantiate $m
        (with "" (instance
          (export "task.return" (func $task-return))
          (export "waitable-set.new" (func $set-new))))))
      (func (export "stuck") async (param "x" u32) (result u32)
        (canon lift (core func $i "stuck") async (callback (core func $i "callback"))))
      (func (export "elsewhere") async (param "x" u32) (result u32)
        (canon lift (core func $i "elsewhere") async (callback (core func $i "callback"))))
      (func (export "bogus") async (param "x" u32) (result u32)
        (canon lift (core func $i "bogus") async (callback (core func $i "callback"))))
      (func (export "empty") async (param "x" u32) (result u32)
        (canon lift (core func $i "empty") async (callback (core func $i "callback")))))
    "#
);

/// An export lifted `async` with no callback: the stackful form.
const STACKFUL_EXPORT: &[u8] = component!(
    r#"
    (component
      (core module $m
        (func (export "answer") (param i32) unreachable))
      (core instance $i (instantiate $m))
      (func (export "answer") async (param "x" u32) (result u32)
        (canon lift (core func $i "answer") async)))
    "#
);

/// A component that imports an `async` function at the root.
const ASYNC_IMPORT: &[u8] = component!(
    r#"
    (component
      (import "answer" (func $answer async (param "x" u32) (result u32))))
    "#
);

/// A component that imports an `async` function inside an interface.
const ASYNC_IMPORT_IN_AN_INTERFACE: &[u8] = component!(
    r#"
    (component
      (import "pdd-tests:host/answers@0.1.0" (instance
        (export "answer" (func async (param "x" u32) (result u32))))))
    "#
);

/// The [`FunctionType`] of the named root-level function export.
fn export_signature(component: &Component, wire_name: &str) -> FunctionType {
    let export = component
        .exports
        .iter()
        .find(|export| match &export.name {
            ExternalName::Plain(name) => name == wire_name,
            ExternalName::Interface(id) => id.to_string() == wire_name,
        })
        .unwrap_or_else(|| panic!("export `{wire_name}` not found"));
    match &export.ty {
        ExternType::Function(ty) => ty.clone(),
        other => panic!("export `{wire_name}` is not a function: {other:?}"),
    }
}

/// Parse `bytes` with the default engine configuration.
async fn parse(bytes: &[u8]) -> Result<Component, Error> {
    let engine = Engine::new().expect("engine");
    Component::new(&engine, bytes).await
}

/// Parse `bytes` with the stackful asynchronous lift accepted by the
/// validator, so that the polyfill's own refusal is what the test
/// observes rather than the validator's.
async fn parse_with_stackful_lifts(bytes: &[u8]) -> Result<Component, Error> {
    let mut config = EngineConfig::new();
    config.wasm_component_model_async_stackful(true);
    let engine = Engine::with_config(&config).expect("engine");
    Component::new(&engine, bytes).await
}

/// Instantiate `bytes` into a fresh store.
async fn instantiate(bytes: &[u8]) -> (Store<()>, Instance) {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, bytes)
        .await
        .expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the instantiation extracts the callback");
    (store, instance)
}

/// The named export of `instance`.
fn func(instance: &Instance, name: &str) -> Func {
    instance
        .get_func(name)
        .unwrap_or_else(|| panic!("export `{name}` not found"))
}

/// Call the named export, which returns one `u32`.
async fn call_u32(store: &mut Store<()>, instance: &Instance, name: &str) -> u32 {
    match func(instance, name)
        .call(store, &[])
        .await
        .unwrap_or_else(|err| panic!("call `{name}`: {err}"))
        .first()
    {
        Some(Val::U32(value)) => *value,
        other => panic!("export `{name}` returned {other:?}"),
    }
}

/// How many task records the store holds.
fn task_count(store: &Store<()>) -> usize {
    store
        .tables()
        .lock()
        .expect("handle tables")
        .tasks
        .task_count()
}

/// Whether some thread holds the instance's record exclusively. The
/// store holds one component instance in each of these tests, so the
/// first record is that instance's.
fn instance_is_held(store: &Store<()>) -> bool {
    store
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

/// The handle table the instance's exports resolve their indices
/// against, read the way a built-in reads it: through the canon
/// options of a declaration of the same component instance.
///
/// The identity has no name outside the crate, so the lookup is a
/// macro rather than a function: every use of it binds the value and
/// hands it straight back to the store's records.
macro_rules! handle_table {
    ($instance:expr, $export:literal) => {{
        let export = func($instance, $export);
        let state = export.abi_state.lock().expect("the instance's ABI state");
        state.handle_tables[export.options.instance]
    }};
}

/// Put a returned subtask in the instance's own table, joined to the
/// set `set_index` names and holding its ready event, and report the
/// index the table gave it.
///
/// This is the test seam of the wait status word. No built-in of this
/// design starts a subtask, so the event a waiting callback receives
/// is inserted through the store's records, the way the feature that
/// adds the first waitable kind will produce it.
fn ready_subtask_in_set(store: &mut Store<()>, instance: &Instance, set_index: u32) -> u32 {
    let table = handle_table!(instance, "awaited");
    let mut guard = store.tables().lock().expect("handle tables");
    let set = guard
        .waitable_set_from_handle(table, set_index)
        .expect("the guest's index names the set it created");
    let subtask = guard.tasks.insert_subtask();
    let waitable = guard.tasks.subtask_waitable(subtask);
    let subtask_index = guard.insert_subtask(table, subtask);
    guard
        .tasks
        .join_waitable_set(waitable, Some(set))
        .expect("the subtask joins the set");
    guard
        .tasks
        .subtask_returned(subtask)
        .expect("the call the subtask names returned");
    guard
        .tasks
        .record_subtask_event(subtask, subtask_index)
        .expect("the subtask is ready");
    subtask_index
}

/// Put a subtask in the table the `elsewhere` export resolves its
/// indices against, joined to no set, and report the index the table
/// gave it: a live entry of the instance's own table that a status
/// word can name and that is not a waitable set.
fn subtask_not_in_a_set(store: &mut Store<()>, instance: &Instance) -> u32 {
    let table = handle_table!(instance, "elsewhere");
    let mut guard = store.tables().lock().expect("handle tables");
    let subtask = guard.tasks.insert_subtask();
    guard.insert_subtask(table, subtask)
}

/// Every message in an error's source chain, joined so that a trap
/// the guest raised can be matched wherever the substrate put it.
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

/// Poll `future` once, as an executor would.
fn poll_once<F: Future>(future: &mut Pin<Box<F>>, waker: &Waker) -> Poll<F::Output> {
    let mut context = Context::from_waker(waker);
    future.as_mut().poll(&mut context)
}

#[wcmp_macros::test]
async fn it_reports_async_on_a_callback_export_and_not_on_a_synchronous_one() {
    // The `async` effect of the function type reaches the public
    // shape. The two exports below come from one core instance, so
    // the flag is the only thing that tells them apart.
    let component = parse(RETURNS_AT_ONCE).await.expect("component parses");

    let asynchronous = export_signature(&component, "answer");
    assert!(
        asynchronous.async_,
        "the callback export's type is `async`: {asynchronous:?}"
    );

    let synchronous = export_signature(&component, "double");
    assert!(
        !synchronous.async_,
        "the synchronous export's type is not `async`: {synchronous:?}"
    );

    // Everything else about the two types is the same, which is why
    // the flag is the fact a host reads.
    assert_eq!(asynchronous.parameters, synchronous.parameters);
    assert_eq!(asynchronous.result, synchronous.result);
}

#[wcmp_macros::test]
async fn it_acquires_a_typed_handle_to_a_callback_export() {
    // The typed conversion compares parameters and result and
    // ignores the `async` flag, so the host writes the same call it
    // writes for a synchronous export of the same shape.
    let (_store, instance) = instantiate(RETURNS_AT_ONCE).await;

    let typed = func(&instance, "answer").typed::<(u32,), u32>();
    assert!(
        typed.is_ok(),
        "a typed handle to a callback export is acquired: {:?}",
        typed.err()
    );
}

#[wcmp_macros::test]
async fn it_resolves_a_call_whose_export_returns_and_exits_at_once() {
    let (mut store, instance) = instantiate(RETURNS_AT_ONCE).await;

    let results = func(&instance, "answer")
        .call(&mut store, &[Val::U32(21)])
        .await
        .expect("the call resolves with the result `task.return` lifted");

    assert_eq!(results.as_ref(), &[Val::U32(42)]);
    assert_eq!(
        task_count(&store),
        0,
        "the exit word ended the task's implicit thread, so its record left the store"
    );
    assert!(
        !instance_is_held(&store),
        "the instance the callback task held exclusively went back"
    );
    assert_eq!(
        store.scheduler().queued_items(),
        0,
        "the task left nothing behind"
    );

    // The typed call is the same call with no change to the host's
    // code, and the synchronous export of the same instance still
    // runs after it.
    let typed = func(&instance, "answer")
        .typed::<(u32,), u32>()
        .expect("typed handle");
    assert_eq!(typed.call(&mut store, (21,)).await.expect("typed call"), 42);

    // The synchronous export of the same instance still runs, so the
    // instance the callback task took is really back.
    let doubled = func(&instance, "double")
        .call(&mut store, &[Val::U32(21)])
        .await
        .expect("the synchronous call runs");
    assert_eq!(doubled.as_ref(), &[Val::U32(42)]);
}

#[wcmp_macros::test]
async fn it_resumes_the_callback_in_a_later_turn_after_a_yield() {
    let (mut store, instance) = instantiate(YIELDS_THEN_EXITS).await;

    let results = func(&instance, "later")
        .call(&mut store, &[Val::U32(5)])
        .await
        .expect("the call resolves at `task.return`");

    assert_eq!(
        results.as_ref(),
        &[Val::U32(6)],
        "the call resolves with the result, though the task is still running"
    );
    // The callback exits when it runs, so a task still in the store
    // is a callback that has not run. Reading the records rather
    // than calling a probe export is the point: a call would be a
    // driver, and a driver is exactly what the resumption waits for.
    assert_eq!(
        task_count(&store),
        1,
        "the yield gave way, so the task outlived the call with its callback unrun"
    );
    assert_eq!(
        store.scheduler().queued_items(),
        1,
        "one callback item waits for a driver to return control to the executor"
    );

    // The next driver of the store runs the resumption the yield
    // left: it sits in the resume-after-yield slot, which a turn
    // takes before anything else.
    assert_eq!(call_u32(&mut store, &instance, "drive").await, 7);
    assert_eq!(
        task_count(&store),
        0,
        "the callback exited, so the task's record left the store"
    );

    assert_eq!(
        call_u32(&mut store, &instance, "runs").await,
        1,
        "the callback ran once, in the turn of the driver that came next"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "code").await,
        0,
        "a yield delivers the none event"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "slot").await,
        105,
        "the context slot the export set survived into the callback"
    );
    assert!(!instance_is_held(&store));
}

#[wcmp_macros::test]
async fn it_leaves_the_task_in_the_store_when_the_calls_future_is_dropped() {
    let (mut store, instance) = instantiate(YIELDS_THEN_EXITS).await;

    {
        // The turn that ran the export ended in a yield, so the
        // driver returns pending with the callback item still
        // queued. Dropping the future there cancels nothing.
        let call = func(&instance, "later");
        let mut abandoned = Box::pin(call.call(&mut store, &[Val::U32(5)]));
        assert!(
            poll_once(&mut abandoned, Waker::noop()).is_pending(),
            "the turn yielded, so the driver returns pending"
        );
    }

    assert_eq!(task_count(&store), 1, "the task stayed in the store");
    assert_eq!(
        call_u32(&mut store, &instance, "runs").await,
        1,
        "the task's callback ran in the next turn of another driver"
    );
    assert_eq!(task_count(&store), 0);
}

#[wcmp_macros::test]
async fn it_fails_the_next_driver_when_a_resumed_callback_traps() {
    let (mut store, instance) = instantiate(YIELDS_THEN_TRAPS).await;

    let results = func(&instance, "later")
        .call(&mut store, &[Val::U32(5)])
        .await
        .expect("the call resolves at `task.return`, before the callback runs");
    assert_eq!(results.as_ref(), &[Val::U32(6)]);

    // The task kept running after its call returned, so the trap its
    // callback raises belongs to whichever driver's turn ran it.
    let err = func(&instance, "drive")
        .call(&mut store, &[])
        .await
        .expect_err("the next driver fails with the callback's trap");
    assert!(
        chain(&err).contains("unreachable"),
        "expected the callback's trap, got {}",
        chain(&err)
    );
}

#[wcmp_macros::test]
async fn it_delivers_an_event_to_a_callback_that_waited_on_a_set() {
    let (mut store, instance) = instantiate(RETURNS_THEN_WAITS).await;

    let results = func(&instance, "awaited")
        .call(&mut store, &[Val::U32(9)])
        .await
        .expect("the call resolves at `task.return`");
    assert_eq!(results.as_ref(), &[Val::U32(9)]);

    let set_index = call_u32(&mut store, &instance, "set").await;
    assert!(
        set_index > 0,
        "the set index is read from the high bits of the word, so it is not the code"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "runs").await,
        0,
        "the set held no event, so the task is parked and its callback has not run"
    );

    // Two events, delivered in the order their waitables joined the
    // set. The first ends the wait; the second is already there when
    // the callback waits again, so that wait does not park.
    let first = ready_subtask_in_set(&mut store, &instance, set_index);
    let second = ready_subtask_in_set(&mut store, &instance, set_index);

    assert_eq!(call_u32(&mut store, &instance, "drive").await, 7);

    assert_eq!(
        call_u32(&mut store, &instance, "runs").await,
        2,
        "both events reached the callback in the turn of the next driver"
    );
    assert_eq!(call_u32(&mut store, &instance, "code1").await, 1);
    assert_eq!(
        call_u32(&mut store, &instance, "first1").await,
        first,
        "the payloads are the waitable's index in the handle table and its state"
    );
    assert_eq!(call_u32(&mut store, &instance, "second1").await, 2);
    assert_eq!(call_u32(&mut store, &instance, "code2").await, 1);
    assert_eq!(call_u32(&mut store, &instance, "first2").await, second);
    assert_eq!(call_u32(&mut store, &instance, "second2").await, 2);
    assert_eq!(
        task_count(&store),
        0,
        "the second event's callback exited, so the task's record left the store"
    );
}

#[wcmp_macros::test]
async fn it_fails_a_call_that_waits_on_a_set_no_turn_ever_fills() {
    let (mut store, instance) = instantiate(STATUS_WORD_TRAPS).await;

    let err = func(&instance, "stuck")
        .call(&mut store, &[Val::U32(1)])
        .await
        .expect_err("the call fails rather than waiting for ever");

    assert!(
        chain(&err).contains("cannot make further progress"),
        "expected the deadlock cause, got {}",
        chain(&err)
    );
}

#[wcmp_macros::test]
async fn it_fails_a_wait_whose_index_does_not_name_a_set() {
    let (mut store, instance) = instantiate(STATUS_WORD_TRAPS).await;
    // A live entry of the instance's own table that is not a set, so
    // that the word names something rather than nothing.
    let index = subtask_not_in_a_set(&mut store, &instance);

    let err = func(&instance, "elsewhere")
        .call(&mut store, &[Val::U32(index)])
        .await
        .expect_err("the call fails on a wait that names no set");

    assert!(
        chain(&err).contains(&format!("handle index {index} is not a waitable-set")),
        "expected the index the high bits named to be rejected, got {}",
        chain(&err)
    );
    assert_eq!(task_count(&store), 0, "the failed task left the store");
    assert!(
        !instance_is_held(&store),
        "the failed task gave the instance back"
    );
}

#[wcmp_macros::test]
async fn it_fails_a_status_word_whose_code_is_above_two() {
    let (mut store, instance) = instantiate(STATUS_WORD_TRAPS).await;

    let err = func(&instance, "bogus")
        .call(&mut store, &[Val::U32(1)])
        .await
        .expect_err("the call fails on the undefined code");

    assert!(
        chain(&err).contains("unsupported callback code"),
        "expected the unsupported-callback-code cause, got {}",
        chain(&err)
    );
    assert_eq!(task_count(&store), 0, "the failed task left the store");
}

#[wcmp_macros::test]
async fn it_fails_an_exit_from_a_task_that_never_returned_a_result() {
    let (mut store, instance) = instantiate(STATUS_WORD_TRAPS).await;

    let err = func(&instance, "empty")
        .call(&mut store, &[Val::U32(1)])
        .await
        .expect_err("the call fails with no result to give");

    assert!(
        chain(&err).contains("async-lifted export failed to produce a result"),
        "expected the no-result cause, got {}",
        chain(&err)
    );
    assert_eq!(task_count(&store), 0, "the failed task left the store");
    assert!(!instance_is_held(&store));
}

#[wcmp_macros::test]
async fn it_refuses_a_stackful_asynchronous_lift() {
    // The stackful form has no callback, so resuming it would mean
    // suspending the export's core function mid-call. The polyfill
    // runs the guest on the one real stack and refuses the lift.
    let err = parse_with_stackful_lifts(STACKFUL_EXPORT)
        .await
        .expect_err("the stackful lift is refused");
    assert!(
        matches!(&err, Error::Unsupported { feature } if feature.contains("stackful")),
        "expected Error::Unsupported, got {err:?}"
    );
}

#[wcmp_macros::test]
async fn it_refuses_an_asynchronous_function_type_on_an_import() {
    // No host function of this design returns its result through a
    // task, so an `async` import is refused where it is listed —
    // whether the import is the function or an interface holding it.
    let err = parse(ASYNC_IMPORT)
        .await
        .expect_err("the asynchronous import is refused");
    assert!(
        matches!(&err, Error::Unsupported { feature } if feature.contains("on imports")),
        "expected Error::Unsupported, got {err:?}"
    );

    let err = parse(ASYNC_IMPORT_IN_AN_INTERFACE)
        .await
        .expect_err("the asynchronous import is refused inside an interface");
    assert!(
        matches!(&err, Error::Unsupported { feature } if feature.contains("on imports")),
        "expected Error::Unsupported, got {err:?}"
    );
}
