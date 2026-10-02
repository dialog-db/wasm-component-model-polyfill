// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Baseline tests for the `thread.yield` built-in.
//!
//! `thread.yield` gives way and returns zero. What it gives way to
//! depends on the task that called it, because the built-in asks the
//! suspend seam to suspend the thread and the seam's nested turn is
//! what runs in its place:
//!
//! - A callback task may block, so the nested turn runs every ready
//!   item the store holds. The yield of such a task therefore runs
//!   the queued work of another task before it returns.
//! - A synchronous export must return before its instance may block,
//!   so the nested turn runs the ready work of that instance alone.
//!   With none ready the yield is a no-op, and an item another
//!   instance queued stays queued.
//!
//! The nested turn is what a yield takes where its thread cannot
//! switch its stack, so these tests turn the suspend provider off.
//! Under a provider a callback task's yield suspends its thread
//! instead, and the last test measures that.
//!
//! The components here are two component instances behind one
//! import: a host `log` function both call, which records the order
//! the guests ran in. Two instances are what the exclusive thread
//! makes necessary — a callback task holds its own instance while
//! its core function runs, so the work a yield of that task can give
//! way to is another instance's. Each instance can also leave a task
//! of its own queued, which is how the tests measure that hold: the
//! item of the yielding instance's own waiting task is still unrun
//! when the yield returns.
//!
//! The waitable the second instance's task waits on is inserted
//! through the store's records, the way the feature that adds the
//! first waitable kind will produce it: a callback task that waits
//! on a set which holds no event is held until a later turn finds
//! the set filled, and the turn that finds it queues its callback
//! item.
//!
//! The queued callback of the second instance yields in its turn.
//! That yield comes from an item the nested turn of the first yield
//! is running, and nested turns nest: it opens a nested turn of its
//! own, one frame further down. Nothing is ready by then, so the
//! inner yield gives way to nothing, returns zero, and the callback
//! runs on to its second entry.

#![cfg(test)]

use std::sync::{Arc, Mutex};

#[cfg(not(target_arch = "wasm32"))]
use crate::Accessor;
use crate::concurrency::Outcome;
#[cfg(not(target_arch = "wasm32"))]
use crate::internal::AccessorInternal;
use crate::internal::FuncInternal;
use crate::store::{StoreContextInternalExt, StoreInternalExt};
use crate::{
    Component, Engine, EngineConfig, Error, Func, HostCall, Instance, Linker, Result, Store,
    SuspendProviderKind, Val,
};
use wcmp_macros::component;

