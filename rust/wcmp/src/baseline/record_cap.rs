// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Baseline tests for the cap on the store's live records.
//!
//! The store counts its tasks, subtasks, threads, waitable sets, and
//! the shared records of streams and futures against a cap, which is 1,000,000 records unless the
//! store's internal API sets another. A record that would take the
//! count past the cap is not created, and its creation fails with
//! Wasmtime's full-table message. A cap set below the count removes
//! nothing. The subtask of a call into the host counts only once the
//! call's first poll leaves it pending, as Wasmtime 50 makes the record
//! of a host call only then.
//!
//! The component here makes a waitable set on each call of its one
//! synchronous export and never drops it, so each call leaves one
//! more record behind than it found. The call itself is a task with
//! its implicit thread, two records that last only as long as the
//! call.

#![cfg(test)]

use crate::error::{Error, ResourceTableError};
use crate::store::{StoreContextInternalExt, StoreInternalExt};
use crate::{Component, Engine, Instance, Linker, Store, Val};
use wcmp_macros::component;

/// A component whose synchronous export makes a waitable set and
/// returns its index, leaving the set in the instance's table.
const NEW_SET: &[u8] = component!(
    r#"
    (component
      (core func $new (canon waitable-set.new))
      (core module $m
        (import "" "waitable-set.new" (func $new (result i32)))
        (func (export "new-set") (result i32) (call $new)))
      (core instance $m (instantiate $m (with "" (instance
        (export "waitable-set.new" (func $new))))))
      (func (export "new-set") (result u32) (canon lift (core func $m "new-set"))))
    "#
);

/// The message of Wasmtime's `ResourceTableError::Full`.
const TABLE_FULL: &str = "resource table has no free keys";

/// Instantiate `NEW_SET` in a fresh store with nothing registered.
async fn instantiate() -> (Store<()>, Instance) {
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, NEW_SET)
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

