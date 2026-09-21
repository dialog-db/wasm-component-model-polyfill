//! Baseline tests for the prepare-and-start protocol of the fused
//! adapters, on its asynchronous half.
//!
//! An asynchronous lower of another component's export reaches the
//! asynchronous start: the callee runs next, and the caller has the
//! status word when the intrinsic returns rather than the result.
//! The four combinations of lower and lift are the corpus's own, in
//! `wasmtime/async/fused.wast` and `cm/async/cross-abi-calls.wast`.
//!
//! What the tests here cover is what no corpus file reaches from a
//! repository test: that the subtask event of the callee's
//! resolution reaches a caller which already took delivery of the
//! start, with the callee's result already in the caller's memory
//! when the callback runs. A guest callee resolves at its
//! `task.return`, and nothing else fills the subtask's event slot
//! afterwards, so this is the one observation that says the
//! resolution is recorded where the caller is waiting.

#![cfg(test)]

use wasm_component_model_polyfill::{Component, Engine, Instance, Linker, Store, Val};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// A caller that reads `STARTED`, joins the subtask to a waitable
/// set, and parks; and a callee that parks before it returns, so the
/// caller can only learn the result from the subtask event.
///
/// The callee's first call stores its argument and gives way. Its
/// callback doubles the argument and calls `task.return`, which runs
/// the return function of the prepared call and writes the result
/// into the caller's memory at the return pointer the caller passed.
/// The caller's callback then traps unless the event is the subtask
/// event carrying `RETURNED` (2), and adds one to what it finds at
/// that pointer, so the result the host reads says both that the
/// event arrived and that the memory was already written when it
/// did. It then drops the subtask and the set, which the delivered
/// resolution is what allows, so nothing of the call is left in the
/// store when the host's call returns.
const PARKS_THEN_RETURNS: &[u8] = component!(
    r#"
    (component
      (component $callee
        (core func $task-return (canon task.return (result u32)))
        (core module $m
          (import "" "task.return" (func $task-return (param i32)))
          (global $x (mut i32) (i32.const 0))
          (func (export "answer") (param i32) (result i32)
            (global.set $x (local.get 0))
            (i32.const 1))
          (func (export "cb") (param i32 i32 i32) (result i32)
            (call $task-return (i32.mul (global.get $x) (i32.const 2)))
            (i32.const 0)))
        (core instance $i (instantiate $m
          (with "" (instance (export "task.return" (func $task-return))))))
        (func (export "answer") async (param "x" u32) (result u32)
          (canon lift (core func $i "answer") async (callback (core func $i "cb")))))
      (component $caller
        (import "answer" (func $answer async (param "x" u32) (result u32)))
        (core module $libc (memory (export "mem") 1))
        (core instance $libc (instantiate $libc))
        (core func $lowered
          (canon lower (func $answer) async (memory (core memory $libc "mem"))))
        (core func $task-return (canon task.return (result u32)))
        (core func $set-new (canon waitable-set.new))
        (core func $set-drop (canon waitable-set.drop))
        (core func $join (canon waitable.join))
        (core func $subtask-drop (canon subtask.drop))
        (core module $m
          (import "" "mem" (memory 1))
          (import "" "answer" (func $answer (param i32 i32) (result i32)))
          (import "" "task.return" (func $task-return (param i32)))
          (import "" "waitable-set.new" (func $set-new (result i32)))
          (import "" "waitable-set.drop" (func $set-drop (param i32)))
          (import "" "waitable.join" (func $join (param i32 i32)))
          (import "" "subtask.drop" (func $subtask-drop (param i32)))
          (global $set (mut i32) (i32.const 0))
          (func (export "run") (param i32) (result i32)
            (local $status i32)
            (local.set $status (call $answer (local.get 0) (i32.const 8)))
            (if (i32.ne (i32.and (local.get $status) (i32.const 0xf)) (i32.const 1))
              (then unreachable))
            (global.set $set (call $set-new))
            (call $join (i32.shr_u (local.get $status) (i32.const 4)) (global.get $set))
            (i32.or (i32.shl (global.get $set) (i32.const 4)) (i32.const 2)))
          (func (export "cb") (param i32 i32 i32) (result i32)
            (if (i32.ne (local.get 0) (i32.const 1)) (then unreachable))
            (if (i32.ne (local.get 2) (i32.const 2)) (then unreachable))
            (call $join (local.get 1) (i32.const 0))
            (call $subtask-drop (local.get 1))
            (call $set-drop (global.get $set))
            (call $task-return (i32.add (i32.load (i32.const 8)) (i32.const 1)))
            (i32.const 0)))
        (core instance $i (instantiate $m (with "" (instance
          (export "mem" (memory $libc "mem"))
          (export "answer" (func $lowered))
          (export "task.return" (func $task-return))
          (export "waitable-set.new" (func $set-new))
          (export "waitable-set.drop" (func $set-drop))
          (export "waitable.join" (func $join))
          (export "subtask.drop" (func $subtask-drop))))))
        (func (export "run") async (param "x" u32) (result u32)
          (canon lift (core func $i "run") async (callback (core func $i "cb")))))
      (instance $a (instantiate $callee))
      (instance $b (instantiate $caller (with "answer" (func $a "answer"))))
      (export "run" (func $b "run")))
    "#
);

