// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Baseline tests for the ends of a stream or a future: `stream.new`,
//! `future.new`, and the four drop built-ins.
//!
//! A stream or future is one shared record in the store and two end
//! records, and each end sits in the creating instance's handle table
//! as an entry of its kind. The tests create the ends through the
//! guest's own built-ins and then read what the store holds: the
//! entries the two indices name, the shared record's dropped mark,
//! and how many records are left once both ends are gone.
//!
//! The states a copy moves an end through are arranged by writing the
//! end's record directly, the way the subtask baselines arrange a
//! subtask's state, so that each test reaches the state it names
//! without the copy that would lead there. The same goes for the
//! event a finished copy leaves on an end, which a waitable set
//! built-in then delivers. The copies themselves have baselines of
//! their own.

#![cfg(test)]

use crate::concurrency::{CopyState, EndId, EndKind, Event, EventCode, WaitableId};
use crate::internal::FuncInternal;
use crate::resource::{HandleKind, TableId};
use crate::store::StoreInternalExt;
use crate::{Component, Engine, Error, Func, Instance, Linker, Store, Val};
use wcmp_macros::component;

/// A component whose synchronous exports are the stream and future
/// built-ins, with the waitable set built-ins beside them so an end
/// can be joined to a set, waited on, and polled.
///
/// `new-stream` and `new-future` return the built-in's `i64` whole.
/// `new-other-stream` creates a stream of another payload type, whose
/// ends the drop built-ins declared for `stream<u8>` do not accept.
const END_BUILTINS: &[u8] = component!(
    r#"
    (component
      (core module $libc
        (memory (export "memory") 1)
        (func (export "peek") (param i32) (result i32) (i32.load (local.get 0))))
      (core instance $libc (instantiate $libc))

      (type $s (stream u8))
      (type $other (stream u32))
      (type $f (future u8))
      (core func $stream-new (canon stream.new $s))
      (core func $other-new (canon stream.new $other))
      (core func $future-new (canon future.new $f))
      (core func $stream-drop-readable (canon stream.drop-readable $s))
      (core func $stream-drop-writable (canon stream.drop-writable $s))
      (core func $future-drop-readable (canon future.drop-readable $f))
      (core func $future-drop-writable (canon future.drop-writable $f))
      (core func $set-new (canon waitable-set.new))
      (core func $set-drop (canon waitable-set.drop))
      (core func $wait (canon waitable-set.wait (memory (core memory $libc "memory"))))
      (core func $poll (canon waitable-set.poll (memory (core memory $libc "memory"))))
      (core func $join (canon waitable.join))

      (core module $m
        (import "" "stream.new" (func $stream-new (result i64)))
        (import "" "other.new" (func $other-new (result i64)))
        (import "" "future.new" (func $future-new (result i64)))
        (import "" "stream.drop-readable" (func $stream-drop-readable (param i32)))
        (import "" "stream.drop-writable" (func $stream-drop-writable (param i32)))
        (import "" "future.drop-readable" (func $future-drop-readable (param i32)))
        (import "" "future.drop-writable" (func $future-drop-writable (param i32)))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (import "" "waitable-set.drop" (func $set-drop (param i32)))
        (import "" "waitable-set.wait" (func $wait (param i32 i32) (result i32)))
        (import "" "waitable-set.poll" (func $poll (param i32 i32) (result i32)))
        (import "" "waitable.join" (func $join (param i32 i32)))
        (func (export "new-stream") (result i64) (call $stream-new))
        (func (export "new-other-stream") (result i64) (call $other-new))
        (func (export "new-future") (result i64) (call $future-new))
        (func (export "drop-stream-readable") (param i32)
          (call $stream-drop-readable (local.get 0)))
        (func (export "drop-stream-writable") (param i32)
          (call $stream-drop-writable (local.get 0)))
        (func (export "drop-future-readable") (param i32)
          (call $future-drop-readable (local.get 0)))
        (func (export "drop-future-writable") (param i32)
          (call $future-drop-writable (local.get 0)))
        (func (export "new-set") (result i32) (call $set-new))
        (func (export "drop-set") (param i32) (call $set-drop (local.get 0)))
        (func (export "wait") (param i32) (result i32)
          (call $wait (local.get 0) (i32.const 0)))
        (func (export "poll") (param i32) (result i32)
          (call $poll (local.get 0) (i32.const 0)))
        (func (export "join") (param i32 i32) (call $join (local.get 0) (local.get 1))))
      (core instance $m (instantiate $m (with "" (instance
        (export "stream.new" (func $stream-new))
        (export "other.new" (func $other-new))
        (export "future.new" (func $future-new))
        (export "stream.drop-readable" (func $stream-drop-readable))
        (export "stream.drop-writable" (func $stream-drop-writable))
        (export "future.drop-readable" (func $future-drop-readable))
        (export "future.drop-writable" (func $future-drop-writable))
        (export "waitable-set.new" (func $set-new))
        (export "waitable-set.drop" (func $set-drop))
        (export "waitable-set.wait" (func $wait))
        (export "waitable-set.poll" (func $poll))
        (export "waitable.join" (func $join))))))

      (func (export "new-stream") (result u64) (canon lift (core func $m "new-stream")))
      (func (export "new-other-stream") (result u64)
        (canon lift (core func $m "new-other-stream")))
      (func (export "new-future") (result u64) (canon lift (core func $m "new-future")))
      (func (export "drop-stream-readable") (param "e" u32)
        (canon lift (core func $m "drop-stream-readable")))
      (func (export "drop-stream-writable") (param "e" u32)
        (canon lift (core func $m "drop-stream-writable")))
      (func (export "drop-future-readable") (param "e" u32)
        (canon lift (core func $m "drop-future-readable")))
      (func (export "drop-future-writable") (param "e" u32)
        (canon lift (core func $m "drop-future-writable")))
      (func (export "new-set") (result u32) (canon lift (core func $m "new-set")))
      (func (export "drop-set") (param "s" u32) (canon lift (core func $m "drop-set")))
      (func (export "wait") (param "s" u32) (result u32) (canon lift (core func $m "wait")))
      (func (export "poll") (param "s" u32) (result u32) (canon lift (core func $m "poll")))
      (func (export "join") (param "w" u32) (param "s" u32)
        (canon lift (core func $m "join")))
      (func (export "peek") (param "p" u32) (result u32)
        (canon lift (core func $libc "peek"))))
    "#
);

