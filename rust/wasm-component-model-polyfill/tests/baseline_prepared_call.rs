//! Baseline tests for the prepare-and-start protocol of the fused
//! adapters, on its synchronous half.
//!
//! With the fused adapter compiler of Wasmtime 49, a call between two
//! components whose lower or lift is asynchronous goes through
//! `component-prepare-call` and then one of the two start
//! intrinsics. A synchronous lower of an asynchronously lifted export
//! reaches the synchronous start: the callee runs next, and the
//! caller has its flat results when the intrinsic returns.
//!
//! The tests here cover what no corpus file reaches from a repository
//! test: the lazy rule of a sync-typed caller's block, what a trap in
//! the callee leaves behind, and — in the browser, whose backend has
//! to record the `funcref` arguments of a call — what a call repeated
//! many times leaves in the store. The four combinations of lower and
//! lift are the corpus's own, in `wasmtime/async/fused.wast` and the
//! files beside it.

#![cfg(test)]

use wasm_component_model_polyfill::{
    Component, Engine, EngineConfig, Error, Instance, Linker, Store, Val,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// A callee that returns its result and exits in its first call,
/// called through a synchronous lower by a sync-typed caller. The
/// caller adds one to what the callee doubled, so the result says
/// the value crossed both ways.
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
        (core func $lowered (canon lower (func $answer)))
        (core module $m
          (import "" "answer" (func $answer (param i32) (result i32)))
          (func (export "run") (param i32) (result i32)
            (i32.add (call $answer (local.get 0)) (i32.const 1))))
        (core instance $i (instantiate $m
          (with "" (instance (export "answer" (func $lowered))))))
        (func (export "run") (param "x" u32) (result u32)
          (canon lift (core func $i "run"))))
      (instance $a (instantiate $callee))
      (instance $b (instantiate $caller (with "answer" (func $a "answer"))))
      (export "run" (func $b "run")))
    "#
);

/// The same shape with a callee that waits on a waitable set nothing
/// ever fills, so the call never resolves.
const WAITS_FOR_EVER: &[u8] = component!(
    r#"
    (component
      (component $callee
        (core func $set-new (canon waitable-set.new))
        (core module $m
          (import "" "waitable-set.new" (func $set-new (result i32)))
          (func (export "cb") (param i32 i32 i32) (result i32) unreachable)
          (func (export "answer") (param i32) (result i32)
            (i32.or (i32.shl (call $set-new) (i32.const 4)) (i32.const 2))))
        (core instance $i (instantiate $m
          (with "" (instance (export "waitable-set.new" (func $set-new))))))
        (func (export "answer") async (param "x" u32) (result u32)
          (canon lift (core func $i "answer") async (callback (core func $i "cb")))))
      (component $caller
        (import "answer" (func $answer async (param "x" u32) (result u32)))
        (core func $lowered (canon lower (func $answer)))
        (core module $m
          (import "" "answer" (func $answer (param i32) (result i32)))
          (func (export "run") (param i32) (result i32)
            (i32.add (call $answer (local.get 0)) (i32.const 1))))
        (core instance $i (instantiate $m
          (with "" (instance (export "answer" (func $lowered))))))
        (func (export "run") (param "x" u32) (result u32)
          (canon lift (core func $i "run"))))
      (instance $a (instantiate $callee))
      (instance $b (instantiate $caller (with "answer" (func $a "answer"))))
      (export "run" (func $b "run")))
    "#
);