/// The same shape with a callee that returns before its core
/// function does, so the call resolves while the caller's start
/// intrinsic is still on the stack. The caller traps unless the
/// status word is `RETURNED` (2) with no index above it, and reads
/// the result out of its own memory, which the crossing wrote before
/// the lower returned.
const RESOLVES_AT_ONCE: &[u8] = component!(
    r#"
    (component
      (component $callee
        (core func $task-return (canon task.return (result u32)))
        (core module $m
          (import "" "task.return" (func $task-return (param i32)))
          (func (export "cb") (param i32 i32 i32) (result i32) unreachable)
          (func (export "answer") (param i32) (result i32)
            (call $task-return (i32.mul (local.get 0) (i32.const 2)))
            (i32.const 0)))
        (core instance $i (instantiate $m
          (with "" (instance (export "task.return" (func $task-return))))))
        (func (export "answer") async (param "x" u32) (result u32)
          (canon lift (core func $i "answer") async (callback (core func $i "cb")))))
      (component $caller
        (import "answer" (func $answer async (param "x" u32) (result u32)))
        (core module $libc (memory (export "mem") 1))
        (core instance $libc (instantiate $libc))
        (core func $lowered
          (canon lower (func $answer) async (memory (core memory $libc "mem"))))
        (core module $m
          (import "" "mem" (memory 1))
          (import "" "answer" (func $answer (param i32 i32) (result i32)))
          (func (export "run") (param i32) (result i32)
            (if (i32.ne (call $answer (local.get 0) (i32.const 8)) (i32.const 2))
              (then unreachable))
            (i32.add (i32.load (i32.const 8)) (i32.const 1))))
        (core instance $i (instantiate $m (with "" (instance
          (export "mem" (memory $libc "mem"))
          (export "answer" (func $lowered))))))
        (func (export "run") (param "x" u32) (result u32)
          (canon lift (core func $i "run"))))
      (instance $a (instantiate $callee))
      (instance $b (instantiate $caller (with "answer" (func $a "answer"))))
      (export "run" (func $b "run")))
    "#
);