/// A component whose `realloc` calls one of the six built-ins, chosen
/// by `select` beforehand. The `run` export takes a string, so the
/// host's argument lowering calls the `realloc`, and the instance's
/// may-leave flag is clear while it runs.
///
/// Mode 0 is `stream.new` and mode 1 `future.new`; modes 2 to 5 are
/// the drops of a readable and a writable stream end and a readable
/// and a writable future end. Each drop names index 1, which the
/// built-in never reaches: the may-leave check comes first.
const REALLOC_CALLS_AN_END_BUILTIN: &[u8] = component!(
    r#"
    (component
      (core module $mem (memory (export "memory") 1))
      (core instance $mem (instantiate $mem))

      (type $s (stream u8))
      (type $f (future u8))
      (core func $stream-new (canon stream.new $s))
      (core func $future-new (canon future.new $f))
      (core func $stream-drop-readable (canon stream.drop-readable $s))
      (core func $stream-drop-writable (canon stream.drop-writable $s))
      (core func $future-drop-readable (canon future.drop-readable $f))
      (core func $future-drop-writable (canon future.drop-writable $f))

      (core module $m
        (import "" "stream.new" (func $stream-new (result i64)))
        (import "" "future.new" (func $future-new (result i64)))
        (import "" "stream.drop-readable" (func $stream-drop-readable (param i32)))
        (import "" "stream.drop-writable" (func $stream-drop-writable (param i32)))
        (import "" "future.drop-readable" (func $future-drop-readable (param i32)))
        (import "" "future.drop-writable" (func $future-drop-writable (param i32)))
        (global $mode (mut i32) (i32.const 0))
        (func (export "select") (param i32) (global.set $mode (local.get 0)))
        (func (export "realloc") (param i32 i32 i32 i32) (result i32)
          (block $drops
            (block $future-writable
              (block $future-readable
                (block $stream-writable
                  (block $stream-readable
                    (block $future-new
                      (block $stream-new
                        (br_table $stream-new $future-new $stream-readable $stream-writable
                          $future-readable $future-writable $drops (global.get $mode)))
                      (drop (call $stream-new))
                      (br $drops))
                    (drop (call $future-new))
                    (br $drops))
                  (call $stream-drop-readable (i32.const 1))
                  (br $drops))
                (call $stream-drop-writable (i32.const 1))
                (br $drops))
              (call $future-drop-readable (i32.const 1))
              (br $drops))
            (call $future-drop-writable (i32.const 1)))
          (i32.const 16))
        (func (export "run") (param i32 i32)))
      (core instance $m (instantiate $m (with "" (instance
        (export "stream.new" (func $stream-new))
        (export "future.new" (func $future-new))
        (export "stream.drop-readable" (func $stream-drop-readable))
        (export "stream.drop-writable" (func $stream-drop-writable))
        (export "future.drop-readable" (func $future-drop-readable))
        (export "future.drop-writable" (func $future-drop-writable))))))

      (func (export "select") (param "m" u32) (canon lift (core func $m "select")))
      (func (export "run") (param "x" string)
        (canon lift (core func $m "run")
          (realloc (core func $m "realloc"))
          (memory (core memory $mem "memory")))))
    "#
);