/// Call `new-set` and report the index it returned, or every message
/// in the chain of the error it failed with.
async fn new_set(store: &mut Store<()>, instance: &Instance) -> Result<u32, String> {
    let func = instance
        .get_func("new-set")
        .expect("the export is declared");
    match func.call(store, &[]).await {
        Ok(values) => match values.first() {
            Some(Val::U32(index)) => Ok(*index),
            other => panic!("new-set answered {other:?}"),
        },
        Err(error) => Err(chain(&error)),
    }
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

/// How many records count against the cap now.
fn record_count(store: &mut Store<()>) -> usize {
    store
        .internal()
        .lock_tables()
        .expect("handle tables")
        .tasks
        .record_count()
}

/// Set the cap through the store's internal API.
fn set_max_records(store: &mut Store<()>, max: usize) {
    store
        .internal()
        .context()
        .internal()
        .set_max_records(max)
        .expect("the cap is set");
}

#[wcmp_macros::test]
async fn it_caps_a_fresh_store_at_one_million_records() {
    let (mut store, _instance) = instantiate().await;

    let max = store
        .internal()
        .lock_tables()
        .expect("handle tables")
        .tasks
        .max_records();

    assert_eq!(max, 1_000_000, "the default capacity of Wasmtime's table");
}

#[wcmp_macros::test]
async fn it_fails_a_new_record_past_the_cap_with_the_full_table_cause() {
    let (mut store, instance) = instantiate().await;
    let live = record_count(&mut store);
    // Room for the call's task and thread, and for one set.
    set_max_records(&mut store, live + 3);

    new_set(&mut store, &instance)
        .await
        .expect("the first set fits under the cap");
    assert_eq!(
        record_count(&mut store),
        live + 1,
        "the set outlives the call"
    );
    let message = new_set(&mut store, &instance)
        .await
        .expect_err("the second set does not fit");

    assert!(
        message.contains(TABLE_FULL),
        "the guest's call traps with Wasmtime's message: {message}"
    );
    // The refused call's task and thread are gone, so the store sits
    // under the cap again until it is set to the count.
    let full = record_count(&mut store);
    set_max_records(&mut store, full);
    let refused = store
        .internal()
        .lock_tables()
        .expect("handle tables")
        .tasks
        .insert_waitable_set();
    assert!(
        matches!(refused, Err(Error::ResourceTable(ResourceTableError::Full))),
        "the cause is structured: {refused:?}"
    );
}

#[wcmp_macros::test]
async fn it_creates_no_part_of_a_task_whose_records_do_not_all_fit() {
    let (mut store, instance) = instantiate().await;
    let live = record_count(&mut store);
    // Room for the call's task but not its thread.
    set_max_records(&mut store, live + 1);

    let message = new_set(&mut store, &instance)
        .await
        .expect_err("the call's task does not fit");

    assert!(message.contains(TABLE_FULL), "{message}");
    assert_eq!(
        record_count(&mut store),
        live,
        "the refused call left no record behind"
    );
}

#[wcmp_macros::test]
async fn it_evicts_nothing_when_the_cap_drops_below_the_live_records() {
    let (mut store, instance) = instantiate().await;
    let mut sets = Vec::new();
    for _ in 0..3 {
        sets.push(new_set(&mut store, &instance).await.expect("a set"));
    }
    let live = record_count(&mut store);

    set_max_records(&mut store, 1);

    assert_eq!(
        record_count(&mut store),
        live,
        "every record stays under a cap below the count"
    );
    let refused = store
        .internal()
        .lock_tables()
        .expect("handle tables")
        .tasks
        .insert_subtask();
    assert!(
        matches!(refused, Err(Error::ResourceTable(ResourceTableError::Full))),
        "only a new record fails: {refused:?}"
    );
    let message = new_set(&mut store, &instance)
        .await
        .expect_err("a new call does not fit");
    assert!(message.contains(TABLE_FULL), "{message}");

    set_max_records(&mut store, live + 3);
    let next = new_set(&mut store, &instance)
        .await
        .expect("the store works again under a cap with room");
    assert!(
        !sets.contains(&next),
        "the sets made before the cap dropped still hold their indices: \
         {sets:?} and {next}"
    );
}

/// A component whose `run` calls the host's `async` function `answer`
/// through a synchronous lower.
const CALLS_A_HOST_ASYNC_FUNCTION: &[u8] = component!(
    r#"
    (component
      (import "answer" (func $answer async (param "x" u32) (result u32)))
      (core func $lowered (canon lower (func $answer)))
      (core module $m
        (import "" "answer" (func $answer (param i32) (result i32)))
        (func (export "run") (result i32)
          (call $answer (i32.const 7))))
      (core instance $i (instantiate $m
        (with "" (instance (export "answer" (func $lowered))))))
      (func (export "run") (result u32) (canon lift (core func $i "run"))))
    "#
);

/// A component whose `run` calls the host's `async` function `answer`
/// through an asynchronous lower, and returns the status word the
/// lower answered. The result goes to address 0 of its memory.
const CALLS_A_HOST_ASYNC_FUNCTION_ASYNC: &[u8] = component!(
    r#"
    (component
      (import "answer" (func $answer async (param "x" u32) (result u32)))
      (core module $libc (memory (export "mem") 1))
      (core instance $libc (instantiate $libc))
      (core func $lowered (canon lower (func $answer) async (memory (core memory $libc "mem"))))
      (core module $m
        (import "" "answer" (func $answer (param i32 i32) (result i32)))
        (func (export "run") (result i32)
          (call $answer (i32.const 7) (i32.const 0))))
      (core instance $i (instantiate $m
        (with "" (instance (export "answer" (func $lowered))))))
      (func (export "run") (result u32) (canon lift (core func $i "run"))))
    "#
);

/// The status word of a call that returned before its lower did.
const RETURNED: u32 = 2;

/// What the host's `answer` saw: the records that counted against the
/// cap at each poll of its body.
type Seen = std::sync::Arc<std::sync::Mutex<Vec<usize>>>;

/// A store with `component` instantiated, whose `answer` records in
/// `seen` the records that count at each poll of its body, and answers
/// its argument plus one: at once, or on its second poll where `pend`.
async fn calling_the_host(component: &[u8], seen: &Seen, pend: bool) -> (Store<()>, Instance) {
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, component)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let seen = seen.clone();
    linker
        .root()
        .func_wrap_concurrent(
            "answer",
            move |accessor: &crate::Accessor<()>, (x,): (u32,)| {
                let accessor = accessor.clone();
                let seen = seen.clone();
                async move {
                    let count = || {
                        accessor.try_with(|mut access| {
                            let store = access.store();
                            store
                                .internal()
                                .lock_tables()
                                .map(|guard| guard.tasks.record_count())
                        })
                    };
                    seen.lock().expect("seen").push(count()??);
                    if pend {
                        let mut polled = false;
                        core::future::poll_fn(|context| {
                            if polled {
                                return core::task::Poll::Ready(());
                            }
                            polled = true;
                            context.waker().wake_by_ref();
                            core::task::Poll::Pending
                        })
                        .await;
                        seen.lock().expect("seen").push(count()??);
                    }
                    Ok::<(u32,), Error>((x + 1,))
                }
            },
        )
        .expect("the registration");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    (store, instance)
}