/// The same callee that waits for ever, called by a caller whose
/// own export is lifted asynchronously with a callback. Such a
/// caller is allowed to block, so its wait does not fail with the
/// cannot-block cause: it runs nested turns until the store is idle
/// and fails with the deadlock cause instead.
const WAITS_FOR_EVER_UNDER_AN_ASYNC_CALLER: &[u8] = component!(
    r#"
    (component
      (component $callee
        (core func $set-new (canon waitable-set.new))
        (core module $m
          (import "" "waitable-set.new" (func $set-new (result i32)))
          (func (export "cb") (param i32 i32 i32) (result i32) unreachable)
          (func (export "answer") (param i32) (result i32)
            (i32.or (i32.shl (call $set-new) (i32.const 4)) (i32.const 2))))
        (core instance $i (instantiate $m
          (with "" (instance (export "waitable-set.new" (func $set-new))))))
        (func (export "answer") async (param "x" u32) (result u32)
          (canon lift (core func $i "answer") async (callback (core func $i "cb")))))
      (component $caller
        (import "answer" (func $answer async (param "x" u32) (result u32)))
        (core func $lowered (canon lower (func $answer)))
        (core func $task-return (canon task.return (result u32)))
        (core module $m
          (import "" "answer" (func $answer (param i32) (result i32)))
          (import "" "task.return" (func $task-return (param i32)))
          (func (export "cb") (param i32 i32 i32) (result i32) unreachable)
          (func (export "run") (param i32) (result i32)
            (call $task-return (i32.add (call $answer (local.get 0)) (i32.const 1)))
            (i32.const 0)))
        (core instance $i (instantiate $m
          (with "" (instance
            (export "answer" (func $lowered))
            (export "task.return" (func $task-return))))))
        (func (export "run") async (param "x" u32) (result u32)
          (canon lift (core func $i "run") async (callback (core func $i "cb")))))
      (instance $a (instantiate $callee))
      (instance $b (instantiate $caller (with "answer" (func $a "answer"))))
      (export "run" (func $b "run")))
    "#
);

/// The same shape with a callee whose first status word is the yield
/// word, so the callee parks with a callback item on the
/// low-priority queue and never resolves. The callback traps, so a
/// turn that ran the item would say so.
const YIELDS_FIRST: &[u8] = component!(
    r#"
    (component
      (component $callee
        (core module $m
          (func (export "cb") (param i32 i32 i32) (result i32) unreachable)
          (func (export "answer") (param i32) (result i32) (i32.const 1)))
        (core instance $i (instantiate $m))
        (func (export "answer") async (param "x" u32) (result u32)
          (canon lift (core func $i "answer") async (callback (core func $i "cb")))))
      (component $caller
        (import "answer" (func $answer async (param "x" u32) (result u32)))
        (core func $lowered (canon lower (func $answer)))
        (core module $m
          (import "" "answer" (func $answer (param i32) (result i32)))
          (func (export "run") (param i32) (result i32)
            (i32.add (call $answer (local.get 0)) (i32.const 1))))
        (core instance $i (instantiate $m
          (with "" (instance (export "answer" (func $lowered))))))
        (func (export "run") (param "x" u32) (result u32)
          (canon lift (core func $i "run"))))
      (instance $a (instantiate $callee))
      (instance $b (instantiate $caller (with "answer" (func $a "answer"))))
      (export "run" (func $b "run")))
    "#
);

/// The same shape again, with a callee that raises and lowers its
/// own instance's backpressure through two synchronous exports. A
/// call of `run` made while the counter is up leaves the callee's
/// start item at the entry gate: the callee never runs at all, and
/// the caller's wait fails with nothing of the callee on the stack.
/// The callee's core function traps, so a turn that ran the item
/// would say so.
const BLOCKED_AT_THE_GATE: &[u8] = component!(
    r#"
    (component
      (component $callee
        (core func $inc (canon backpressure.inc))
        (core func $dec (canon backpressure.dec))
        (core module $m
          (import "" "backpressure.inc" (func $inc))
          (import "" "backpressure.dec" (func $dec))
          (func (export "cb") (param i32 i32 i32) (result i32) unreachable)
          (func (export "answer") (param i32) (result i32) unreachable)
          (func (export "block") (call $inc))
          (func (export "unblock") (call $dec)))
        (core instance $i (instantiate $m
          (with "" (instance
            (export "backpressure.inc" (func $inc))
            (export "backpressure.dec" (func $dec))))))
        (func (export "answer") async (param "x" u32) (result u32)
          (canon lift (core func $i "answer") async (callback (core func $i "cb"))))
        (func (export "block") (canon lift (core func $i "block")))
        (func (export "unblock") (canon lift (core func $i "unblock"))))
      (component $caller
        (import "answer" (func $answer async (param "x" u32) (result u32)))
        (core func $lowered (canon lower (func $answer)))
        (core module $m
          (import "" "answer" (func $answer (param i32) (result i32)))
          (func (export "run") (param i32) (result i32)
            (i32.add (call $answer (local.get 0)) (i32.const 1))))
        (core instance $i (instantiate $m
          (with "" (instance (export "answer" (func $lowered))))))
        (func (export "run") (param "x" u32) (result u32)
          (canon lift (core func $i "run"))))
      (instance $a (instantiate $callee))
      (instance $b (instantiate $caller (with "answer" (func $a "answer"))))
      (export "run" (func $b "run"))
      (export "block" (func $a "block"))
      (export "unblock" (func $a "unblock")))
    "#
);