/// Two component instances behind one host import.
///
/// `$yielding` exports a callback task and a synchronous function
/// that each call `thread.yield` between the log entries 1 and 3,
/// and the word the yield returned. `$queueing` exports a callback
/// task that returns its result and then waits on a waitable set of
/// its own; its callback logs 2, yields in its turn, logs 4, and
/// keeps the word that nested yield returned. `$yielding` exports
/// the same pair under `own-` names, with a callback that logs 5, so
/// that a test can leave a task of the yielding instance itself
/// queued. The host wires both to the same `log`, so the entries of
/// the two instances land in one order.
const TWO_INSTANCES: &[u8] = component!(
    r#"
    (component
      (import "log" (func $log (param "x" u32)))

      (component $yielding
        (import "log" (func $log (param "x" u32)))
        (core func $log (canon lower (func $log)))
        (core func $task-return (canon task.return (result u32)))
        (core func $yield (canon thread.yield))
        (core func $set-new (canon waitable-set.new))
        (core module $m
          (import "" "log" (func $log (param i32)))
          (import "" "task.return" (func $task-return (param i32)))
          (import "" "thread.yield" (func $yield (result i32)))
          (import "" "waitable-set.new" (func $set-new (result i32)))
          (global $word (mut i32) (i32.const -1))
          (func (export "give-way") (param i32) (result i32)
            (call $log (i32.const 1))
            (global.set $word (call $yield))
            (call $log (i32.const 3))
            (call $task-return (local.get 0))
            (i32.const 0))
          (func (export "give-way-callback") (param i32 i32 i32) (result i32) unreachable)
          (func (export "give-way-now") (result i32)
            (call $log (i32.const 1))
            (global.set $word (call $yield))
            (call $log (i32.const 3))
            (global.get $word))
          (func (export "word") (result i32) (global.get $word))
          (func (export "new-set") (result i32) (call $set-new))
          (func (export "wait-on") (param i32) (result i32)
            (call $task-return (i32.const 0))
            ;; The wait status word: the set index above the code.
            (i32.or (i32.shl (local.get 0) (i32.const 4)) (i32.const 2)))
          (func (export "wait-on-callback") (param i32 i32 i32) (result i32)
            (call $log (i32.const 5))
            (i32.const 0)))
        (core instance $i (instantiate $m (with "" (instance
          (export "log" (func $log))
          (export "task.return" (func $task-return))
          (export "thread.yield" (func $yield))
          (export "waitable-set.new" (func $set-new))))))
        (func (export "give-way") async (param "x" u32) (result u32)
          (canon lift (core func $i "give-way") async
            (callback (core func $i "give-way-callback"))))
        (func (export "give-way-now") (result u32)
          (canon lift (core func $i "give-way-now")))
        (func (export "word") (result u32) (canon lift (core func $i "word")))
        (func (export "new-set") (result u32) (canon lift (core func $i "new-set")))
        (func (export "wait-on") async (param "s" u32) (result u32)
          (canon lift (core func $i "wait-on") async
            (callback (core func $i "wait-on-callback")))))

      (component $queueing
        (import "log" (func $log (param "x" u32)))
        (core func $log (canon lower (func $log)))
        (core func $task-return (canon task.return (result u32)))
        (core func $set-new (canon waitable-set.new))
        (core func $yield (canon thread.yield))
        (core module $m
          (import "" "log" (func $log (param i32)))
          (import "" "task.return" (func $task-return (param i32)))
          (import "" "waitable-set.new" (func $set-new (result i32)))
          (import "" "thread.yield" (func $yield (result i32)))
          (global $word (mut i32) (i32.const -1))
          (func (export "new-set") (result i32) (call $set-new))
          (func (export "wait-on") (param i32) (result i32)
            (call $task-return (i32.const 0))
            ;; The wait status word: the set index above the code.
            (i32.or (i32.shl (local.get 0) (i32.const 4)) (i32.const 2)))
          (func (export "wait-on-callback") (param i32 i32 i32) (result i32)
            (call $log (i32.const 2))
            (global.set $word (call $yield))
            (call $log (i32.const 4))
            (i32.const 0))
          (func (export "word") (result i32) (global.get $word)))
        (core instance $i (instantiate $m (with "" (instance
          (export "log" (func $log))
          (export "task.return" (func $task-return))
          (export "waitable-set.new" (func $set-new))
          (export "thread.yield" (func $yield))))))
        (func (export "new-set") (result u32) (canon lift (core func $i "new-set")))
        (func (export "word") (result u32) (canon lift (core func $i "word")))
        (func (export "wait-on") async (param "s" u32) (result u32)
          (canon lift (core func $i "wait-on") async
            (callback (core func $i "wait-on-callback")))))

      (instance $a (instantiate $yielding (with "log" (func $log))))
      (instance $b (instantiate $queueing (with "log" (func $log))))
      (export "give-way" (func $a "give-way"))
      (export "give-way-now" (func $a "give-way-now"))
      (export "word" (func $a "word"))
      (export "own-new-set" (func $a "new-set"))
      (export "own-wait-on" (func $a "wait-on"))
      (export "new-set" (func $b "new-set"))
      (export "wait-on" (func $b "wait-on"))
      (export "nested-word" (func $b "word")))
    "#
);

/// A component whose `realloc` calls `thread.yield`. The `run`
/// export takes a string, so the host's argument lowering calls the
/// `realloc`, and the instance's may-leave flag is clear while it
/// runs.
const REALLOC_CALLS_YIELD: &[u8] = component!(
    r#"
    (component
      (core module $mem (memory (export "memory") 1))
      (core instance $mem (instantiate $mem))

      (core func $yield (canon thread.yield))

      (core module $m
        (import "" "thread.yield" (func $yield (result i32)))
        (func (export "realloc") (param i32 i32 i32 i32) (result i32)
          (drop (call $yield))
          (i32.const 16))
        (func (export "run") (param i32 i32)))
      (core instance $m (instantiate $m (with "" (instance
        (export "thread.yield" (func $yield))))))

      (func (export "run") (param "x" string)
        (canon lift (core func $m "run")
          (realloc (core func $m "realloc"))
          (memory (core memory $mem "memory")))))
    "#
);

