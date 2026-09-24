//! Baseline tests for a readable end that crosses the boundary
//! between two components.
//!
//! A `stream<T>` or `future<T>` in a parameter or a result is the
//! readable end of a stream or a future. Between two composed
//! components the fused adapter moves it with the `StreamTransfer`
//! or the `FutureTransfer` intrinsic: the entry leaves the sender's
//! handle table and a readable entry of the same kind enters the
//! receiver's, under the receiver's own index, over the same end
//! record. The writable end never moves.
//!
//! The tests compose a creator with a reader. The creator makes each
//! stream or future and hands its readable end to the reader, as a
//! parameter, or receives one the reader made, as a result: a
//! synchronous result in the flat form, a synchronous result inside
//! a tuple in the memory form, and an asynchronous result through
//! `task.return`. The states a copy moves an end through are
//! arranged by writing the end's record directly, as the baselines of
//! the ends themselves do.
//!
//! Out of a guest to the host the same crossing has no host value to
//! lift the end into yet, so it fails with `Error::Unsupported` after
//! the lift's checks. Into a guest the host passes a readable end it
//! holds, and a value that is not one fails as a host value mismatch,
//! in the flat and in the memory form.

#![cfg(test)]

use crate::concurrency::{CopyState, EndId, EndKind};
use crate::internal::FuncInternal;
use crate::resource::{HandleKind, TableId};
use crate::store::StoreInternalExt;
use crate::{AbiCause, Component, Engine, Error, Func, Instance, Linker, Store, Val};
use wcmp_macros::component;

