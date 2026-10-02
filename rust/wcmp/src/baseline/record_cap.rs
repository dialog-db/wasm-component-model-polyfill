// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Baseline tests for the cap on the store's live records.
//!
//! The store counts its tasks, subtasks, threads, host tasks of
//! calls, waitable sets, and the shared records of streams and
//! futures against a cap, which is 1,000,000 records unless the
//! store's internal API sets another. A record that would take the
//! count past the cap is not created, and its creation fails with
//! Wasmtime's full-table message. A cap set below the count removes
//! nothing.
//!
//! The component here makes a waitable set on each call of its one
//! synchronous export and never drops it, so each call leaves one
//! more record behind than it found. The call itself is a task with
//! its implicit thread, two records that last only as long as the
//! call.

#![cfg(test)]

use crate::error::{Error, SchedulerCause};
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
        matches!(refused, Err(Error::Scheduler(SchedulerCause::TableFull))),
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
        matches!(refused, Err(Error::Scheduler(SchedulerCause::TableFull))),
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