/// A component whose `post-return` calls `thread.yield`. The
/// `post-return` runs once the export's result is in hand, and the
/// instance's may-leave flag is clear while it runs.
const POST_RETURN_CALLS_YIELD: &[u8] = component!(
    r#"
    (component
      (core func $yield (canon thread.yield))

      (core module $m
        (import "" "thread.yield" (func $yield (result i32)))
        (func (export "run") (result i32) (i32.const 7))
        (func (export "post-return") (param i32)
          (drop (call $yield))))
      (core instance $m (instantiate $m (with "" (instance
        (export "thread.yield" (func $yield))))))

      (func (export "run") (result u32)
        (canon lift (core func $m "run")
          (post-return (core func $m "post-return")))))
    "#
);

/// What the guests logged, in the order they logged it.
type Log = Arc<Mutex<Vec<u32>>>;

/// An engine with the suspend provider turned off. The tests here
/// measure the nested turn a yield takes where its thread cannot
/// switch its stack; under a provider a yield suspends the thread
/// instead, which the last test here measures.
fn fallback_engine() -> Engine {
    let mut config = EngineConfig::new();
    config.suspend_provider(false);
    Engine::with_backend(crate::runtime_layer::test_backend())
        .and_then(|engine| engine.with_config(&config))
        .expect("engine")
}

/// Instantiate `binary` in a fresh store with the host `log`
/// function registered, and hand back the list it appends to.
async fn instantiate(binary: &[u8]) -> (Store<()>, Instance, Log) {
    instantiate_on(fallback_engine(), binary).await
}

/// Instantiate `binary` as [`instantiate`] does, into a store of
/// `engine`.
async fn instantiate_on(engine: Engine, binary: &[u8]) -> (Store<()>, Instance, Log) {
    let component = Component::new(&engine, binary)
        .await
        .expect("component parses");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let recorded = log.clone();
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap(
            "log",
            move |_: HostCall<'_, ()>, (entry,): (u32,)| -> Result<()> {
                recorded.lock().expect("log").push(entry);
                Ok(())
            },
        )
        .expect("the registration");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    (store, instance, log)
}

/// Instantiate `binary` in a fresh store with nothing registered.
async fn instantiate_bare(binary: &[u8]) -> (Store<()>, Instance) {
    let engine = fallback_engine();
    let component = Component::new(&engine, binary)
        .await
        .expect("component parses");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let linker: Linker<()> = Linker::new(&engine);
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    (store, instance)
}

/// One export of the instance, by name.
fn func(instance: &Instance, name: &str) -> Func {
    instance.get_func(name).expect("the export is declared")
}

/// Call `name` with `args` and report the one value it returned, or
/// the message of the trap it raised.
async fn call(
    store: &mut Store<()>,
    instance: &Instance,
    name: &str,
    args: &[Val],
) -> std::result::Result<Option<Val>, String> {
    func(instance, name)
        .call(store, args)
        .await
        .map(|values| values.first().cloned())
        .map_err(|error| chain(&error))
}

/// Every message in an error's source chain, joined so that a trap a
/// built-in raised can be matched wherever the substrate put it.
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

/// Call `name` and expect it to return one `u32`.
async fn call_u32(store: &mut Store<()>, instance: &Instance, name: &str, args: &[Val]) -> u32 {
    match call(store, instance, name, args).await {
        Ok(Some(Val::U32(value))) => value,
        other => panic!("{name} answered {other:?}"),
    }
}

/// Call `name` and expect it to trap, reporting the message.
async fn call_expecting_a_trap(
    store: &mut Store<()>,
    instance: &Instance,
    name: &str,
    args: &[Val],
) -> String {
    match call(store, instance, name, args).await {
        Err(message) => message,
        Ok(value) => panic!("{name} returned {value:?} rather than trapping"),
    }
}

/// What the guests have logged so far.
fn entries(log: &Log) -> Vec<u32> {
    log.lock().expect("log").clone()
}

/// The handle table an instance's exports resolve their indices
/// against, read the way a built-in reads it: through the canon
/// options of the declaration `name` was lifted from, which belongs
/// to the component instance that owns the table.
///
/// The identity has no name outside the crate, so the lookup is a
/// macro rather than a function: every use of it binds the value and
/// hands it straight back to the store's records.
macro_rules! handle_table {
    ($instance:expr, $name:expr) => {{
        let export = func($instance, $name);
        let state = export.abi_state().lock().expect("the instance's ABI state");
        state.handle_tables[export.options().instance]
    }};
}