/// A creator and a reader, composed. The outer component exports
/// every function of both, so a test calls each side directly and
/// reads each side's handle table.
///
/// The creator makes streams and futures with its own built-ins, and
/// `give-stream` and `give-future` pass the readable end at the index
/// they are given to the reader's `take-stream` and `take-future`,
/// which answer the index the end arrived under. `new-other-stream`
/// makes a `stream<u32>`, which `give-stream` passes as a
/// `stream<u8>` all the same.
///
/// The reader makes a stream of its own for each of the creator's
/// three `receive` exports. `return-stream` returns its readable end
/// flat, `return-pair` returns it inside a tuple through memory next
/// to the number 7, and `return-async` returns it through
/// `task.return` from an export lifted asynchronously. Each keeps the
/// writable end. The reader's `take-list` takes a list of streams,
/// which only the host can call it with.
const COMPOSED: &[u8] = component!(
    r#"
    (component
      (component $reader
        (type $s (stream u8))
        (type $f (future u8))
        (core func $stream-new (canon stream.new $s))
        (core func $stream-drop (canon stream.drop-readable $s))
        (core func $future-drop (canon future.drop-readable $f))
        (core func $task-return (canon task.return (result $s)))
        (core module $m
          (import "" "stream.new" (func $stream-new (result i64)))
          (import "" "stream.drop-readable" (func $stream-drop (param i32)))
          (import "" "future.drop-readable" (func $future-drop (param i32)))
          (import "" "task.return" (func $task-return (param i32)))
          (memory (export "memory") 1)
          (global $bump (mut i32) (i32.const 1024))
          (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32)
            (local $ptr i32)
            (local.set $ptr (global.get $bump))
            (global.set $bump (i32.add (local.get $ptr) (local.get 3)))
            (local.get $ptr))
          (func (export "take") (param i32) (result i32) (local.get 0))
          (func (export "take-list") (param i32 i32))
          (func (export "drop-stream") (param i32) (call $stream-drop (local.get 0)))
          (func (export "drop-future") (param i32) (call $future-drop (local.get 0)))
          (func (export "return-stream") (result i32)
            (i32.wrap_i64 (call $stream-new)))
          (func (export "return-pair") (result i32)
            (i32.store (i32.const 32) (i32.wrap_i64 (call $stream-new)))
            (i32.store (i32.const 36) (i32.const 7))
            (i32.const 32))
          (func (export "cb") (param i32 i32 i32) (result i32) unreachable)
          (func (export "return-async") (result i32)
            (call $task-return (i32.wrap_i64 (call $stream-new)))
            (i32.const 0)))
        (core instance $i (instantiate $m (with "" (instance
          (export "stream.new" (func $stream-new))
          (export "stream.drop-readable" (func $stream-drop))
          (export "future.drop-readable" (func $future-drop))
          (export "task.return" (func $task-return))))))
        (func (export "take-stream") (param "s" $s) (result u32)
          (canon lift (core func $i "take")))
        (func (export "take-future") (param "f" $f) (result u32)
          (canon lift (core func $i "take")))
        (func (export "take-list") (param "l" (list $s))
          (canon lift (core func $i "take-list")
            (memory (core memory $i "memory"))
            (realloc (core func $i "cabi_realloc"))))
        (func (export "drop-stream") (param "e" u32) (canon lift (core func $i "drop-stream")))
        (func (export "drop-future") (param "e" u32) (canon lift (core func $i "drop-future")))
        (func (export "return-stream") (result $s) (canon lift (core func $i "return-stream")))
        (func (export "return-pair") (result (tuple $s u32))
          (canon lift (core func $i "return-pair") (memory (core memory $i "memory"))))
        (func (export "return-async") async (result $s)
          (canon lift (core func $i "return-async") async (callback (core func $i "cb")))))

      (component $creator
        (type $s (stream u8))
        (type $other (stream u32))
        (type $f (future u8))
        (import "take-stream" (func $take-stream (param "s" $s) (result u32)))
        (import "take-future" (func $take-future (param "f" $f) (result u32)))
        (import "return-stream" (func $return-stream (result $s)))
        (import "return-pair" (func $return-pair (result (tuple $s u32))))
        (import "return-async" (func $return-async async (result $s)))
        (core module $libc (memory (export "memory") 1))
        (core instance $libc (instantiate $libc))
        (core func $stream-new (canon stream.new $s))
        (core func $other-new (canon stream.new $other))
        (core func $future-new (canon future.new $f))
        (core func $stream-drop-writable (canon stream.drop-writable $s))
        (core func $set-new (canon waitable-set.new))
        (core func $join (canon waitable.join))
        (core func $take-stream (canon lower (func $take-stream)))
        (core func $take-future (canon lower (func $take-future)))
        (core func $return-stream (canon lower (func $return-stream)))
        (core func $return-pair (canon lower (func $return-pair)
          (memory (core memory $libc "memory"))))
        (core func $return-async (canon lower (func $return-async)))
        (core module $m
          (import "" "memory" (memory 1))
          (import "" "stream.new" (func $stream-new (result i64)))
          (import "" "other.new" (func $other-new (result i64)))
          (import "" "future.new" (func $future-new (result i64)))
          (import "" "stream.drop-writable" (func $stream-drop-writable (param i32)))
          (import "" "waitable-set.new" (func $set-new (result i32)))
          (import "" "waitable.join" (func $join (param i32 i32)))
          (import "" "take-stream" (func $take-stream (param i32) (result i32)))
          (import "" "take-future" (func $take-future (param i32) (result i32)))
          (import "" "return-stream" (func $return-stream (result i32)))
          (import "" "return-pair" (func $return-pair (param i32)))
          (import "" "return-async" (func $return-async (result i32)))
          (func (export "new-stream") (result i64) (call $stream-new))
          (func (export "new-other-stream") (result i64) (call $other-new))
          (func (export "new-future") (result i64) (call $future-new))
          (func (export "drop-stream-writable") (param i32)
            (call $stream-drop-writable (local.get 0)))
          (func (export "new-set") (result i32) (call $set-new))
          (func (export "join") (param i32 i32) (call $join (local.get 0) (local.get 1)))
          (func (export "give-stream") (param i32) (result i32)
            (call $take-stream (local.get 0)))
          (func (export "give-future") (param i32) (result i32)
            (call $take-future (local.get 0)))
          (func (export "receive-stream") (result i32) (call $return-stream))
          (func (export "receive-pair") (result i32)
            (call $return-pair (i32.const 16))
            (if (i32.ne (i32.load (i32.const 20)) (i32.const 7)) (then unreachable))
            (i32.load (i32.const 16)))
          (func (export "receive-async") (result i32) (call $return-async)))
        (core instance $i (instantiate $m (with "" (instance
          (export "memory" (memory $libc "memory"))
          (export "stream.new" (func $stream-new))
          (export "other.new" (func $other-new))
          (export "future.new" (func $future-new))
          (export "stream.drop-writable" (func $stream-drop-writable))
          (export "waitable-set.new" (func $set-new))
          (export "waitable.join" (func $join))
          (export "take-stream" (func $take-stream))
          (export "take-future" (func $take-future))
          (export "return-stream" (func $return-stream))
          (export "return-pair" (func $return-pair))
          (export "return-async" (func $return-async))))))
        (func (export "new-stream") (result u64) (canon lift (core func $i "new-stream")))
        (func (export "new-other-stream") (result u64)
          (canon lift (core func $i "new-other-stream")))
        (func (export "new-future") (result u64) (canon lift (core func $i "new-future")))
        (func (export "drop-stream-writable") (param "e" u32)
          (canon lift (core func $i "drop-stream-writable")))
        (func (export "new-set") (result u32) (canon lift (core func $i "new-set")))
        (func (export "join") (param "w" u32) (param "s" u32) (canon lift (core func $i "join")))
        (func (export "give-stream") (param "e" u32) (result u32)
          (canon lift (core func $i "give-stream")))
        (func (export "give-future") (param "e" u32) (result u32)
          (canon lift (core func $i "give-future")))
        (func (export "receive-stream") (result u32) (canon lift (core func $i "receive-stream")))
        (func (export "receive-pair") (result u32) (canon lift (core func $i "receive-pair")))
        (func (export "receive-async") (result u32) (canon lift (core func $i "receive-async"))))

      (instance $r (instantiate $reader))
      (instance $c (instantiate $creator
        (with "take-stream" (func $r "take-stream"))
        (with "take-future" (func $r "take-future"))
        (with "return-stream" (func $r "return-stream"))
        (with "return-pair" (func $r "return-pair"))
        (with "return-async" (func $r "return-async"))))
      (export "take-stream" (func $r "take-stream"))
      (export "take-list" (func $r "take-list"))
      (export "drop-stream" (func $r "drop-stream"))
      (export "drop-future" (func $r "drop-future"))
      (export "return-pair" (func $r "return-pair"))
      (export "new-stream" (func $c "new-stream"))
      (export "new-other-stream" (func $c "new-other-stream"))
      (export "new-future" (func $c "new-future"))
      (export "drop-stream-writable" (func $c "drop-stream-writable"))
      (export "new-set" (func $c "new-set"))
      (export "join" (func $c "join"))
      (export "give-stream" (func $c "give-stream"))
      (export "give-future" (func $c "give-future"))
      (export "receive-stream" (func $c "receive-stream"))
      (export "receive-pair" (func $c "receive-pair"))
      (export "receive-async" (func $c "receive-async")))
    "#
);