/// The same shape with a callee whose core function traps.
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
        (core func $lowered (canon lower (func $answer)))
        (core module $m
          (import "" "answer" (func $answer (param i32) (result i32)))
          (func (export "run") (param i32) (result i32)
            (i32.add (call $answer (local.get 0)) (i32.const 1))))
        (core instance $i (instantiate $m
          (with "" (instance (export "answer" (func $lowered))))))
        (func (export "run") (param "x" u32) (result u32)
          (canon lift (core func $i "run"))))
      (instance $a (instantiate $callee))
      (instance $b (instantiate $caller (with "answer" (func $a "answer"))))
      (export "run" (func $b "run")))
    "#
);

/// A component that declares `canon task.cancel`, which stays
/// refused: cancellation is one mechanism with two ends and one
/// design owns both.
const TASK_CANCEL: &[u8] = component!(
    r#"
    (component
      (core func $task-cancel (canon task.cancel))
      (core module $m (import "" "task.cancel" (func $task-cancel)))
      (core instance $i (instantiate $m
        (with "" (instance (export "task.cancel" (func $task-cancel)))))))
    "#
);

/// A component that declares `canon subtask.cancel`, which stays
/// refused for the same reason.
const SUBTASK_CANCEL: &[u8] = component!(
    r#"
    (component
      (core func $subtask-cancel (canon subtask.cancel))
      (core module $m (import "" "subtask.cancel" (func $subtask-cancel (param i32) (result i32))))
      (core instance $i (instantiate $m
        (with "" (instance (export "subtask.cancel" (func $subtask-cancel)))))))
    "#
);