/// Instantiate `binary` in a fresh store with nothing registered.
async fn instantiate(binary: &[u8]) -> (Store<()>, Instance) {
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
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
    instance
        .get_func(name)
        .unwrap_or_else(|| panic!("the component exports `{name}`"))
}

/// Call `name` with `args` and report the one value it returned, or
/// the message of the trap it raised.
async fn call(
    store: &mut Store<()>,
    instance: &Instance,
    name: &str,
    args: &[Val],
) -> Result<Option<Val>, String> {
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

/// Call `name` and expect it to succeed.
async fn call_ok(store: &mut Store<()>, instance: &Instance, name: &str, args: &[Val]) {
    if let Err(message) = call(store, instance, name, args).await {
        panic!("{name} trapped: {message}");
    }
}

/// Call `name` and expect it to trap, reporting the message.
async fn call_trap(store: &mut Store<()>, instance: &Instance, name: &str, args: &[Val]) -> String {
    match call(store, instance, name, args).await {
        Err(message) => message,
        Ok(value) => panic!("{name} returned {value:?} rather than trapping"),
    }
}

/// Call the `new` export `name` and split the `i64` it returned into
/// the readable end's index, from the low half, and the writable
/// end's, from the high half.
async fn new_ends(store: &mut Store<()>, instance: &Instance, name: &str) -> (u32, u32) {
    match call(store, instance, name, &[]).await {
        Ok(Some(Val::U64(packed))) => (packed as u32, (packed >> 32) as u32),
        other => panic!("{name} answered {other:?}"),
    }
}

/// The handle table the instance's exports resolve their indices
/// against, read the way a built-in reads it: through the canon
/// options of a declaration of the same component instance.
fn handle_table(instance: &Instance) -> TableId {
    let export = func(instance, "wait");
    let state = export.abi_state().lock().expect("the instance's ABI state");
    state.handle_tables[export.options().instance]
}

/// The entry at `index` of the instance's table.
fn entry(store: &Store<()>, instance: &Instance, index: u32) -> Option<HandleKind> {
    let table = handle_table(instance);
    store
        .internal_ref()
        .tables()
        .lock()
        .expect("handle tables")
        .entry(table, index)
}

/// The end record the entry at `index` of the instance's table names,
/// which must be an end of `kind`.
fn end_at(store: &Store<()>, instance: &Instance, index: u32, kind: EndKind) -> EndId {
    let table = handle_table(instance);
    store
        .internal_ref()
        .tables()
        .lock()
        .expect("handle tables")
        .end_from_handle(table, index, kind)
        .expect("the index names an end of the kind")
}

/// How many end records and how many shared records the store holds.
fn record_counts(store: &Store<()>) -> (usize, usize) {
    let guard = store.internal_ref().tables().lock().expect("handle tables");
    (guard.tasks.end_count(), guard.tasks.shared_record_count())
}

/// Whether the shared record of `end` is marked dropped.
fn shared_dropped(store: &Store<()>, end: EndId) -> bool {
    store
        .internal_ref()
        .tables()
        .lock()
        .expect("handle tables")
        .tasks
        .shared_record(end)
        .expect("the end's shared record")
        .dropped
}

/// Move `end` to the copy state `state`, as a copy would.
fn set_copy_state(store: &mut Store<()>, end: EndId, state: CopyState) {
    store
        .internal()
        .tables()
        .lock()
        .expect("handle tables")
        .tasks
        .end_mut(end)
        .expect("the end's record")
        .state = state;
}

#[wcmp_macros::test]
async fn it_returns_the_readable_index_low_and_the_writable_index_high() {
    let (mut store, instance) = instantiate(END_BUILTINS).await;

    let (readable, writable) = new_ends(&mut store, &instance, "new-stream").await;

    assert_eq!(
        (readable, writable),
        (1, 2),
        "a fresh instance's allocator hands out the readable end first"
    );
    assert!(
        matches!(
            entry(&store, &instance, readable),
            Some(HandleKind::StreamReadable { .. })
        ),
        "the low half names the readable end"
    );
    assert!(
        matches!(
            entry(&store, &instance, writable),
            Some(HandleKind::StreamWritable { .. })
        ),
        "the high half names the writable end"
    );

    let (readable, writable) = new_ends(&mut store, &instance, "new-future").await;

    assert_eq!(
        (readable, writable),
        (3, 4),
        "the future's ends come from the same allocator"
    );
    assert!(matches!(
        entry(&store, &instance, readable),
        Some(HandleKind::FutureReadable { .. })
    ));
    assert!(matches!(
        entry(&store, &instance, writable),
        Some(HandleKind::FutureWritable { .. })
    ));
    assert_eq!(
        record_counts(&store),
        (4, 2),
        "two end records and one shared record per stream or future"
    );
}

#[wcmp_macros::test]
async fn it_marks_the_shared_record_dropped_when_the_first_end_drops() {
    let (mut store, instance) = instantiate(END_BUILTINS).await;
    let (readable, writable) = new_ends(&mut store, &instance, "new-stream").await;
    let end = end_at(&store, &instance, writable, EndKind::StreamWritable);
    assert!(
        !shared_dropped(&store, end),
        "a fresh stream has no dropped end"
    );

    call_ok(
        &mut store,
        &instance,
        "drop-stream-readable",
        &[Val::U32(readable)],
    )
    .await;

    assert!(
        shared_dropped(&store, end),
        "the first drop marks the shared record"
    );
    assert_eq!(
        entry(&store, &instance, readable),
        None,
        "the entry is gone"
    );
    assert_eq!(
        record_counts(&store),
        (2, 1),
        "both end records and the shared record stay until the other end drops"
    );
}

#[wcmp_macros::test]
async fn it_frees_the_shared_record_and_its_index_when_the_second_end_drops() {
    let (mut store, instance) = instantiate(END_BUILTINS).await;
    let (readable, writable) = new_ends(&mut store, &instance, "new-stream").await;
    let end = end_at(&store, &instance, writable, EndKind::StreamWritable);

    call_ok(
        &mut store,
        &instance,
        "drop-stream-writable",
        &[Val::U32(writable)],
    )
    .await;
    call_ok(
        &mut store,
        &instance,
        "drop-stream-readable",
        &[Val::U32(readable)],
    )
    .await;

    assert_eq!(
        record_counts(&store),
        (0, 0),
        "the second drop takes the shared record and both ends out of the store"
    );
    assert!(
        store
            .internal_ref()
            .tables()
            .lock()
            .expect("handle tables")
            .tasks
            .end(end)
            .is_none(),
        "the ends' identities name nothing"
    );
    assert_eq!(
        new_ends(&mut store, &instance, "new-stream").await,
        (readable, writable),
        "the freed indices are handed out again, from the instance's free list"
    );
}

#[wcmp_macros::test]
async fn it_traps_a_drop_of_a_writable_future_end_that_has_not_written() {
    let (mut store, instance) = instantiate(END_BUILTINS).await;
    let (_, writable) = new_ends(&mut store, &instance, "new-future").await;

    let message = call_trap(
        &mut store,
        &instance,
        "drop-future-writable",
        &[Val::U32(writable)],
    )
    .await;

    assert!(
        message.contains("cannot drop future write end without first writing a value"),
        "a writable future end must write before it drops: {message}"
    );
    assert!(
        matches!(
            entry(&store, &instance, writable),
            Some(HandleKind::FutureWritable { .. })
        ),
        "the refused drop kept the entry"
    );
}

#[wcmp_macros::test]
async fn it_drops_a_writable_future_end_that_is_done() {
    let (mut store, instance) = instantiate(END_BUILTINS).await;
    let (_, writable) = new_ends(&mut store, &instance, "new-future").await;
    let end = end_at(&store, &instance, writable, EndKind::FutureWritable);
    set_copy_state(&mut store, end, CopyState::Done);

    call_ok(
        &mut store,
        &instance,
        "drop-future-writable",
        &[Val::U32(writable)],
    )
    .await;

    assert_eq!(entry(&store, &instance, writable), None);
}

#[wcmp_macros::test]
async fn it_traps_a_drop_of_a_busy_end_with_the_message_of_its_kind() {
    for (new, drop, side, kind, expected) in [
        (
            "new-stream",
            "drop-stream-readable",
            0,
            EndKind::StreamReadable,
            "cannot remove busy stream",
        ),
        (
            "new-stream",
            "drop-stream-writable",
            1,
            EndKind::StreamWritable,
            "cannot drop busy stream",
        ),
        (
            "new-future",
            "drop-future-readable",
            0,
            EndKind::FutureReadable,
            "cannot remove busy future",
        ),
        (
            "new-future",
            "drop-future-writable",
            1,
            EndKind::FutureWritable,
            "cannot drop busy future",
        ),
    ] {
        for state in [CopyState::Copying, CopyState::Cancelling] {
            let (mut store, instance) = instantiate(END_BUILTINS).await;
            let (readable, writable) = new_ends(&mut store, &instance, new).await;
            let index = [readable, writable][side];
            let end = end_at(&store, &instance, index, kind);
            set_copy_state(&mut store, end, state);

            let message = call_trap(&mut store, &instance, drop, &[Val::U32(index)]).await;

            assert!(
                message.contains(expected),
                "{drop} of an end in {state:?}: {message}"
            );
            assert!(
                entry(&store, &instance, index).is_some(),
                "the refused drop kept the entry"
            );
        }
    }
}

#[wcmp_macros::test]
async fn it_traps_a_drop_of_an_end_of_another_kind() {
    /// A store with a stream's two ends and a waitable set in it, and
    /// their indices: readable, writable, and the set. A trap poisons
    /// the store, so each drop below runs in a store of its own.
    async fn ends_and_a_set() -> (Store<()>, Instance, [u32; 3]) {
        let (mut store, instance) = instantiate(END_BUILTINS).await;
        let (readable, writable) = new_ends(&mut store, &instance, "new-stream").await;
        let set = call_u32(&mut store, &instance, "new-set", &[]).await;
        (store, instance, [readable, writable, set])
    }

    let (mut store, instance, [_, writable, _]) = ends_and_a_set().await;
    let message = call_trap(
        &mut store,
        &instance,
        "drop-stream-readable",
        &[Val::U32(writable)],
    )
    .await;
    assert!(
        message.contains(&format!(
            "handle index {writable} is not a readable end of a stream"
        )),
        "a writable end is not a readable one: {message}"
    );

    let (mut store, instance, [readable, _, _]) = ends_and_a_set().await;
    let message = call_trap(
        &mut store,
        &instance,
        "drop-future-readable",
        &[Val::U32(readable)],
    )
    .await;
    assert!(
        message.contains("is not a readable end of a future"),
        "a stream end is not a future end: {message}"
    );

    let (mut store, instance, [_, _, set]) = ends_and_a_set().await;
    let message = call_trap(
        &mut store,
        &instance,
        "drop-stream-writable",
        &[Val::U32(set)],
    )
    .await;
    assert!(
        message.contains("is not a writable end of a stream"),
        "a waitable set is not an end: {message}"
    );

    let (mut store, instance, _) = ends_and_a_set().await;
    let message = call_trap(
        &mut store,
        &instance,
        "drop-stream-writable",
        &[Val::U32(9)],
    )
    .await;
    assert!(
        message.contains("unknown handle index 9"),
        "an index with no entry names nothing: {message}"
    );
}

#[wcmp_macros::test]
async fn it_traps_a_drop_of_an_end_of_another_payload_type() {
    let (mut store, instance) = instantiate(END_BUILTINS).await;
    let (readable, _) = new_ends(&mut store, &instance, "new-other-stream").await;

    let message = call_trap(
        &mut store,
        &instance,
        "drop-stream-readable",
        &[Val::U32(readable)],
    )
    .await;

    assert!(
        message.contains(
            "the readable end of a stream carries another payload type than the built-in or crossing declares"
        ),
        "a `stream<u32>` end is not a `stream<u8>` one: {message}"
    );
}

#[wcmp_macros::test]
async fn it_joins_an_end_to_a_set_and_takes_it_out_when_the_end_drops() {
    let (mut store, instance) = instantiate(END_BUILTINS).await;
    let (readable, _) = new_ends(&mut store, &instance, "new-stream").await;
    let set = call_u32(&mut store, &instance, "new-set", &[]).await;
    let end = end_at(&store, &instance, readable, EndKind::StreamReadable);

    call_ok(
        &mut store,
        &instance,
        "join",
        &[Val::U32(readable), Val::U32(set)],
    )
    .await;

    {
        let guard = store.internal_ref().tables().lock().expect("handle tables");
        let set_id = guard
            .waitable_set_from_handle(handle_table(&instance), set)
            .expect("the set");
        assert_eq!(
            guard.tasks.waitable_set(set_id).expect("the set").waitables,
            vec![WaitableId::from_end(EndKind::StreamReadable, end)],
            "the end joined the set"
        );
    }
    call_ok(
        &mut store,
        &instance,
        "drop-stream-readable",
        &[Val::U32(readable)],
    )
    .await;

    call_ok(&mut store, &instance, "drop-set", &[Val::U32(set)]).await;

    // A drop of the set while the end is in it traps, and a trap
    // poisons the store, so that drop runs in a store of its own.
    let (mut store, instance) = instantiate(END_BUILTINS).await;
    let (readable, _) = new_ends(&mut store, &instance, "new-stream").await;
    let set = call_u32(&mut store, &instance, "new-set", &[]).await;
    call_ok(
        &mut store,
        &instance,
        "join",
        &[Val::U32(readable), Val::U32(set)],
    )
    .await;
    let message = call_trap(&mut store, &instance, "drop-set", &[Val::U32(set)]).await;
    assert!(
        message.contains("cannot drop waitable set with waitables in it"),
        "the set holds the end: {message}"
    );
}

#[wcmp_macros::test]
async fn it_delivers_the_event_an_end_holds_through_wait_and_poll() {
    let (mut store, instance) = instantiate(END_BUILTINS).await;
    let (readable, writable) = new_ends(&mut store, &instance, "new-stream").await;
    let set = call_u32(&mut store, &instance, "new-set", &[]).await;
    for index in [readable, writable] {
        call_ok(
            &mut store,
            &instance,
            "join",
            &[Val::U32(index), Val::U32(set)],
        )
        .await;
    }

    // A finished copy leaves the read or write event on its end,
    // carrying the end's index and the packed copy result.
    for (index, kind, code) in [
        (readable, EndKind::StreamReadable, EventCode::StreamRead),
        (writable, EndKind::StreamWritable, EventCode::StreamWrite),
    ] {
        let end = end_at(&store, &instance, index, kind);
        store
            .internal()
            .tables()
            .lock()
            .expect("handle tables")
            .tasks
            .set_pending_event(
                WaitableId::from_end(kind, end),
                Event::copy(code, index, 0x30),
            )
            .expect("the end takes the event");
    }

    assert_eq!(
        call_u32(&mut store, &instance, "wait", &[Val::U32(set)]).await,
        2,
        "the stream read event's code"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "peek", &[Val::U32(0)]).await,
        readable
    );
    assert_eq!(
        call_u32(&mut store, &instance, "peek", &[Val::U32(4)]).await,
        0x30
    );

    assert_eq!(
        call_u32(&mut store, &instance, "poll", &[Val::U32(set)]).await,
        3,
        "the stream write event's code"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "peek", &[Val::U32(0)]).await,
        writable
    );
    assert_eq!(
        call_u32(&mut store, &instance, "poll", &[Val::U32(set)]).await,
        0,
        "each event is delivered once"
    );
}