/// Instantiate `binary` in a fresh store with nothing registered.
async fn instantiate(binary: &[u8]) -> (Store<()>, Instance) {
    let engine = Engine::new().expect("engine");
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
/// the error it failed with.
async fn call(
    store: &mut Store<()>,
    instance: &Instance,
    name: &str,
    args: &[Val],
) -> Result<Option<Val>, Error> {
    func(instance, name)
        .call(store, args)
        .await
        .map(|values| values.first().cloned())
}

/// Every message in an error's source chain, joined so that a trap an
/// intrinsic raised can be matched wherever the substrate put it.
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
        Ok(other) => panic!("{name} answered {other:?}"),
        Err(error) => panic!("{name} failed: {}", chain(&error)),
    }
}

/// Call `name` and expect it to succeed.
async fn call_ok(store: &mut Store<()>, instance: &Instance, name: &str, args: &[Val]) {
    if let Err(error) = call(store, instance, name, args).await {
        panic!("{name} failed: {}", chain(&error));
    }
}

/// Call `name` and expect it to trap, reporting the message.
async fn call_trap(store: &mut Store<()>, instance: &Instance, name: &str, args: &[Val]) -> String {
    match call(store, instance, name, args).await {
        Err(error) => chain(&error),
        Ok(value) => panic!("{name} returned {value:?} rather than trapping"),
    }
}