/// Put a returned subtask in the table `new_set` resolves against,
/// joined to the set `set_index` names and holding its ready event.
///
/// This is what fills the set the waiting task is held on: nothing
/// in these components starts a subtask, so the test inserts one
/// through the records.
fn ready_subtask_in_set(store: &mut Store<()>, instance: &Instance, new_set: &str, set_index: u32) {
    let table = handle_table!(instance, new_set);
    let mut guard = store.internal().tables().lock().expect("handle tables");
    let set = guard
        .waitable_set_from_handle(table, set_index)
        .expect("the guest's index names the set it created");
    let subtask = guard
        .tasks
        .insert_subtask()
        .expect("room under the record cap");
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
}

/// Leave the callback task `wait_on` starts queued and ready: it
/// waits on a set of its own, and the set is then filled, so the
/// next turn of any driver queues its callback item.
async fn queue_a_waiting_task(
    store: &mut Store<()>,
    instance: &Instance,
    new_set: &str,
    wait_on: &str,
) {
    let set_index = call_u32(store, instance, new_set, &[]).await;
    call(store, instance, wait_on, &[Val::U32(set_index)])
        .await
        .expect("the task returns its result and waits on its set");
    ready_subtask_in_set(store, instance, new_set, set_index);
}

/// Queue the second instance's waiting task, the one whose callback
/// logs 2, yields, and logs 4.
async fn queue_the_other_instance(store: &mut Store<()>, instance: &Instance, log: &Log) {
    queue_a_waiting_task(store, instance, "new-set", "wait-on").await;
    assert!(
        entries(log).is_empty(),
        "the waiting task has logged nothing yet"
    );
}

/// Queue the yielding instance's own waiting task, the one whose
/// callback logs 5.
async fn queue_the_yielding_instance(store: &mut Store<()>, instance: &Instance, log: &Log) {
    queue_a_waiting_task(store, instance, "own-new-set", "own-wait-on").await;
    assert!(
        entries(log).is_empty(),
        "the waiting task has logged nothing yet"
    );
}

#[wcmp_macros::test]
async fn it_runs_a_queued_item_of_another_task_from_a_callback_task() {
    let (mut store, instance, log) = instantiate(TWO_INSTANCES).await;
    queue_the_other_instance(&mut store, &instance, &log).await;

    let answer = call_u32(&mut store, &instance, "give-way", &[Val::U32(7)]).await;

    assert_eq!(answer, 7, "the callback task returned its own result");
    assert_eq!(
        entries(&log),
        vec![1, 2, 4, 3],
        "the queued item of the other task ran between the two entries the \
         yielding task wrote, so the yield ran it before it returned"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "word", &[]).await,
        0,
        "the yield returned zero"
    );
}

#[wcmp_macros::test]
async fn it_returns_zero_from_a_yield_inside_a_nested_turn() {
    let (mut store, instance, log) = instantiate(TWO_INSTANCES).await;
    queue_the_other_instance(&mut store, &instance, &log).await;

    let answer = call_u32(&mut store, &instance, "give-way", &[Val::U32(7)]).await;

    // The other instance's callback is an item of the nested turn
    // the outer yield ran, so its own yield opens a nested turn one
    // frame further down. Nothing is ready there, so it gives way to
    // nothing and returns zero: the callback logged 4 after it, and
    // the task that yielded first ran on afterwards.
    assert_eq!(
        entries(&log),
        vec![1, 2, 4, 3],
        "the nested callback ran past its own yield"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "nested-word", &[]).await,
        0,
        "the yield of the item the nested turn ran returned zero"
    );
    assert_eq!(answer, 7, "the outer callback task returned its own result");
}