/// Call `run` and report its answer, or every message in the chain of
/// the error it failed with.
async fn run(store: &mut Store<()>, instance: &Instance) -> Result<Box<[Val]>, String> {
    instance
        .get_func("run")
        .expect("the export is declared")
        .call(store, &[])
        .await
        .map_err(|error| chain(&error))
}

#[wcmp_macros::test]
async fn it_takes_no_record_for_a_host_call_that_answers_on_its_first_poll() {
    // Wasmtime 50 makes the record of a host call only when the call
    // outlives its first poll. While the body first runs, only the
    // caller's task and thread count.
    let seen = Seen::default();
    let (mut store, instance) = calling_the_host(CALLS_A_HOST_ASYNC_FUNCTION, &seen, false).await;
    let live = record_count(&mut store);
    let answered = run(&mut store, &instance).await.expect("the call returns");
    assert_eq!(answered.as_ref(), [Val::U32(8)]);
    let polls = core::mem::take(&mut *seen.lock().expect("seen"));
    let [during] = polls[..] else {
        panic!("the body ran once: {polls:?}");
    };
    assert_eq!(
        during,
        live + 2,
        "while the body first runs, the caller's task and thread count, and the call does not"
    );
    assert_eq!(
        record_count(&mut store),
        live,
        "the call let go of every record"
    );

    // A store with no room left past the caller's task and thread runs
    // the call, through either lower.
    for (component, answer) in [
        (CALLS_A_HOST_ASYNC_FUNCTION, 8),
        (CALLS_A_HOST_ASYNC_FUNCTION_ASYNC, RETURNED),
    ] {
        let (mut store, instance) = calling_the_host(component, &seen, false).await;
        set_max_records(&mut store, during);
        let answered = run(&mut store, &instance)
            .await
            .expect("a call that answers at once needs no room");
        assert_eq!(answered.as_ref(), [Val::U32(answer)]);
        assert_eq!(seen.lock().expect("seen").len(), 1, "the body ran");
        seen.lock().expect("seen").clear();
    }
}

#[wcmp_macros::test]
async fn it_counts_a_host_call_once_its_first_poll_leaves_it_pending() {
    let seen = Seen::default();
    let (mut store, instance) = calling_the_host(CALLS_A_HOST_ASYNC_FUNCTION, &seen, true).await;
    let live = record_count(&mut store);
    let answered = run(&mut store, &instance).await.expect("the call returns");
    assert_eq!(answered.as_ref(), [Val::U32(8)]);
    let polls = core::mem::take(&mut *seen.lock().expect("seen"));
    let [first, second] = polls[..] else {
        panic!("the body was polled twice: {polls:?}");
    };
    assert_eq!(
        first,
        live + 2,
        "the call does not count while its first poll runs"
    );
    assert_eq!(
        second,
        first + 1,
        "the pending call counts once from then on"
    );
    assert_eq!(
        record_count(&mut store),
        live,
        "the call let go of every record"
    );

    // In a store with no room for the call's record, the first poll
    // runs, and the call that stays pending fails after it, through
    // either lower.
    for component in [
        CALLS_A_HOST_ASYNC_FUNCTION,
        CALLS_A_HOST_ASYNC_FUNCTION_ASYNC,
    ] {
        let (mut store, instance) = calling_the_host(component, &seen, true).await;
        set_max_records(&mut store, first);
        let message = run(&mut store, &instance)
            .await
            .expect_err("the pending call does not fit");
        assert!(message.contains(TABLE_FULL), "{message}");
        assert_eq!(
            seen.lock().expect("seen").len(),
            1,
            "the body's first poll ran, and no later one"
        );
        seen.lock().expect("seen").clear();
    }
}

/// A component whose `run` calls the export of a second component
/// through an asynchronous lower, which prepares the call's subtask,
/// task, and thread in one step.
const CALLS_A_SECOND_COMPONENT: &[u8] = component!(
    r#"
    (component
      (component $callee
        (core module $m (func (export "f") (result i32) (i32.const 5)))
        (core instance $i (instantiate $m))
        (func (export "f") async (result u32) (canon lift (core func $i "f"))))
      (component $caller
        (import "f" (func $f async (result u32)))
        (core module $libc (memory (export "mem") 1))
        (core instance $libc (instantiate $libc))
        (core func $f (canon lower (func $f) async (memory (core memory $libc "mem"))))
        (core module $m
          (import "" "f" (func $f (param i32) (result i32)))
          (func (export "run") (result i32)
            (drop (call $f (i32.const 0)))
            (i32.const 1)))
        (core instance $i (instantiate $m (with "" (instance
          (export "f" (func $f))))))
        (func (export "run") (result u32) (canon lift (core func $i "run"))))
      (instance $a (instantiate $callee))
      (instance $b (instantiate $caller (with "f" (func $a "f"))))
      (export "run" (func $b "run")))
    "#
);