/// Call the creator's `new` export `name` and split the `i64` it
/// returned into the readable end's index and the writable end's.
async fn new_ends(store: &mut Store<()>, instance: &Instance, name: &str) -> (u32, u32) {
    match call(store, instance, name, &[]).await {
        Ok(Some(Val::U64(packed))) => (packed as u32, (packed >> 32) as u32),
        Ok(other) => panic!("{name} answered {other:?}"),
        Err(error) => panic!("{name} failed: {}", chain(&error)),
    }
}

/// The handle table of the component instance whose export `name`
/// is, read the way a built-in reads it: through the instance index
/// the export's canon options name.
fn handle_table(instance: &Instance, name: &str) -> TableId {
    let export = func(instance, name);
    let state = export.abi_state().lock().expect("the instance's ABI state");
    state.handle_tables[export.options().instance]
}

/// The creator's handle table.
fn creator(instance: &Instance) -> TableId {
    handle_table(instance, "give-stream")
}

/// The reader's handle table.
fn reader(instance: &Instance) -> TableId {
    handle_table(instance, "drop-stream")
}

/// The entry at `index` of `table`.
fn entry(store: &Store<()>, table: TableId, index: u32) -> Option<HandleKind> {
    store
        .internal_ref()
        .tables()
        .lock()
        .expect("handle tables")
        .entry(table, index)
}

/// The end the entry at `index` of `table` names, which must be an end
/// of `kind`.
fn end_at(store: &Store<()>, table: TableId, index: u32, kind: EndKind) -> EndId {
    store
        .internal_ref()
        .tables()
        .lock()
        .expect("handle tables")
        .end_from_handle(table, index, kind)
        .expect("the index names an end of the kind")
}

/// The readable and the writable end the shared record of `end`
/// names.
fn pair_of(store: &Store<()>, end: EndId) -> (EndId, EndId) {
    let guard = store.internal_ref().tables().lock().expect("handle tables");
    let record = guard
        .tasks
        .shared_record(end)
        .expect("the end's shared record");
    (record.readable, record.writable)
}

/// How many end records and how many shared records the store holds.
fn record_counts(store: &Store<()>) -> (usize, usize) {
    let guard = store.internal_ref().tables().lock().expect("handle tables");
    (guard.tasks.end_count(), guard.tasks.shared_record_count())
}