#[wcmp_macros::test]
async fn it_leaves_a_queued_item_of_its_own_instance_unrun_from_a_callback_task() {
    let (mut store, instance, log) = instantiate(TWO_INSTANCES).await;
    queue_the_yielding_instance(&mut store, &instance, &log).await;

    let answer = call_u32(&mut store, &instance, "give-way", &[Val::U32(7)]).await;

    assert_eq!(answer, 7, "the callback task returned its own result");
    // The yielding task holds its own instance exclusively while its
    // core function runs, so the nested turn of the yield can only
    // defer the queued callback of that same instance.
    assert_eq!(
        entries(&log),
        vec![1, 3],
        "the queued item of the yielding instance's own task did not run \
         between the two entries the yielding task wrote"
    );

    // It runs once the task has let the instance go, which is as the
    // task ends — and a turn hands what it released there to the
    // turn that follows, by which time this call had its result. So
    // the item is work the call left in the store, and the next
    // driver runs it.
    assert_eq!(
        call_u32(&mut store, &instance, "word", &[]).await,
        0,
        "the yield returned zero"
    );
    assert_eq!(
        entries(&log),
        vec![1, 3, 5],
        "the queued item ran in the turn of the driver that came next"
    );
}

#[wcmp_macros::test]
async fn it_returns_zero_at_once_from_a_synchronous_export_with_nothing_ready() {
    let (mut store, instance, log) = instantiate(TWO_INSTANCES).await;

    let word = call_u32(&mut store, &instance, "give-way-now", &[]).await;

    assert_eq!(word, 0, "the yield returned zero");
    assert_eq!(
        entries(&log),
        vec![1, 3],
        "nothing ran between the two entries the yielding export wrote"
    );
}

#[wcmp_macros::test]
async fn it_leaves_a_queued_item_of_another_instance_unrun_from_a_synchronous_export() {
    let (mut store, instance, log) = instantiate(TWO_INSTANCES).await;
    queue_the_other_instance(&mut store, &instance, &log).await;

    let word = call_u32(&mut store, &instance, "give-way-now", &[]).await;

    assert_eq!(word, 0, "the yield returned zero");
    assert_eq!(
        entries(&log),
        vec![1, 3, 2, 4],
        "a synchronous export gives way only to the ready work of its own \
         instance, so the other instance's item ran after the call rather \
         than inside the yield"
    );
}

#[wcmp_macros::test]
async fn it_fails_a_yield_a_realloc_calls_with_the_cannot_leave_cause() {
    let (mut store, instance) = instantiate_bare(REALLOC_CALLS_YIELD).await;

    // Lowering the string argument calls the `realloc`, and the
    // instance may not be left while it runs.
    let message = call_expecting_a_trap(
        &mut store,
        &instance,
        "run",
        &[Val::String("hi".to_owned())],
    )
    .await;

    assert!(
        message.contains("cannot leave component instance"),
        "thread.yield from a realloc must fail with the cannot-leave cause: {message}"
    );
}

#[wcmp_macros::test]
async fn it_fails_a_yield_a_post_return_calls_with_the_cannot_leave_cause() {
    let (mut store, instance) = instantiate_bare(POST_RETURN_CALLS_YIELD).await;

    // The export's result is lifted first, and the `post-return`
    // then runs with the instance's may-leave flag clear.
    let message = call_expecting_a_trap(&mut store, &instance, "run", &[]).await;

    assert!(
        message.contains("cannot leave component instance"),
        "thread.yield from a post-return must fail with the cannot-leave cause: {message}"
    );
}