/// A component that declares `canon thread.index`, one of the thread
/// built-ins other than `thread.yield`, which stay refused.
const THREAD_INDEX: &[u8] = component!(
    r#"
    (component
      (core func $thread-index (canon thread.index))
      (core module $m (import "" "thread.index" (func $thread-index (result i32))))
      (core instance $i (instantiate $m
        (with "" (instance (export "thread.index" (func $thread-index)))))))
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

async fn parse(bytes: &[u8]) -> Result<Component, Error> {
    let engine = Engine::new().expect("engine");
    Component::new(&engine, bytes).await
}

/// Parse with the thread built-ins allowed past validation, so that
/// the refusal a test reads is the polyfill's own and not the
/// validator's feature gate.
async fn parse_with_threading(bytes: &[u8]) -> Result<Component, Error> {
    let mut config = EngineConfig::new();
    config.wasm_component_model_threading(true);
    let engine = Engine::with_config(&config).expect("engine");
    Component::new(&engine, bytes).await
}

/// The whole message of an error and everything under it, on one
/// line.
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

/// How many core function records the browser's backend holds for
/// this store.
///
/// The browser backend records a `funcref` a host function received
/// as an argument in the store, because the JS object it arrives as
/// is not a handle the host can hold on its own. Nothing removes
/// such a record, so the count is what says whether a repeated call
/// keeps making them. The native backend has no such record, so the
/// measurement is the browser's alone.
#[cfg(target_arch = "wasm32")]
fn function_record_count(store: &mut Store<()>) -> usize {
    use wasm_runtime_layer::AsContextMut;

    store.inner_mut().as_context_mut().inner.func_count()
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

/// Take one turn of the store's scheduler and name what it achieved.
///
/// The turn is the whole of a driver's poll, without a call to give
/// it a condition, which is what makes it the way to pin what a
/// driver would find. Its answer is named rather than returned,
/// because the enumeration the store answers with is not part of the
/// crate's public surface.
fn turn(store: &mut Store<()>) -> String {
    let outcome = store
        .turn(core::task::Waker::noop())
        .expect("the turn itself does not fail");
    format!("{outcome:?}")
}

#[wcmp_macros::test]
async fn it_accepts_the_prepare_and_start_trampolines_of_a_synchronous_lower() {
    // The adapter between the two components imports the prepare and
    // the synchronous start intrinsics, and passes the two functions
    // it generated for the call as `funcref` parameters. A component
    // that translates and instantiates is the whole of the claim.
    let (_store, instance) = instantiate(RESOLVES_AT_ONCE).await;
    assert!(
        instance.get_func("run").is_some(),
        "the composed component exposes the caller's export"
    );
}

#[wcmp_macros::test]
async fn it_returns_the_callees_result_to_a_synchronous_lower() {
    // The start function lifts the caller's argument and lowers it
    // into the callee; the callee's `task.return` runs the return
    // function, which lowers the result back into the caller; and
    // the start intrinsic hands the caller its flat results.
    let (mut store, instance) = instantiate(RESOLVES_AT_ONCE).await;
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
async fn it_blocks_a_sync_typed_caller_only_after_the_callee_did_not_resolve_at_once() {
    // The rule is lazy, as the reference and Wasmtime 49 state it. A
    // sync-typed caller is not allowed to block, but the callee runs
    // before the caller ever waits, so a callee that resolves while
    // the caller's start intrinsic runs never reaches the failure.
    let (mut store, instance) = instantiate(RESOLVES_AT_ONCE).await;
    let run = instance.get_func("run").expect("the caller's export");
    let result = run
        .call(&mut store, &[Val::U32(1)])
        .await
        .expect("a callee that resolves at once does not block its caller");
    assert_eq!(result.as_ref(), &[Val::U32(3)]);

    // The same caller, with a callee that waits on a set nothing
    // fills, does have to wait, and that is what fails.
    let (mut store, instance) = instantiate(WAITS_FOR_EVER).await;
    let run = instance.get_func("run").expect("the caller's export");
    let err = run
        .call(&mut store, &[Val::U32(1)])
        .await
        .expect_err("a sync-typed caller cannot wait for its callee");
    let message = chain(&err);
    assert!(
        message.contains("cannot block a synchronous task before returning"),
        "expected the cannot-block cause, got {message}"
    );
}

#[wcmp_macros::test]
async fn it_releases_the_call_when_the_wait_fails_with_the_cannot_block_cause() {
    // The caller is sync-typed, so it may not block, and the callee
    // waits on a set nothing fills. The wait is the caller's failure
    // rather than the callee's, and it gives back what a trap in the
    // callee gives back: the subtask leaves the store, the callee's
    // task ends, and the instance that task held exclusively goes
    // back with it.
    let (mut store, instance) = instantiate(WAITS_FOR_EVER).await;
    let run = instance.get_func("run").expect("the caller's export");
    let err = run
        .call(&mut store, &[Val::U32(1)])
        .await
        .expect_err("a sync-typed caller cannot wait for its callee");
    let message = chain(&err);
    assert!(
        message.contains("cannot block a synchronous task before returning"),
        "expected the cannot-block cause, got {message}"
    );
    assert_eq!(
        task_count(&store),
        0,
        "neither the caller's task nor the callee's is left in the store"
    );
    assert_eq!(subtask_count(&store), 0, "the subtask left the store");
    assert!(
        !any_instance_is_held(&store),
        "the callee's exclusive thread is released by the failure"
    );
}

#[wcmp_macros::test]
async fn it_drops_the_callees_queued_callback_when_the_wait_fails_after_a_yield() {
    // The callee's first status word is the yield word, so it parks
    // with a callback item on the low-priority queue and the
    // sync-typed caller's wait fails. The item names a task whose
    // record the failure took out of the store, so the item goes
    // with the record: the next driver turn finds nothing to run.
    let (mut store, instance) = instantiate(YIELDS_FIRST).await;
    let run = instance.get_func("run").expect("the caller's export");
    let err = run
        .call(&mut store, &[Val::U32(1)])
        .await
        .expect_err("a sync-typed caller cannot wait for its callee");
    let message = chain(&err);
    assert!(
        message.contains("cannot block a synchronous task before returning"),
        "expected the cannot-block cause, got {message}"
    );
    assert_eq!(task_count(&store), 0, "neither task is left in the store");
    assert_eq!(subtask_count(&store), 0, "the subtask left the store");
    assert_eq!(
        store.scheduler().queued_items(),
        0,
        "the callback item the yield left went with the task's record"
    );

    // The turn the next driver takes. The callee's callback traps,
    // so a turn that ran the orphaned item would fail with that
    // trap, and a turn that ran it against the missing record would
    // fail with the polyfill's own invariant cause. It does neither:
    // it finds nothing ready and goes idle.
    assert_eq!(turn(&mut store), "Idle");
}

#[wcmp_macros::test]
async fn it_drops_the_callees_held_callback_when_the_wait_fails_during_a_wait() {
    // The callee's first status word waits on a set nothing ever
    // fills, so the scheduler holds its callback item and the
    // sync-typed caller's wait fails. The held item goes with the
    // task's record, and the wait it was holding for ends with it,
    // so the set the callee made has no waiter left.
    let (mut store, instance) = instantiate(WAITS_FOR_EVER).await;
    let run = instance.get_func("run").expect("the caller's export");
    let err = run
        .call(&mut store, &[Val::U32(1)])
        .await
        .expect_err("a sync-typed caller cannot wait for its callee");
    let message = chain(&err);
    assert!(
        message.contains("cannot block a synchronous task before returning"),
        "expected the cannot-block cause, got {message}"
    );
    assert_eq!(task_count(&store), 0, "neither task is left in the store");
    assert_eq!(
        store.scheduler().held_callbacks(),
        0,
        "the held callback item went with the task's record"
    );
    assert_eq!(
        store.scheduler().queued_items(),
        0,
        "and nothing else of the callee's is queued"
    );

    assert_eq!(turn(&mut store), "Idle");
}

#[wcmp_macros::test]
async fn it_drops_the_callees_start_item_when_the_wait_fails_at_the_entry_gate() {
    // The callee's instance has its backpressure raised, so the
    // gate holds the callee's start item and the callee never runs
    // at all. The sync-typed caller's wait fails with nothing of the
    // callee on the stack, and the start item goes with the task's
    // record — the gate's count of the tasks waiting to enter with
    // it, so the gate is not left shut against the calls that come
    // after.
    let (mut store, instance) = instantiate(BLOCKED_AT_THE_GATE).await;
    instance
        .get_func("block")
        .expect("the callee's backpressure export")
        .call(&mut store, &[])
        .await
        .expect("the callee raises its own backpressure");

    let run = instance.get_func("run").expect("the caller's export");
    let err = run
        .call(&mut store, &[Val::U32(1)])
        .await
        .expect_err("a sync-typed caller cannot wait for a callee the gate holds");
    let message = chain(&err);
    assert!(
        message.contains("cannot block a synchronous task before returning"),
        "expected the cannot-block cause, got {message}"
    );
    assert_eq!(task_count(&store), 0, "neither task is left in the store");
    assert_eq!(subtask_count(&store), 0, "the subtask left the store");
    assert_eq!(
        store.scheduler().waiting_at_gate(),
        0,
        "the start item the gate held went with the task's record"
    );
    assert_eq!(
        store.scheduler().queued_items(),
        0,
        "and nothing else of the callee's is queued"
    );

    // The gate opens again with nothing behind it. The callee's core
    // function traps, so a turn that let the orphaned start item
    // through would fail with that trap.
    instance
        .get_func("unblock")
        .expect("the callee's backpressure export")
        .call(&mut store, &[])
        .await
        .expect("the callee lowers its own backpressure");
    assert_eq!(turn(&mut store), "Idle");
}

#[wcmp_macros::test]
async fn it_releases_the_call_when_the_wait_fails_with_the_deadlock_cause() {
    // The same callee under a caller that is allowed to block. The
    // wait runs nested turns until the store is idle and then fails
    // with the deadlock cause, and the release is the one the
    // cannot-block failure makes.
    let (mut store, instance) = instantiate(WAITS_FOR_EVER_UNDER_AN_ASYNC_CALLER).await;
    let run = instance.get_func("run").expect("the caller's export");
    let err = run
        .call(&mut store, &[Val::U32(1)])
        .await
        .expect_err("the callee never resolves, so the store goes idle");
    let message = chain(&err);
    assert!(
        message.contains("cannot make further progress"),
        "expected the deadlock cause, got {message}"
    );
    assert_eq!(
        task_count(&store),
        0,
        "neither the caller's task nor the callee's is left in the store"
    );
    assert_eq!(subtask_count(&store), 0, "the subtask left the store");
    assert!(
        !any_instance_is_held(&store),
        "the callee's exclusive thread is released by the failure"
    );
}

#[wcmp_macros::test]
async fn it_fails_the_callers_call_when_the_callee_traps() {
    // The trap unwinds through the start item to the trampoline and
    // fails the caller's call with the message the synchronous
    // baseline gives the same trap. The callee's task ends with it,
    // so the instance it held exclusively goes back and nothing is
    // left in the store for the next call to wait behind.
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
    assert!(
        !any_instance_is_held(&store),
        "the callee's exclusive thread is released by the failure"
    );
    assert_eq!(task_count(&store), 0, "neither task is left in the store");
    assert_eq!(subtask_count(&store), 0, "the subtask left the store");
}

#[wcmp_macros::test]
async fn it_still_refuses_the_built_ins_this_design_does_not_own() {
    // Cancellation and the thread built-ins other than
    // `thread.yield` stay refused at translation, under the name the
    // translator gives each. The stream, future, and error-context
    // built-ins are refused the same way, which the conformance
    // corpus records file by file.
    for (bytes, name) in [
        (TASK_CANCEL, "task-cancel"),
        (SUBTASK_CANCEL, "subtask-cancel"),
    ] {
        let err = parse(bytes).await.expect_err("the built-in is refused");
        assert!(
            matches!(&err, Error::Unsupported { feature } if feature.contains(name)),
            "expected `{name}` to be refused, got {err:?}"
        );
    }
    // The thread built-ins are behind a validator gate of their own,
    // so the refusal a component that opts into them reads is the
    // polyfill's rather than the validator's.
    let err = parse_with_threading(THREAD_INDEX)
        .await
        .expect_err("the thread built-in is refused");
    assert!(
        matches!(&err, Error::Unsupported { feature } if feature.contains("thread-index")),
        "expected `thread-index` to be refused, got {err:?}"
    );
}

#[cfg(target_arch = "wasm32")]
#[wcmp_macros::test]
async fn it_records_the_functions_of_a_prepared_call_once_however_often_it_runs() {
    // Each prepared call hands the host three `funcref` arguments:
    // the two functions the adapter generated for the call, and the
    // callee's core function. Each of them arrives as a JS object,
    // which the browser backend has to record in the store before
    // the host can call it back, and a record outlives the call that
    // made it: the prepare intrinsic keeps its functions until the
    // call it prepared starts. The functions are the same three
    // objects every time, though, so the second call and every call
    // after it reuses what the first one recorded, and a store that
    // runs the same call all day holds the records of one.
    let (mut store, instance) = instantiate(RESOLVES_AT_ONCE).await;
    let run = instance.get_func("run").expect("the caller's export");
    let before_any_call = function_record_count(&mut store);

    let result = run
        .call(&mut store, &[Val::U32(1)])
        .await
        .expect("the call returns");
    assert_eq!(result.as_ref(), &[Val::U32(3)]);
    let after_one_call = function_record_count(&mut store);
    assert!(
        after_one_call > before_any_call,
        "the first call records the functions it converted, \
         so there is something for a later call to reuse \
         ({before_any_call} before, {after_one_call} after)"
    );

    for _ in 0..8 {
        let result = run
            .call(&mut store, &[Val::U32(1)])
            .await
            .expect("the call returns");
        assert_eq!(result.as_ref(), &[Val::U32(3)]);
    }
    assert_eq!(
        function_record_count(&mut store),
        after_one_call,
        "eight more calls of the same export record no further functions"
    );
}