#[wcmp_macros::test]
async fn it_creates_no_part_of_a_prepared_call_whose_records_do_not_all_fit() {
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, CALLS_A_SECOND_COMPONENT)
        .await
        .expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let fresh = async || {
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");
        let instance = linker
            .instantiate(&mut store, &component)
            .await
            .expect("instantiate");
        (store, instance)
    };

    // The caller's task and thread take two records, and the prepared
    // call three more at once: its subtask, the callee's task, and the
    // callee's thread. A cap with room for one or two of the three
    // refuses the call and leaves none of them.
    let (mut store, _) = fresh().await;
    let live = record_count(&mut store);
    for room in 2..=4 {
        let (mut store, instance) = fresh().await;
        set_max_records(&mut store, live + room);
        let message = run(&mut store, &instance)
            .await
            .expect_err("the prepared call does not fit");
        assert!(message.contains(TABLE_FULL), "room for {room}: {message}");
        assert_eq!(
            record_count(&mut store),
            live,
            "room for {room}: the refused call left no record behind"
        );
    }
    let (mut store, instance) = fresh().await;
    set_max_records(&mut store, live + 5);
    let answered = run(&mut store, &instance)
        .await
        .expect("room for all five records admits the call");
    assert_eq!(answered.as_ref(), [Val::U32(1)]);
}

/// A component whose exports each create one record of their own:
/// `new-stream` a stream, `new-future` a future, and `new-thread` an
/// explicit thread, which it never resumes.
const CREATES_ONE_RECORD: &[u8] = component!(
    r#"
    (component
      (type $s (stream u32))
      (type $f (future u32))
      (core module $libc
        (table (export "__indirect_function_table") 1 funcref))
      (core instance $libc (instantiate $libc))
      (core type $start-ty (func (param i32)))
      (alias core export $libc "__indirect_function_table" (core table $table))
      (core func $stream-new (canon stream.new $s))
      (core func $future-new (canon future.new $f))
      (core func $new-indirect (canon thread.new-indirect $start-ty (core table $table)))
      (core module $m
        (import "" "stream.new" (func $stream-new (result i64)))
        (import "" "future.new" (func $future-new (result i64)))
        (import "" "thread.new-indirect" (func $new-indirect (param i32 i32) (result i32)))
        (import "libc" "__indirect_function_table" (table 1 funcref))
        (func $entry (param i32))
        (elem (i32.const 0) func $entry)
        (func (export "new-stream") (result i32)
          (drop (call $stream-new))
          (i32.const 1))
        (func (export "new-future") (result i32)
          (drop (call $future-new))
          (i32.const 1))
        (func (export "new-thread") (result i32)
          (call $new-indirect (i32.const 0) (i32.const 0))))
      (core instance $i (instantiate $m (with "" (instance
        (export "stream.new" (func $stream-new))
        (export "future.new" (func $future-new))
        (export "thread.new-indirect" (func $new-indirect))))
        (with "libc" (instance $libc))))
      (func (export "new-stream") (result u32) (canon lift (core func $i "new-stream")))
      (func (export "new-future") (result u32) (canon lift (core func $i "new-future")))
      (func (export "new-thread") (result u32) (canon lift (core func $i "new-thread"))))
    "#
);

#[wcmp_macros::test]
async fn it_refuses_a_stream_a_future_and_a_thread_past_the_cap() {
    let mut config = crate::EngineConfig::new();
    config.wasm_component_model_threading(true);
    let engine = Engine::with_backend(crate::runtime_layer::test_backend())
        .and_then(|engine| engine.with_config(&config))
        .expect("engine");
    let component = Component::new(&engine, CREATES_ONE_RECORD)
        .await
        .expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    for name in ["new-stream", "new-future", "new-thread"] {
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");
        let instance = linker
            .instantiate(&mut store, &component)
            .await
            .expect("instantiate");
        let live = record_count(&mut store);
        // Room for the call's task and thread, and no more.
        set_max_records(&mut store, live + 2);
        let message = instance
            .get_func(name)
            .expect("the export is declared")
            .call(&mut store, &[])
            .await
            .map(drop)
            .map_err(|error| chain(&error))
            .expect_err("the new record does not fit");
        assert!(message.contains(TABLE_FULL), "`{name}`: {message}");
        assert_eq!(
            record_count(&mut store),
            live,
            "`{name}` left no record behind"
        );
    }
}