/// A synchronously lifted callee under an asynchronous lower, with a
/// `post-return`. The results cross as the callee's core function
/// returns, so the caller reads `RETURNED` too, and the callee's
/// `post-return` records that it ran in the callee's own memory,
/// which the callee's second export reports.
const SYNCHRONOUS_CALLEE: &[u8] = component!(
    r#"
    (component
      (component $callee
        (core module $m
          (memory (export "mem") 1)
          (func (export "answer") (param i32) (result i32)
            (i32.mul (local.get 0) (i32.const 2)))
          (func (export "post") (param i32)
            (i32.store (i32.const 4) (local.get 0)))
          (func (export "ran") (result i32)
            (i32.load (i32.const 4))))
        (core instance $i (instantiate $m))
        (func (export "answer") async (param "x" u32) (result u32)
          (canon lift (core func $i "answer") (post-return (core func $i "post"))))
        (func (export "ran") (result u32)
          (canon lift (core func $i "ran"))))
      (component $caller
        (import "answer" (func $answer async (param "x" u32) (result u32)))
        (core module $libc (memory (export "mem") 1))
        (core instance $libc (instantiate $libc))
        (core func $lowered
          (canon lower (func $answer) async (memory (core memory $libc "mem"))))
        (core module $m
          (import "" "mem" (memory 1))
          (import "" "answer" (func $answer (param i32 i32) (result i32)))
          (func (export "run") (param i32) (result i32)
            (if (i32.ne (call $answer (local.get 0) (i32.const 8)) (i32.const 2))
              (then unreachable))
            (i32.add (i32.load (i32.const 8)) (i32.const 1))))
        (core instance $i (instantiate $m (with "" (instance
          (export "mem" (memory $libc "mem"))
          (export "answer" (func $lowered))))))
        (func (export "run") (param "x" u32) (result u32)
          (canon lift (core func $i "run"))))
      (instance $a (instantiate $callee))
      (instance $b (instantiate $caller (with "answer" (func $a "answer"))))
      (export "run" (func $b "run"))
      (export "ran" (func $a "ran")))
    "#
);

/// A callee whose core function traps, under an asynchronous lower.
/// The trap unwinds through the start item to the trampoline and
/// fails the caller's call, as it does for the synchronous start.
const TRAPS: &[u8] = component!(
    r#"
    (component
      (component $callee
        (core func $task-return (canon task.return (result u32)))
        (core module $m
          (import "" "task.return" (func $task-return (param i32)))
          (func (export "cb") (param i32 i32 i32) (result i32) unreachable)
          (func (export "answer") (param i32) (result i32) unreachable))
        (core instance $i (instantiate $m
          (with "" (instance (export "task.return" (func $task-return))))))
        (func (export "answer") async (param "x" u32) (result u32)
          (canon lift (core func $i "answer") async (callback (core func $i "cb")))))
      (component $caller
        (import "answer" (func $answer async (param "x" u32) (result u32)))
        (core module $libc (memory (export "mem") 1))
        (core instance $libc (instantiate $libc))
        (core func $lowered
          (canon lower (func $answer) async (memory (core memory $libc "mem"))))
        (core module $m
          (import "" "mem" (memory 1))
          (import "" "answer" (func $answer (param i32 i32) (result i32)))
          (func (export "run") (param i32) (result i32)
            (call $answer (local.get 0) (i32.const 8))))
        (core instance $i (instantiate $m (with "" (instance
          (export "mem" (memory $libc "mem"))
          (export "answer" (func $lowered))))))
        (func (export "run") (param "x" u32) (result u32)
          (canon lift (core func $i "run"))))
      (instance $a (instantiate $callee))
      (instance $b (instantiate $caller (with "answer" (func $a "answer"))))
      (export "run" (func $b "run")))
    "#
);

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
        .expect("instantiate");
    (store, instance)
}