/// Whether the store holds the end record `end` names.
fn end_exists(store: &Store<()>, end: EndId) -> bool {
    store
        .internal_ref()
        .tables()
        .lock()
        .expect("handle tables")
        .tasks
        .end(end)
        .is_some()
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
async fn it_moves_a_readable_stream_end_into_the_sibling_over_the_same_record() {
    let (mut store, instance) = instantiate(COMPOSED).await;
    // A future first, so the stream's indices in the creator are not
    // the ones a fresh table hands out.
    new_ends(&mut store, &instance, "new-future").await;
    let (readable, writable) = new_ends(&mut store, &instance, "new-stream").await;
    let (creator, reader) = (creator(&instance), reader(&instance));
    let sent = end_at(&store, creator, readable, EndKind::StreamReadable);
    let kept = end_at(&store, creator, writable, EndKind::StreamWritable);
    let counts = record_counts(&store);

    let received = call_u32(&mut store, &instance, "give-stream", &[Val::U32(readable)]).await;

    assert_eq!(
        (readable, received),
        (3, 1),
        "the reader's own table hands out the index the end arrives under"
    );
    assert_eq!(
        entry(&store, creator, readable),
        None,
        "the readable end left the creator's table"
    );
    assert_eq!(
        end_at(&store, reader, received, EndKind::StreamReadable),
        sent,
        "the reader's entry names the end record the creator's named"
    );
    assert_eq!(
        end_at(&store, creator, writable, EndKind::StreamWritable),
        kept,
        "the writable end stays in the creating instance"
    );
    assert_eq!(
        pair_of(&store, sent),
        (sent, kept),
        "the two ends still share their record"
    );
    assert_eq!(
        record_counts(&store),
        counts,
        "the crossing makes no record and takes none away"
    );
}

#[wcmp_macros::test]
async fn it_moves_a_readable_future_end_into_the_sibling_over_the_same_record() {
    let (mut store, instance) = instantiate(COMPOSED).await;
    new_ends(&mut store, &instance, "new-stream").await;
    let (readable, writable) = new_ends(&mut store, &instance, "new-future").await;
    let (creator, reader) = (creator(&instance), reader(&instance));
    let sent = end_at(&store, creator, readable, EndKind::FutureReadable);
    let kept = end_at(&store, creator, writable, EndKind::FutureWritable);

    let received = call_u32(&mut store, &instance, "give-future", &[Val::U32(readable)]).await;

    assert_eq!((readable, received), (3, 1));
    assert_eq!(entry(&store, creator, readable), None);
    assert_eq!(
        end_at(&store, reader, received, EndKind::FutureReadable),
        sent,
        "the reader's entry is a readable future end over the same record"
    );
    assert_eq!(
        end_at(&store, creator, writable, EndKind::FutureWritable),
        kept,
        "the writable end stays in the creating instance"
    );
    assert_eq!(pair_of(&store, sent), (sent, kept));
}

#[wcmp_macros::test]
async fn it_moves_a_readable_end_the_sibling_returns_flat_into_the_caller() {
    let (mut store, instance) = instantiate(COMPOSED).await;
    let (creator, reader) = (creator(&instance), reader(&instance));

    let received = call_u32(&mut store, &instance, "receive-stream", &[]).await;

    let end = end_at(&store, creator, received, EndKind::StreamReadable);
    let (_, writable) = pair_of(&store, end);
    assert_eq!(
        entry(&store, reader, 1),
        None,
        "the readable end left the reader, which made it"
    );
    assert_eq!(
        end_at(&store, reader, 2, EndKind::StreamWritable),
        writable,
        "the writable end stays in the reader, which made it"
    );
}

#[wcmp_macros::test]
async fn it_moves_a_readable_end_the_sibling_returns_through_memory_into_the_caller() {
    let (mut store, instance) = instantiate(COMPOSED).await;
    let (creator, reader) = (creator(&instance), reader(&instance));

    // The creator traps unless the number beside the end arrived too.
    let received = call_u32(&mut store, &instance, "receive-pair", &[]).await;

    let end = end_at(&store, creator, received, EndKind::StreamReadable);
    let (_, writable) = pair_of(&store, end);
    assert_eq!(entry(&store, reader, 1), None);
    assert_eq!(
        end_at(&store, reader, 2, EndKind::StreamWritable),
        writable,
        "the writable end stays in the reader, which made it"
    );
}

#[wcmp_macros::test]
async fn it_moves_a_readable_end_returned_through_task_return_into_the_caller() {
    let (mut store, instance) = instantiate(COMPOSED).await;
    let (creator, reader) = (creator(&instance), reader(&instance));

    let received = call_u32(&mut store, &instance, "receive-async", &[]).await;

    let end = end_at(&store, creator, received, EndKind::StreamReadable);
    let (readable, writable) = pair_of(&store, end);
    assert_eq!(
        readable, end,
        "the caller's entry names the readable end of the pair"
    );
    assert_eq!(
        entry(&store, reader, 1),
        None,
        "the `task.return` took the readable end out of the reader"
    );
    assert_eq!(
        end_at(&store, reader, 2, EndKind::StreamWritable),
        writable,
        "the writable end stays in the reader, which made it"
    );
}

#[wcmp_macros::test]
async fn it_traps_a_crossing_of_an_end_in_a_waitable_set_with_the_message_of_its_kind() {
    for (new, give, expected) in [
        (
            "new-stream",
            "give-stream",
            "cannot lift stream while it's in a waitable set",
        ),
        (
            "new-future",
            "give-future",
            "cannot lift future while it's in a waitable set",
        ),
    ] {
        let (mut store, instance) = instantiate(COMPOSED).await;
        let (readable, _) = new_ends(&mut store, &instance, new).await;
        let set = call_u32(&mut store, &instance, "new-set", &[]).await;
        call_ok(
            &mut store,
            &instance,
            "join",
            &[Val::U32(readable), Val::U32(set)],
        )
        .await;

        let message = call_trap(&mut store, &instance, give, &[Val::U32(readable)]).await;

        assert!(message.contains(expected), "{give}: {message}");
        assert!(
            entry(&store, creator(&instance), readable).is_some(),
            "the refused lift kept the entry"
        );
    }
}

#[wcmp_macros::test]
async fn it_traps_a_crossing_of_an_end_of_another_payload_type() {
    let (mut store, instance) = instantiate(COMPOSED).await;
    let (readable, _) = new_ends(&mut store, &instance, "new-other-stream").await;

    let message = call_trap(&mut store, &instance, "give-stream", &[Val::U32(readable)]).await;

    assert!(
        message.contains(
            "the readable end of a stream carries another payload type than the built-in or \
             crossing declares"
        ),
        "a `stream<u32>` end does not cross as a `stream<u8>`: {message}"
    );
}

#[wcmp_macros::test]
async fn it_traps_a_crossing_of_a_busy_end_with_the_message_of_its_kind() {
    for (new, give, kind, expected) in [
        (
            "new-stream",
            "give-stream",
            EndKind::StreamReadable,
            "cannot remove busy stream",
        ),
        (
            "new-future",
            "give-future",
            EndKind::FutureReadable,
            "cannot remove busy future",
        ),
    ] {
        for state in [CopyState::Copying, CopyState::Cancelling] {
            let (mut store, instance) = instantiate(COMPOSED).await;
            let (readable, _) = new_ends(&mut store, &instance, new).await;
            let end = end_at(&store, creator(&instance), readable, kind);
            set_copy_state(&mut store, end, state);

            let message = call_trap(&mut store, &instance, give, &[Val::U32(readable)]).await;

            assert!(message.contains(expected), "{give} in {state:?}: {message}");
        }
    }
}

#[wcmp_macros::test]
async fn it_traps_a_crossing_of_a_done_end_with_the_message_of_its_kind() {
    for (new, give, kind, expected) in [
        (
            "new-stream",
            "give-stream",
            EndKind::StreamReadable,
            "cannot lift stream after being notified that the writable end dropped",
        ),
        (
            "new-future",
            "give-future",
            EndKind::FutureReadable,
            "cannot lift future after previous read succeeded",
        ),
    ] {
        let (mut store, instance) = instantiate(COMPOSED).await;
        let (readable, _) = new_ends(&mut store, &instance, new).await;
        let end = end_at(&store, creator(&instance), readable, kind);
        set_copy_state(&mut store, end, CopyState::Done);

        let message = call_trap(&mut store, &instance, give, &[Val::U32(readable)]).await;

        assert!(message.contains(expected), "{give}: {message}");
    }
}

#[wcmp_macros::test]
async fn it_traps_a_crossing_of_a_writable_end() {
    let (mut store, instance) = instantiate(COMPOSED).await;
    let (_, writable) = new_ends(&mut store, &instance, "new-stream").await;

    let message = call_trap(&mut store, &instance, "give-stream", &[Val::U32(writable)]).await;

    assert!(
        message.contains(&format!(
            "handle index {writable} is not a readable end of a stream"
        )),
        "only the readable end crosses: {message}"
    );
}

#[wcmp_macros::test]
async fn it_misses_an_old_end_identity_once_its_slot_is_reused_after_a_crossing() {
    let (mut store, instance) = instantiate(COMPOSED).await;
    let (readable, writable) = new_ends(&mut store, &instance, "new-stream").await;
    let (creator, reader) = (creator(&instance), reader(&instance));
    let old = [
        end_at(&store, creator, readable, EndKind::StreamReadable),
        end_at(&store, creator, writable, EndKind::StreamWritable),
    ];
    let received = call_u32(&mut store, &instance, "give-stream", &[Val::U32(readable)]).await;

    // The two ends go from the two tables they ended up in, which
    // takes both records and the shared record out of the store.
    call_ok(&mut store, &instance, "drop-stream", &[Val::U32(received)]).await;
    call_ok(
        &mut store,
        &instance,
        "drop-stream-writable",
        &[Val::U32(writable)],
    )
    .await;
    assert_eq!(record_counts(&store), (0, 0));
    assert_eq!(
        entry(&store, reader, received),
        None,
        "the reader's entry went with its drop"
    );

    let (readable, writable) = new_ends(&mut store, &instance, "new-stream").await;
    let new = [
        end_at(&store, creator, readable, EndKind::StreamReadable),
        end_at(&store, creator, writable, EndKind::StreamWritable),
    ];

    for end in new {
        let previous = old
            .iter()
            .find(|previous| previous.index() == end.index())
            .expect("the new end took a slot an old end freed");
        assert_ne!(end, *previous, "the slot's generation moved on");
        assert!(end_exists(&store, end));
        assert!(
            !end_exists(&store, *previous),
            "the old identity names no record at all"
        );
    }
}

#[wcmp_macros::test]
async fn it_fails_a_readable_end_the_host_would_receive_through_memory_as_unsupported() {
    let (mut store, instance) = instantiate(COMPOSED).await;

    let error = call(&mut store, &instance, "return-pair", &[])
        .await
        .expect_err("the host has no value to hold an end in");

    assert!(
        matches!(error, Error::Unsupported { .. }),
        "the lift fails after its checks: {}",
        chain(&error)
    );
    assert!(
        matches!(
            entry(&store, reader(&instance), 1),
            Some(HandleKind::StreamReadable { .. })
        ),
        "the end stays in the table it was lifted from"
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_host_value_that_is_not_a_readable_end_in_the_flat_form() {
    let (mut store, instance) = instantiate(COMPOSED).await;

    let error = call(&mut store, &instance, "take-stream", &[Val::U32(1)])
        .await
        .expect_err("a number is not a readable end");

    assert!(
        matches!(&error, Error::Abi(abi) if matches!(abi.cause, AbiCause::HostValueMismatch)),
        "{}",
        chain(&error)
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_host_value_that_is_not_a_readable_end_in_the_memory_form() {
    let (mut store, instance) = instantiate(COMPOSED).await;

    let error = call(
        &mut store,
        &instance,
        "take-list",
        &[Val::List(vec![Val::U32(1)].into_boxed_slice())],
    )
    .await
    .expect_err("a number is not a readable end");

    assert!(
        matches!(&error, Error::Abi(abi) if matches!(abi.cause, AbiCause::HostValueMismatch)),
        "{}",
        chain(&error)
    );
}