/// Two components, the second calling the first, where the callee's
/// core function gives way `n` times and then returns 7.
///
/// The callee runs from inside the caller's frame: the caller's task
/// is a scope below the callee's for the length of the call, which
/// is the shape the suspend seam's budget is about. A callee with
/// `n` inside the budget is being served and must see zero from
/// every one of its yields. A callee that never stops asking must
/// see the call fail with the stack-switch cause instead of running
/// for ever, because the one thread that could release it is the
/// caller whose frame the polyfill cannot leave without a stack
/// switch. The yield itself fails in neither case: what fails is
/// the call the spinning thread is inside.
const CALLEE_GIVES_WAY: &[u8] = component!(
    r#"
    (component
      (component $callee
        (core func $yield (canon thread.yield))
        (core module $m
          (import "" "thread.yield" (func $yield (result i32)))
          (func (export "spin") (param $n i32) (result i32)
            (local $i i32)
            (block $done
              (loop $again
                (br_if $done (i32.ge_u (local.get $i) (local.get $n)))
                (drop (call $yield))
                (local.set $i (i32.add (local.get $i) (i32.const 1)))
                (br $again)))
            (i32.const 7)))
        (core instance $i (instantiate $m (with "" (instance
          (export "thread.yield" (func $yield))))))
        (func (export "spin") (param "n" u32) (result u32)
          (canon lift (core func $i "spin"))))

      (component $caller
        (import "spin" (func $spin (param "n" u32) (result u32)))
        (core func $spin' (canon lower (func $spin)))
        (core module $m
          (import "" "spin" (func $spin (param i32) (result i32)))
          (func (export "run") (param i32) (result i32)
            (call $spin (local.get 0))))
        (core instance $i (instantiate $m (with "" (instance
          (export "spin" (func $spin'))))))
        (func (export "run") (param "n" u32) (result u32)
          (canon lift (core func $i "run"))))

      (instance $callee (instantiate $callee))
      (instance $caller (instantiate $caller (with "spin" (func $callee "spin"))))
      (export "run" (func $caller "run")))
    "#
);

#[wcmp_macros::test]
async fn it_returns_zero_from_every_yield_of_a_callee_that_gives_way_and_then_returns() {
    let (mut store, instance) = instantiate_bare(CALLEE_GIVES_WAY).await;

    // Eight times over, with the caller's frame below and the store
    // holding nothing: each one gives way to nothing and returns
    // zero, and the call finishes.
    let answer = call_u32(&mut store, &instance, "run", &[Val::U32(8)]).await;

    assert_eq!(
        answer, 7,
        "a callee that gives way and then returns is being served, not \
         spin-waiting for its caller, so every one of its yields returns zero"
    );
}

#[wcmp_macros::test]
async fn it_fails_the_call_of_a_callee_that_gives_way_for_ever_with_the_stack_switch_cause() {
    let (mut store, instance) = instantiate_bare(CALLEE_GIVES_WAY).await;

    // More times than any budget: the call cannot finish, so what
    // the test measures is that it stops rather than what it
    // answers.
    let message = call_expecting_a_trap(&mut store, &instance, "run", &[Val::U32(u32::MAX)]).await;

    assert!(
        message.contains("blocking here requires a stack switch"),
        "a callee that never stops giving way against a store that holds \
         nothing can only be released by the caller whose frame is below it, \
         so the seam gives up past its budget and the call fails: {message}"
    );
}

/// One component whose export gives way `n` times and then returns
/// 7, called from the host with no guest frame below it.
///
/// The seam's budget reads the store and not the stack, so a host
/// call that never stops giving way reaches the same failure as a
/// callee that spin-waits for its caller. Nothing short of running
/// the guest to its end tells the two apart, which is what makes
/// the bound a budget and not a proof.
const HOST_CALL_GIVES_WAY: &[u8] = component!(
    r#"
    (component
      (core func $yield (canon thread.yield))
      (core module $m
        (import "" "thread.yield" (func $yield (result i32)))
        (func (export "spin") (param $n i32) (result i32)
          (local $i i32)
          (block $done
            (loop $again
              (br_if $done (i32.ge_u (local.get $i) (local.get $n)))
              (drop (call $yield))
              (local.set $i (i32.add (local.get $i) (i32.const 1)))
              (br $again)))
          (i32.const 7)))
      (core instance $i (instantiate $m (with "" (instance
        (export "thread.yield" (func $yield))))))
      (func (export "spin") (param "n" u32) (result u32)
        (canon lift (core func $i "spin"))))
    "#
);

#[wcmp_macros::test]
async fn it_returns_zero_from_every_yield_of_a_host_call_that_gives_way_and_then_returns() {
    let (mut store, instance) = instantiate_bare(HOST_CALL_GIVES_WAY).await;

    let answer = call_u32(&mut store, &instance, "spin", &[Val::U32(8)]).await;

    assert_eq!(
        answer, 7,
        "a guest loop that gives way inside the budget is served whether or \
         not a guest frame is below it"
    );
}

#[wcmp_macros::test]
async fn it_fails_a_host_call_that_gives_way_for_ever_with_the_stack_switch_cause() {
    let (mut store, instance) = instantiate_bare(HOST_CALL_GIVES_WAY).await;

    let message = call_expecting_a_trap(&mut store, &instance, "spin", &[Val::U32(u32::MAX)]).await;

    assert!(
        message.contains("blocking here requires a stack switch"),
        "the seam counts the turns the store did not serve and nothing else, \
         so a guest that gives way for ever against a store that holds \
         nothing reaches the budget's failure here too: {message}"
    );
}

/// One component instance that both gives way and leaves a callback
/// item of its own queued.
///
/// `give-way-now` is a synchronous export: it logs 1, gives way, and
/// logs 3. A synchronous export must return before its instance may
/// block, so the nested turn its yield opens runs the ready work of
/// that instance and nothing else — and a queued callback of the
/// same instance is exactly that work. The task a synchronous export
/// runs takes no exclusive hold of its instance, so the callback
/// item finds the instance free and runs.
///
/// `wait-on` returns its result and waits on a set of its own; its
/// callback logs 5, gives way in its turn, and logs 6. The two
/// exports give way through the one `thread.yield` the component
/// declares, so the callback's yield is a second call of the very
/// built-in the outer yield is inside.
const QUEUES_A_CALLBACK_OF_ITS_OWN: &[u8] = component!(
    r#"
    (component
      (import "log" (func $log (param "x" u32)))
      (core func $log (canon lower (func $log)))
      (core func $task-return (canon task.return (result u32)))
      (core func $yield (canon thread.yield))
      (core func $set-new (canon waitable-set.new))
      (core module $m
        (import "" "log" (func $log (param i32)))
        (import "" "task.return" (func $task-return (param i32)))
        (import "" "thread.yield" (func $yield (result i32)))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (global $word (mut i32) (i32.const -1))
        (func (export "give-way-now") (result i32)
          (call $log (i32.const 1))
          (global.set $word (call $yield))
          (call $log (i32.const 3))
          (global.get $word))
        (func (export "word") (result i32) (global.get $word))
        (func (export "new-set") (result i32) (call $set-new))
        (func (export "wait-on") (param i32) (result i32)
          (call $task-return (i32.const 0))
          ;; The wait status word: the set index above the code.
          (i32.or (i32.shl (local.get 0) (i32.const 4)) (i32.const 2)))
        (func (export "wait-on-callback") (param i32 i32 i32) (result i32)
          (call $log (i32.const 5))
          (drop (call $yield))
          (call $log (i32.const 6))
          (i32.const 0)))
      (core instance $i (instantiate $m (with "" (instance
        (export "log" (func $log))
        (export "task.return" (func $task-return))
        (export "thread.yield" (func $yield))
        (export "waitable-set.new" (func $set-new))))))
      (func (export "give-way-now") (result u32)
        (canon lift (core func $i "give-way-now")))
      (func (export "word") (result u32) (canon lift (core func $i "word")))
      (func (export "new-set") (result u32) (canon lift (core func $i "new-set")))
      (func (export "wait-on") async (param "s" u32) (result u32)
        (canon lift (core func $i "wait-on") async
          (callback (core func $i "wait-on-callback")))))
    "#
);

/// The entry at which the `log` of [`instantiate_with_a_refusal`]
/// reaches for its store from where no poll of it is running.
#[cfg(not(target_arch = "wasm32"))]
const REFUSED_AT: u32 = 5;

/// Instantiate `binary` with a `log` that reaches for its store
/// through an accessor when it is given [`REFUSED_AT`], and hand
/// back the list it appends to.
///
/// The reach is made from inside a synchronous host call, where no
/// poll of the store is running, so it fails with the
/// store-not-in-poll cause. That is a scheduler cause a yield never
/// asked a turn to avoid, and it belongs to the item the nested turn
/// ran rather than to the yield.
#[cfg(not(target_arch = "wasm32"))]
async fn instantiate_with_a_refusal(binary: &[u8]) -> (Store<()>, Instance, Log) {
    let engine = fallback_engine();
    let component = Component::new(&engine, binary)
        .await
        .expect("component parses");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let recorded = log.clone();
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap(
            "log",
            move |mut call: HostCall<'_, ()>, (entry,): (u32,)| -> Result<()> {
                recorded.lock().expect("log").push(entry);
                if entry != REFUSED_AT {
                    return Ok(());
                }
                let accessor: Accessor<()> = Accessor::new(call.store().internal().id());
                accessor.with(|store| *store.data())?;
                Ok(())
            },
        )
        .expect("the registration");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    (store, instance, log)
}

/// Leave the instance's own callback task queued and ready, the one
/// whose callback logs 5, gives way, and logs 6.
async fn queue_the_callback(store: &mut Store<()>, instance: &Instance, log: &Log) {
    queue_a_waiting_task(store, instance, "new-set", "wait-on").await;
    assert!(
        entries(log).is_empty(),
        "the waiting task has logged nothing yet"
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[wcmp_macros::test]
async fn it_hands_on_a_store_not_in_poll_an_item_of_its_nested_turn_raised() {
    let (mut store, instance, log) = instantiate_with_a_refusal(QUEUES_A_CALLBACK_OF_ITS_OWN).await;
    queue_the_callback(&mut store, &instance, &log).await;

    // The yield's nested turn runs the queued callback, whose first
    // host call reaches for the store from outside any poll of it.
    // That is a scheduler cause, and it is not one of the three the
    // seam raises when a wait cannot end: the yield asked for no
    // wait. It belongs to the work the turn ran, so the yield hands
    // it on rather than returning zero over it.
    //
    // What the call carries is the trap the callback took, which is
    // how the refusal reached the guest. The cause's own message
    // does not survive that rendering today, so what this measures
    // is that the failure travelled at all: the same component with
    // a `log` that refuses nothing gives way, runs the callback to
    // its end, and answers zero.
    let message = call_expecting_a_trap(&mut store, &instance, "give-way-now", &[]).await;

    assert!(
        !message.is_empty(),
        "the yield hands on the failure of the item its nested turn ran \
         rather than returning zero over it"
    );
    assert_eq!(
        entries(&log),
        vec![1, REFUSED_AT],
        "the callback logged its way in and was refused there, and the \
         yielding export never reached the entry after its yield"
    );
}

#[wcmp_macros::test]
async fn it_runs_the_callback_of_its_nested_turn_through_a_second_yield() {
    let (mut store, instance, log) = instantiate(QUEUES_A_CALLBACK_OF_ITS_OWN).await;
    queue_the_callback(&mut store, &instance, &log).await;

    let outcome = call(&mut store, &instance, "give-way-now", &[]).await;

    // The outer `thread.yield` is still on the stack when the
    // callback yields, and both backends call a host function
    // already on the stack. The callback's own yield gives way to
    // nothing, returns zero, and the callback runs to its end inside
    // the nested turn the outer yield opened.
    assert_eq!(
        outcome.expect("the call returns"),
        Some(Val::U32(0)),
        "the outer yield returned zero"
    );
    assert_eq!(
        entries(&log),
        vec![1, 5, 6, 3],
        "the queued callback of the same instance ran inside the yield, gave \
         way to nothing, and ran to its end before the yielding export logged \
         its way out"
    );
}

#[wcmp_macros::test]
async fn it_suspends_a_callback_tasks_yield_behind_another_tasks_item_under_the_provider() {
    // Under a provider the yield of the callback task suspends its
    // thread rather than running a nested turn. The other instance's
    // queued callback runs in the turn, logs 2, and yields in its turn,
    // which suspends that thread too. The first yield's resumption is
    // the older of the two, so the first task logs 3 and returns, and
    // the call's result is in before the second resumption runs. The
    // second resumption waits in the low-priority queue: as in
    // Wasmtime, a driver whose call is answered at once never reaches
    // it, so the test runs turns until the store is idle, and the
    // second callback logs 4 in one of them.
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    if engine.suspend_provider() == SuspendProviderKind::None {
        return;
    }
    let (mut store, instance, log) = instantiate_on(engine, TWO_INSTANCES).await;
    queue_the_other_instance(&mut store, &instance, &log).await;

    let answer = call_u32(&mut store, &instance, "give-way", &[Val::U32(7)]).await;

    assert_eq!(answer, 7, "the callback task returned its own result");
    assert_eq!(
        entries(&log),
        vec![1, 2, 3],
        "the other task's item ran while the yield was suspended"
    );
    // A turn that resumed a thread under the host-suspension provider
    // answers `Resuming`, and the thread runs only once the driver
    // awaits it, which the loop does as a driver would.
    loop {
        let outcome = core::future::poll_fn(|context| {
            core::task::Poll::Ready(store.internal().turn(context.waker()).expect("turn"))
        })
        .await;
        match outcome {
            Outcome::Idle => break,
            Outcome::Resuming => store.internal().context().internal().fly().await,
            _ => {}
        }
    }
    assert_eq!(
        call_u32(&mut store, &instance, "nested-word", &[]).await,
        0,
        "the other callback's yield returned zero once it resumed"
    );
    assert_eq!(entries(&log), vec![1, 2, 3, 4]);
    assert_eq!(
        store
            .internal()
            .context()
            .internal()
            .scheduler()
            .nested_turns(),
        0,
        "no yield ran a nested turn"
    );
}