/// Call `run` with the `realloc` set to call the built-in of `mode`,
/// and report the trap it raised.
async fn realloc_trap(mode: u32) -> String {
    let (mut store, instance) = instantiate(REALLOC_CALLS_AN_END_BUILTIN).await;
    call_ok(&mut store, &instance, "select", &[Val::U32(mode)]).await;
    call_trap(
        &mut store,
        &instance,
        "run",
        &[Val::String("hi".to_owned())],
    )
    .await
}

#[wcmp_macros::test]
async fn it_fails_stream_new_from_a_realloc_with_the_cannot_leave_cause() {
    let message = realloc_trap(0).await;
    assert!(
        message.contains("cannot leave component instance"),
        "`stream.new` from a realloc: {message}"
    );
}

#[wcmp_macros::test]
async fn it_fails_future_new_from_a_realloc_with_the_cannot_leave_cause() {
    let message = realloc_trap(1).await;
    assert!(
        message.contains("cannot leave component instance"),
        "`future.new` from a realloc: {message}"
    );
}

#[wcmp_macros::test]
async fn it_fails_stream_drop_readable_from_a_realloc_with_the_cannot_leave_cause() {
    let message = realloc_trap(2).await;
    assert!(
        message.contains("cannot leave component instance"),
        "`stream.drop-readable` from a realloc: {message}"
    );
}

#[wcmp_macros::test]
async fn it_fails_stream_drop_writable_from_a_realloc_with_the_cannot_leave_cause() {
    let message = realloc_trap(3).await;
    assert!(
        message.contains("cannot leave component instance"),
        "`stream.drop-writable` from a realloc: {message}"
    );
}

#[wcmp_macros::test]
async fn it_fails_future_drop_readable_from_a_realloc_with_the_cannot_leave_cause() {
    let message = realloc_trap(4).await;
    assert!(
        message.contains("cannot leave component instance"),
        "`future.drop-readable` from a realloc: {message}"
    );
}

#[wcmp_macros::test]
async fn it_fails_future_drop_writable_from_a_realloc_with_the_cannot_leave_cause() {
    let message = realloc_trap(5).await;
    assert!(
        message.contains("cannot leave component instance"),
        "`future.drop-writable` from a realloc: {message}"
    );
}