/// The whole message of an error and everything under it, on one
/// line.
fn chain(error: &wasm_component_model_polyfill::Error) -> String {
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

/// How many task records the store holds.
fn task_count(store: &Store<()>) -> usize {
    store
        .tables()
        .lock()
        .expect("handle tables")
        .tasks
        .task_count()
}

/// How many subtask records the store holds.
fn subtask_count(store: &Store<()>) -> usize {
    store
        .tables()
        .lock()
        .expect("handle tables")
        .tasks
        .subtask_count()
}

/// Whether any component instance of the store is held exclusively
/// by a thread.
fn any_instance_is_held(store: &Store<()>) -> bool {
    store
        .tables()
        .lock()
        .expect("handle tables")
        .tasks
        .instances()
        .iter()
        .any(|record| record.exclusive_thread.is_some())
}

#[wcmp_macros::test]
async fn it_delivers_the_subtask_event_with_the_result_already_in_the_callers_memory() {
    // The caller took `STARTED` and went back to waiting, so the
    // resolution has to fill the subtask's event slot where it
    // happens — at the callee's `task.return` — for the caller ever
    // to hear of it. The result crosses first, at that same moment,
    // so the caller's callback finds its memory written before the
    // event reaches it: it doubles in the callee, one is added in
    // the caller's callback, and 43 says both halves happened in
    // that order.
    let (mut store, instance) = instantiate(PARKS_THEN_RETURNS).await;
    let run = instance.get_func("run").expect("the caller's export");
    let result = run
        .call(&mut store, &[Val::U32(21)])
        .await
        .expect("the call returns");
    assert_eq!(result.as_ref(), &[Val::U32(43)]);
    assert_eq!(task_count(&store), 0, "both tasks left the store");
    assert_eq!(subtask_count(&store), 0, "the subtask left the store");
    assert!(!any_instance_is_held(&store));
}

#[wcmp_macros::test]
async fn it_answers_with_the_returned_status_when_the_callee_resolves_at_once() {
    // The gate is open and the callee returns its result inside the
    // start item, so the call resolves before the lower returns: the
    // status word is `RETURNED` with no index, the caller is given
    // no entry to wait on, and the result is already at the return
    // pointer the caller passed.
    let (mut store, instance) = instantiate(RESOLVES_AT_ONCE).await;
    let run = instance.get_func("run").expect("the caller's export");
    let result = run
        .call(&mut store, &[Val::U32(21)])
        .await
        .expect("the call returns");
    assert_eq!(result.as_ref(), &[Val::U32(43)]);
    assert_eq!(subtask_count(&store), 0, "no subtask is left behind");
    assert!(!any_instance_is_held(&store));
}

#[wcmp_macros::test]
async fn it_crosses_the_results_of_a_synchronously_lifted_callee_and_runs_its_post_return() {
    // A synchronously lifted callee has no `task.return`: its
    // results cross as its core function returns, and its
    // `post-return` runs afterwards, inside its own task. The caller
    // therefore reads `RETURNED` too, and the callee's own record of
    // its `post-return` says the second half ran.
    let (mut store, instance) = instantiate(SYNCHRONOUS_CALLEE).await;
    let run = instance.get_func("run").expect("the caller's export");
    let result = run
        .call(&mut store, &[Val::U32(21)])
        .await
        .expect("the call returns");
    assert_eq!(result.as_ref(), &[Val::U32(43)]);

    let ran = instance.get_func("ran").expect("the callee's witness");
    let witness = ran.call(&mut store, &[]).await.expect("the call returns");
    assert_eq!(
        witness.as_ref(),
        &[Val::U32(42)],
        "the `post-return` ran with the flat result the core function returned"
    );
    assert_eq!(subtask_count(&store), 0, "no subtask is left behind");
    assert!(!any_instance_is_held(&store));
}

#[wcmp_macros::test]
async fn it_fails_the_callers_call_when_the_callee_traps() {
    // The trap unwinds through the start item into the trampoline,
    // which is on the stack while the switch slot runs, and fails
    // the caller's call with the message the synchronous baseline
    // gives the same trap. Nothing is left behind for the next call
    // to wait behind.
    let (mut store, instance) = instantiate(TRAPS).await;
    let run = instance.get_func("run").expect("the caller's export");
    let err = run
        .call(&mut store, &[Val::U32(1)])
        .await
        .expect_err("the callee's trap fails the caller's call");
    let message = chain(&err);
    assert!(
        message.contains("unreachable"),
        "expected the baseline's trap message, got {message}"
    );
    assert_eq!(task_count(&store), 0, "neither task is left in the store");
    assert_eq!(subtask_count(&store), 0, "the subtask left the store");
    assert!(
        !any_instance_is_held(&store),
        "the callee's exclusive thread is released by the failure"
    );
}
