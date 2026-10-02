// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Baseline tests for how a readable end the host holds ends.
//!
//! A reader the host holds ends through a pipe or a close. `close`
//! drops the readable end: a guest write in progress completes with
//! the dropped result, and a later write sees that result at once. A
//! stream or future the host created takes its producer with it, which
//! is dropped unpolled. `close_with` closes through an accessor inside
//! a poll, and a guard closes on drop through the accessor it holds,
//! so a guard dropped outside a poll of its store leaks its end the
//! way a reader dropped without a close does: until the store drops,
//! with the guest's write pending all along. Dropping the store drops
//! every shared record, every end, and every producer and consumer,
//! polling none of them.
//!
//! Several host values can name one end: clones of an untyped value,
//! and the readers converted from them. Once one lowers, pipes, or
//! closes the end, the others may still convert, and a close of an end
//! closed already does nothing, as in Wasmtime. An end one piped to a
//! consumer while the guest holds the writable end may be piped again,
//! which replaces the consumer, or closed, which drops it, while no
//! write is in flight, as in Wasmtime too. Every other use is refused.
//! A use of an end that is gone, or of a value that closed, fails with
//! Wasmtime's "resource not present".
//!
//! The tests drive one component whose synchronous exports each call
//! one built-in, and read what the store holds through its internal
//! surface: the end and shared records, the producers the scheduler
//! holds, and the wakers kept for host ends.

#![cfg(test)]

use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::task::Wake;

use crate::concurrency::EndId;
use crate::internal::{FutureReaderInternal, StreamReaderInternal};
use crate::resource::HandleTables;
use crate::store::StoreInternalExt;
use crate::{
    AbiCause, Component, CopyCause, Destination, EndKind, Engine, Error, FutureConsumer,
    FutureReader, GuardedFutureReader, Instance, Linker, SchedulerCause, Source, Store,
    StoreContext, StreamAny, StreamConsumer, StreamProducer, StreamReader, StreamResult, Val,
};
use wcmp_macros::component;

/// The word a copy returns when it has not finished.
const BLOCKED: u32 = 0xffff_ffff;

/// The code a poll of a set that holds no event delivers.
const EVENT_NONE: u32 = 0;

/// The code of the event a write on a stream end delivers.
const STREAM_WRITE: u32 = 3;

/// The code of the event a write on a future end delivers.
const FUTURE_WRITE: u32 = 5;

/// The result a copy that found the other end dropped packs.
const DROPPED: u32 = 1;

/// How many calls a test makes to show that no event arrives: each is
/// a driver of the store that runs turns.
const CALLS_WITHOUT_AN_EVENT: usize = 4;

/// A component that creates a `stream<u8>` and a `future<u32>` and
/// writes them, one built-in per synchronous export.
///
/// `make` creates a stream, keeps its writable end, and returns its
/// readable end; `make-future` does the same for a future. `write`
/// and `future-write` start an asynchronous write of the kept writable
/// end from a pointer, and `drop-writable` and `future-drop-writable`
/// drop it. `writable` and `future-writable` return the kept indices.
/// `take` takes a stream from its caller and returns the index it
/// arrived under, `give` returns the stream at an index to its
/// caller, and `drop-readable` drops the readable end at an index.
/// `poll` polls a set and writes the event it delivers at address 0:
/// the end's index there and the packed result at address 4. `peek`
/// reads a word of memory and `poke` writes one.
const WRITES: &[u8] = component!(
    r#"
    (component
      (type $s (stream u8))
      (type $f (future u32))
      (core module $libc
        (memory (export "memory") 1)
        (func (export "peek") (param i32) (result i32) (i32.load (local.get 0)))
        (func (export "poke") (param i32 i32) (i32.store (local.get 0) (local.get 1))))
      (core instance $libc (instantiate $libc))

      (core func $stream-new (canon stream.new $s))
      (core func $future-new (canon future.new $f))
      (core func $write (canon stream.write $s async (memory (core memory $libc "memory"))))
      (core func $future-write
        (canon future.write $f async (memory (core memory $libc "memory"))))
      (core func $drop-writable (canon stream.drop-writable $s))
      (core func $drop-readable (canon stream.drop-readable $s))
      (core func $future-drop-writable (canon future.drop-writable $f))
      (core func $set-new (canon waitable-set.new))
      (core func $poll (canon waitable-set.poll (memory (core memory $libc "memory"))))
      (core func $join (canon waitable.join))

      (core module $m
        (import "" "stream.new" (func $stream-new (result i64)))
        (import "" "future.new" (func $future-new (result i64)))
        (import "" "stream.write" (func $write (param i32 i32 i32) (result i32)))
        (import "" "future.write" (func $future-write (param i32 i32) (result i32)))
        (import "" "stream.drop-writable" (func $drop-writable (param i32)))
        (import "" "stream.drop-readable" (func $drop-readable (param i32)))
        (import "" "future.drop-writable" (func $future-drop-writable (param i32)))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (import "" "waitable-set.poll" (func $poll (param i32 i32) (result i32)))
        (import "" "waitable.join" (func $join (param i32 i32)))
        (global $w (mut i32) (i32.const 0))
        (global $fw (mut i32) (i32.const 0))
        (func (export "make") (result i32)
          (local $pair i64)
          (local.set $pair (call $stream-new))
          (global.set $w (i32.wrap_i64 (i64.shr_u (local.get $pair) (i64.const 32))))
          (i32.wrap_i64 (local.get $pair)))
        (func (export "make-future") (result i32)
          (local $pair i64)
          (local.set $pair (call $future-new))
          (global.set $fw (i32.wrap_i64 (i64.shr_u (local.get $pair) (i64.const 32))))
          (i32.wrap_i64 (local.get $pair)))
        (func (export "write") (param i32 i32) (result i32)
          (call $write (global.get $w) (local.get 0) (local.get 1)))
        (func (export "future-write") (param i32) (result i32)
          (call $future-write (global.get $fw) (local.get 0)))
        (func (export "drop-writable") (call $drop-writable (global.get $w)))
        (func (export "future-drop-writable") (call $future-drop-writable (global.get $fw)))
        (func (export "writable") (result i32) (global.get $w))
        (func (export "future-writable") (result i32) (global.get $fw))
        (func (export "take") (param i32) (result i32) (local.get 0))
        (func (export "give") (param i32) (result i32) (local.get 0))
        (func (export "drop-readable") (param i32) (call $drop-readable (local.get 0)))
        (func (export "new-set") (result i32) (call $set-new))
        (func (export "poll") (param i32) (result i32) (call $poll (local.get 0) (i32.const 0)))
        (func (export "join") (param i32 i32) (call $join (local.get 0) (local.get 1))))
      (core instance $m (instantiate $m (with "" (instance
        (export "stream.new" (func $stream-new))
        (export "future.new" (func $future-new))
        (export "stream.write" (func $write))
        (export "future.write" (func $future-write))
        (export "stream.drop-writable" (func $drop-writable))
        (export "stream.drop-readable" (func $drop-readable))
        (export "future.drop-writable" (func $future-drop-writable))
        (export "waitable-set.new" (func $set-new))
        (export "waitable-set.poll" (func $poll))
        (export "waitable.join" (func $join))))))

      (func (export "make") (result $s) (canon lift (core func $m "make")))
      (func (export "make-future") (result $f) (canon lift (core func $m "make-future")))
      (func (export "write") (param "p" u32) (param "n" u32) (result u32)
        (canon lift (core func $m "write")))
      (func (export "future-write") (param "p" u32) (result u32)
        (canon lift (core func $m "future-write")))
      (func (export "drop-writable") (canon lift (core func $m "drop-writable")))
      (func (export "future-drop-writable") (canon lift (core func $m "future-drop-writable")))
      (func (export "writable") (result u32) (canon lift (core func $m "writable")))
      (func (export "future-writable") (result u32) (canon lift (core func $m "future-writable")))
      (func (export "take") (param "s" $s) (result u32) (canon lift (core func $m "take")))
      (func (export "give") (param "i" u32) (result $s) (canon lift (core func $m "give")))
      (func (export "drop-readable") (param "i" u32)
        (canon lift (core func $m "drop-readable")))
      (func (export "new-set") (result u32) (canon lift (core func $m "new-set")))
      (func (export "poll") (param "s" u32) (result u32) (canon lift (core func $m "poll")))
      (func (export "join") (param "w" u32) (param "s" u32) (canon lift (core func $m "join")))
      (func (export "peek") (param "p" u32) (result u32) (canon lift (core func $libc "peek")))
      (func (export "poke") (param "p" u32) (param "v" u32) (canon lift (core func $libc "poke"))))
    "#
);

/// How often the producers and consumers of a test were polled and
/// dropped.
#[derive(Default)]
struct Counts {
    polls: AtomicUsize,
    drops: AtomicUsize,
}

impl Counts {
    fn polls(&self) -> usize {
        self.polls.load(Ordering::SeqCst)
    }

    fn drops(&self) -> usize {
        self.drops.load(Ordering::SeqCst)
    }
}

/// A producer or a consumer that counts its polls and its drop, and
/// answers every poll pending.
struct Counted(Arc<Counts>);

impl Drop for Counted {
    fn drop(&mut self) {
        self.0.drops.fetch_add(1, Ordering::SeqCst);
    }
}

impl StreamProducer<()> for Counted {
    type Item = u8;

    fn poll_produce(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _store: &mut StoreContext<'_, ()>,
        _destination: Destination<'_, u8>,
        _finish: bool,
    ) -> Poll<Result<StreamResult, Error>> {
        self.0.polls.fetch_add(1, Ordering::SeqCst);
        Poll::Pending
    }
}

impl StreamConsumer<()> for Counted {
    type Item = u8;

    fn poll_consume(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _store: &mut StoreContext<'_, ()>,
        _source: Source<'_, u8>,
        _finish: bool,
    ) -> Poll<Result<StreamResult, Error>> {
        self.0.polls.fetch_add(1, Ordering::SeqCst);
        Poll::Pending
    }
}

impl FutureConsumer<()> for Counted {
    type Item = u32;

    fn poll_consume(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _store: &mut StoreContext<'_, ()>,
        _source: Source<'_, u32>,
        _finish: bool,
    ) -> Poll<Result<(), Error>> {
        self.0.polls.fetch_add(1, Ordering::SeqCst);
        Poll::Pending
    }
}

/// A consumer that counts its polls and its drop through the
/// [`Counted`] it holds, and takes the whole of every write at once.
struct Taking(Counted);

impl StreamConsumer<()> for Taking {
    type Item = u8;

    fn poll_consume(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        store: &mut StoreContext<'_, ()>,
        mut source: Source<'_, u8>,
        _finish: bool,
    ) -> Poll<Result<StreamResult, Error>> {
        self.0.0.polls.fetch_add(1, Ordering::SeqCst);
        let count = source.remaining();
        source.read(store, &mut Vec::new(), count)?;
        Poll::Ready(Ok(StreamResult::Completed))
    }
}

/// A future producer, as an `async` block writes one, that counts its
/// polls and its drop through `counted` and never resolves.
fn counted_future(counted: Counted) -> impl crate::FutureProducer<(), Item = u32> {
    core::future::poll_fn(move |_cx| {
        counted.0.polls.fetch_add(1, Ordering::SeqCst);
        Poll::<Result<u32, Error>>::Pending
    })
}

/// A waker that counts its wakes and whose references are counted:
/// the test holds one strong reference, and every waker built from it
/// holds another.
#[derive(Default)]
struct Idle {
    wakes: AtomicUsize,
}

impl Wake for Idle {
    fn wake(self: Arc<Self>) {
        self.wakes.fetch_add(1, Ordering::SeqCst);
    }
}

/// Instantiate [`WRITES`] into a store of its own.
async fn instantiate() -> (Store<()>, Instance) {
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, WRITES)
        .await
        .expect("the component parses");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let linker: Linker<()> = Linker::new(&engine);
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiation succeeds");
    (store, instance)
}

/// Every message in an error's source chain, joined, so that a trap a
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

/// Call `name` with the numbers `args` and report the one value it
/// returned, or the message of the trap it raised.
async fn call(
    store: &mut Store<()>,
    instance: &Instance,
    name: &str,
    args: &[u32],
) -> Result<Option<Val>, String> {
    let args: Vec<Val> = args.iter().map(|&n| Val::U32(n)).collect();
    instance
        .get_func(name)
        .unwrap_or_else(|| panic!("the component exports `{name}`"))
        .call(store, &args)
        .await
        .map(|values| values.first().cloned())
        .map_err(|error| chain(&error))
}

/// Call `name` and expect it to return one `u32`.
async fn call_u32(store: &mut Store<()>, instance: &Instance, name: &str, args: &[u32]) -> u32 {
    match call(store, instance, name, args).await {
        Ok(Some(Val::U32(value))) => value,
        other => panic!("{name} answered {other:?}"),
    }
}

/// Call `name` and expect it to succeed.
async fn call_ok(store: &mut Store<()>, instance: &Instance, name: &str, args: &[u32]) {
    if let Err(message) = call(store, instance, name, args).await {
        panic!("{name} trapped: {message}");
    }
}

/// Call the guest's `make` through a typed call, which hands the host
/// the readable end of the stream the guest created.
async fn make(store: &mut Store<()>, instance: &Instance) -> StreamReader<u8> {
    instance
        .get_func("make")
        .expect("the component exports `make`")
        .typed::<(), StreamReader<u8>>()
        .expect("`make` returns a `stream<u8>`")
        .call(store, ())
        .await
        .expect("the readable end crosses to the host")
}

/// Call the guest's `make-future` through a typed call, which hands
/// the host the readable end of the future the guest created.
async fn make_future(store: &mut Store<()>, instance: &Instance) -> FutureReader<u32> {
    instance
        .get_func("make-future")
        .expect("the component exports `make-future`")
        .typed::<(), FutureReader<u32>>()
        .expect("`make-future` returns a `future<u32>`")
        .call(store, ())
        .await
        .expect("the readable end crosses to the host")
}

/// Start the guest's asynchronous write of four bytes of its memory to
/// the stream it keeps. Answers what the write returned.
async fn write(store: &mut Store<()>, instance: &Instance) -> u32 {
    call_ok(store, instance, "poke", &[100, 0x0403_0201]).await;
    call_u32(store, instance, "write", &[100, 4]).await
}

/// Start the guest's asynchronous write of a `u32` to the future it
/// keeps. Answers what the write returned.
async fn future_write(store: &mut Store<()>, instance: &Instance) -> u32 {
    call_ok(store, instance, "poke", &[100, 42]).await;
    call_u32(store, instance, "future-write", &[100]).await
}

/// Join the end the export `end` names to a fresh set and poll the set
/// once. Answers the event as the guest reads it: the code, the end's
/// index, and the packed result.
async fn poll_event(store: &mut Store<()>, instance: &Instance, end: &str) -> (u32, u32, u32) {
    let end = call_u32(store, instance, end, &[]).await;
    let set = call_u32(store, instance, "new-set", &[]).await;
    call_ok(store, instance, "join", &[end, set]).await;
    let code = call_u32(store, instance, "poll", &[set]).await;
    let index = call_u32(store, instance, "peek", &[0]).await;
    let packed = call_u32(store, instance, "peek", &[4]).await;
    call_ok(store, instance, "join", &[end, 0]).await;
    (code, index, packed)
}

/// Poll a set that holds the end the export `end` names, a call at a
/// time, [`CALLS_WITHOUT_AN_EVENT`] times, and answer whether any poll
/// delivered an event.
async fn any_event(store: &mut Store<()>, instance: &Instance, end: &str) -> bool {
    let end = call_u32(store, instance, end, &[]).await;
    let set = call_u32(store, instance, "new-set", &[]).await;
    call_ok(store, instance, "join", &[end, set]).await;
    let mut delivered = false;
    for _ in 0..CALLS_WITHOUT_AN_EVENT {
        delivered |= call_u32(store, instance, "poll", &[set]).await != EVENT_NONE;
    }
    call_ok(store, instance, "join", &[end, 0]).await;
    delivered
}

/// The count of end records and of shared records the store holds.
fn record_counts(store: &Store<()>) -> (usize, usize) {
    let guard = store.internal_ref().tables().lock().expect("handle tables");
    (guard.tasks.end_count(), guard.tasks.shared_record_count())
}

/// The writable end of the stream or future whose readable end is
/// `reader`.
fn writer_of(store: &Store<()>, reader: EndId) -> EndId {
    store
        .internal_ref()
        .tables()
        .lock()
        .expect("handle tables")
        .tasks
        .shared_record(reader)
        .expect("the reader's shared record")
        .writable
}

/// A weak handle on the store's handle tables, which hold every end
/// and shared record: it upgrades for as long as anything keeps them.
fn tables_of(store: &mut Store<()>) -> Weak<Mutex<HandleTables>> {
    Arc::downgrade(&store.internal().tables_handle())
}

/// Whether `failure` is the not-held cause for `kind`.
fn not_held(failure: &Error, kind: EndKind) -> bool {
    matches!(failure, Error::Copy(CopyCause::NotHeldByHost { kind: k }) if *k == kind)
}

/// Whether `failure` is the not-present cause for `kind`.
fn not_present(failure: &Error, kind: EndKind) -> bool {
    matches!(failure, Error::Copy(CopyCause::HostEndNotPresent { kind: k }) if *k == kind)
}

/// Whether `failure` is a lower refused as an invalid handle for
/// `reason`.
fn invalid_handle(failure: &Error, reason: &str) -> bool {
    matches!(
        failure,
        Error::Abi(abi) if matches!(&abi.cause, AbiCause::InvalidHandle { reason: r } if r == reason)
    )
}

/// The reason a lower of a value whose end the host gave up is
/// refused for.
const NOT_HELD: &str = "the readable end is not one the host holds";

/// Call the guest's `make` and convert the reader it hands the host
/// into an untyped value, which the tests of copies clone.
async fn make_any(store: &mut Store<()>, instance: &Instance) -> StreamAny {
    make(store, instance)
        .await
        .try_into_stream_any(&mut store.as_context_mut())
        .expect("the host holds the end the guest handed it")
}

/// Lower `stream` into the guest through its `take`, and answer the
/// index the guest's entry took, or the lower's refusal.
async fn take(
    store: &mut Store<()>,
    instance: &Instance,
    stream: &StreamAny,
) -> Result<u32, Error> {
    let results = instance
        .get_func("take")
        .expect("the component exports `take`")
        .call(store, &[Val::Stream(stream.clone())])
        .await?;
    match results.as_ref() {
        [Val::U32(index)] => Ok(*index),
        other => panic!("`take` answered {other:?}"),
    }
}

/// Pipe `stream` to `consumer` through its typed reader.
fn pipe(
    store: &mut Store<()>,
    stream: &StreamAny,
    consumer: impl StreamConsumer<(), Item = u8>,
) -> Result<(), Error> {
    StreamReader::<u8>::try_from_stream_any(stream.clone())
        .expect("the stream carries `u8`")
        .pipe(&mut store.as_context_mut(), consumer)
}

/// Close a clone of `stream`, which leaves `stream` itself naming its
/// end whatever the close does.
fn close_copy(store: &mut Store<()>, stream: &StreamAny) -> Result<(), Error> {
    stream.clone().close(&mut store.as_context_mut())
}

#[wcmp_macros::test]
async fn it_completes_a_pending_guest_write_with_the_dropped_result_when_the_host_closes() {
    let (mut store, instance) = instantiate().await;
    let mut reader = make(&mut store, &instance).await;
    assert_eq!(write(&mut store, &instance).await, BLOCKED);

    reader
        .close(&mut store.as_context_mut())
        .expect("the host closes the stream");

    let writable = call_u32(&mut store, &instance, "writable", &[]).await;
    assert_eq!(
        poll_event(&mut store, &instance, "writable").await,
        (STREAM_WRITE, writable, DROPPED),
        "the pending write completes with the dropped result and nothing moved"
    );
    assert_eq!(
        record_counts(&store),
        (2, 1),
        "the records stay until the guest drops its writable end"
    );
    call_ok(&mut store, &instance, "drop-writable", &[]).await;
    assert_eq!(
        record_counts(&store),
        (0, 0),
        "the guest's drop is the second of the pair"
    );
}

#[wcmp_macros::test]
async fn it_completes_the_guests_next_write_with_the_dropped_result_at_once() {
    let (mut store, instance) = instantiate().await;
    let mut reader = make(&mut store, &instance).await;

    reader
        .close(&mut store.as_context_mut())
        .expect("the host closes the stream");

    assert_eq!(
        write(&mut store, &instance).await,
        DROPPED,
        "a write after the close completes at once with the dropped result, \
         never the blocked sentinel"
    );
    call_ok(&mut store, &instance, "drop-writable", &[]).await;
    assert_eq!(record_counts(&store), (0, 0));
}

#[wcmp_macros::test]
async fn it_completes_a_pending_future_write_with_the_dropped_result_when_the_host_closes() {
    let (mut store, instance) = instantiate().await;
    let mut reader = make_future(&mut store, &instance).await;
    assert_eq!(future_write(&mut store, &instance).await, BLOCKED);

    reader
        .close(&mut store.as_context_mut())
        .expect("the host closes the future");

    let writable = call_u32(&mut store, &instance, "future-writable", &[]).await;
    assert_eq!(
        poll_event(&mut store, &instance, "future-writable").await,
        (FUTURE_WRITE, writable, DROPPED),
        "the pending write completes with the dropped result"
    );
    call_ok(&mut store, &instance, "future-drop-writable", &[]).await;
    assert_eq!(
        record_counts(&store),
        (0, 0),
        "a writable future end told of the drop is done, and drops cleanly"
    );
}

#[wcmp_macros::test]
async fn it_refuses_to_close_or_lower_a_reader_it_closed_already() {
    let (mut store, instance) = instantiate().await;
    let mut reader = make(&mut store, &instance).await;
    reader
        .close(&mut store.as_context_mut())
        .expect("the first close succeeds");

    let again = reader
        .close(&mut store.as_context_mut())
        .expect_err("the second close is refused");
    assert!(
        not_present(&again, EndKind::StreamReadable),
        "the close left the reader naming no end, as Wasmtime's does: {again}"
    );
    assert_eq!(
        again.to_string(),
        "copy error: resource not present",
        "the cause carries Wasmtime's message"
    );

    let lowered = instance
        .get_func("take")
        .expect("the component exports `take`")
        .typed::<(StreamReader<u8>,), u32>()
        .expect("`take` takes a `stream<u8>`")
        .call(&mut store, (reader,))
        .await;
    let lowered = lowered.expect_err("a closed end does not enter a guest's table");
    assert!(
        invalid_handle(&lowered, "resource not present"),
        "{lowered:?}"
    );
    assert_eq!(
        record_counts(&store),
        (2, 1),
        "the refusals touched nothing: the records wait for the guest's drop"
    );
}

#[wcmp_macros::test]
async fn it_drops_the_producer_of_a_stream_the_host_created_unpolled_on_close() {
    let (mut store, _instance) = instantiate().await;
    let counts = Arc::new(Counts::default());
    let mut reader = StreamReader::<u8>::new(&mut store.as_context_mut(), Counted(counts.clone()))
        .expect("the host creates a stream");
    let writer = writer_of(&store, StreamReaderInternal::end(&reader));
    let idle = Arc::new(Idle::default());
    store
        .internal()
        .scheduler_mut()
        .set_host_end_waker(writer, Waker::from(idle.clone()));

    reader
        .close(&mut store.as_context_mut())
        .expect("the host closes the stream");

    assert_eq!(counts.drops(), 1, "the producer is dropped");
    assert_eq!(counts.polls(), 0, "the producer is never polled");
    let scheduler = store.internal().scheduler_mut();
    assert!(
        !scheduler.holds_host_writer(writer),
        "the scheduler holds no producer for the end"
    );
    assert!(
        scheduler.take_host_end_waker(writer).is_none(),
        "the scheduler keeps no waker for the end"
    );
    assert_eq!(
        Arc::strong_count(&idle),
        1,
        "the waker kept for the end is dropped with it"
    );
    assert_eq!(
        idle.wakes.load(Ordering::SeqCst),
        0,
        "the waker is let go of, not woken"
    );
    assert_eq!(
        record_counts(&store),
        (0, 0),
        "the host's writable end drops as the second of the pair"
    );
}

#[wcmp_macros::test]
async fn it_drops_the_producer_of_a_future_the_host_created_unpolled_on_close() {
    let (mut store, _instance) = instantiate().await;
    let counts = Arc::new(Counts::default());
    let mut reader = FutureReader::<u32>::new(
        &mut store.as_context_mut(),
        counted_future(Counted(counts.clone())),
    )
    .expect("the host creates a future");
    let writer = writer_of(&store, FutureReaderInternal::end(&reader));

    reader
        .close(&mut store.as_context_mut())
        .expect("the host closes the future");

    assert_eq!(counts.drops(), 1, "the producer is dropped");
    assert_eq!(counts.polls(), 0, "the producer is never polled");
    assert!(!store.internal().scheduler().holds_host_writer(writer));
    assert_eq!(record_counts(&store), (0, 0));
}

#[wcmp_macros::test]
async fn it_frees_the_records_when_the_host_closes_a_stream_whose_writer_dropped() {
    let (mut store, instance) = instantiate().await;
    let mut reader = make(&mut store, &instance).await;
    call_ok(&mut store, &instance, "drop-writable", &[]).await;
    assert_eq!(record_counts(&store), (2, 1));

    reader
        .close(&mut store.as_context_mut())
        .expect("the host closes the stream");

    assert_eq!(
        record_counts(&store),
        (0, 0),
        "the close is the second drop of the pair"
    );
}

#[wcmp_macros::test]
async fn it_leaves_a_guest_write_pending_until_the_store_drops_when_a_reader_is_dropped() {
    let (mut store, instance) = instantiate().await;
    {
        // The reader goes out of scope neither piped nor closed.
        let _reader = make(&mut store, &instance).await;
    }

    assert_eq!(write(&mut store, &instance).await, BLOCKED);
    assert!(
        !any_event(&mut store, &instance, "writable").await,
        "nothing tells the writer: its write waits"
    );
    assert_eq!(
        record_counts(&store),
        (2, 1),
        "the dropped reader's end leaks with the stream"
    );

    let tables = tables_of(&mut store);
    drop(store);
    // In the browser the runtime layer keeps the store, and the host
    // functions that hold the tables, for a guest call the browser has
    // yet to settle. They drop once it settles.
    #[cfg(target_arch = "wasm32")]
    settle_until(|| tables.upgrade().is_none()).await;
    assert!(
        tables.upgrade().is_none(),
        "the leaked records go when the store drops"
    );
    drop(instance);
}

#[wcmp_macros::test]
async fn it_closes_a_guarded_reader_that_drops_inside_a_poll() {
    let (mut store, instance) = instantiate().await;
    let reader = make(&mut store, &instance).await;
    assert_eq!(write(&mut store, &instance).await, BLOCKED);

    store
        .run_concurrent(async move |accessor| drop(reader.guard(accessor.clone())))
        .await
        .expect("run the closure");

    let writable = call_u32(&mut store, &instance, "writable", &[]).await;
    assert_eq!(
        poll_event(&mut store, &instance, "writable").await,
        (STREAM_WRITE, writable, DROPPED),
        "the guard's drop closed the stream"
    );
}

#[wcmp_macros::test]
async fn it_leaks_the_end_of_a_guard_that_drops_outside_a_poll() {
    let (mut store, instance) = instantiate().await;
    let reader = make(&mut store, &instance).await;
    assert_eq!(write(&mut store, &instance).await, BLOCKED);

    let guard = store
        .run_concurrent(async move |accessor| reader.guard(accessor.clone()))
        .await
        .expect("run the closure");
    drop(guard);

    assert!(
        !any_event(&mut store, &instance, "writable").await,
        "a guard dropped outside a poll cannot reach the store"
    );
    assert_eq!(record_counts(&store), (2, 1), "its end leaks");
}

#[wcmp_macros::test]
async fn it_gives_the_reader_back_from_a_guard_without_closing() {
    let (mut store, instance) = instantiate().await;
    let reader = make(&mut store, &instance).await;
    assert_eq!(write(&mut store, &instance).await, BLOCKED);

    let mut reader = store
        .run_concurrent(async move |accessor| reader.guard(accessor.clone()).into_stream())
        .await
        .expect("run the closure");
    assert!(
        !any_event(&mut store, &instance, "writable").await,
        "a guard that gave its reader back closed nothing"
    );

    reader
        .close(&mut store.as_context_mut())
        .expect("the reader given back closes");
    let writable = call_u32(&mut store, &instance, "writable", &[]).await;
    assert_eq!(
        poll_event(&mut store, &instance, "writable").await,
        (STREAM_WRITE, writable, DROPPED)
    );
}

#[wcmp_macros::test]
async fn it_closes_a_future_through_an_accessor_inside_a_poll() {
    let (mut store, instance) = instantiate().await;
    let mut reader = make_future(&mut store, &instance).await;
    assert_eq!(future_write(&mut store, &instance).await, BLOCKED);

    store
        .run_concurrent(async move |accessor| reader.close_with(accessor))
        .await
        .expect("run the closure")
        .expect("the close reaches the store");

    let writable = call_u32(&mut store, &instance, "future-writable", &[]).await;
    assert_eq!(
        poll_event(&mut store, &instance, "future-writable").await,
        (FUTURE_WRITE, writable, DROPPED)
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_close_through_an_accessor_outside_a_poll() {
    let (mut store, instance) = instantiate().await;
    let mut reader = make(&mut store, &instance).await;
    let accessor = store
        .run_concurrent(async |accessor| accessor.clone())
        .await
        .expect("run the closure");

    let failure = reader
        .close_with(&accessor)
        .expect_err("no poll of the store is running");
    assert_eq!(
        failure.to_string(),
        Error::Scheduler(SchedulerCause::StoreNotInPoll).to_string()
    );
    reader
        .close(&mut store.as_context_mut())
        .expect("the reader is still the host's to close");
}

#[wcmp_macros::test]
async fn it_drops_every_record_producer_and_consumer_unpolled_when_the_store_drops() {
    let (mut store, instance) = instantiate().await;
    let counts = Arc::new(Counts::default());
    let counted = || Counted(counts.clone());

    // A stream and a future the host created and holds.
    let held_stream = StreamReader::<u8>::new(&mut store.as_context_mut(), counted())
        .expect("the host creates a stream");
    let held_future =
        FutureReader::<u32>::new(&mut store.as_context_mut(), counted_future(counted()))
            .expect("the host creates a future");
    // A stream and a future the guest created, piped to consumers.
    make(&mut store, &instance)
        .await
        .pipe(&mut store.as_context_mut(), counted())
        .expect("the host pipes the guest's stream");
    make_future(&mut store, &instance)
        .await
        .pipe(&mut store.as_context_mut(), counted())
        .expect("the host pipes the guest's future");
    // A stream the host created and piped to itself: a host task.
    StreamReader::<u8>::new(&mut store.as_context_mut(), counted())
        .expect("the host creates a stream")
        .pipe(&mut store.as_context_mut(), counted())
        .expect("the host pipes its own stream");
    // A waker kept for a host end.
    let idle = Arc::new(Idle::default());
    let writer = writer_of(&store, StreamReaderInternal::end(&held_stream));
    store
        .internal()
        .scheduler_mut()
        .set_host_end_waker(writer, Waker::from(idle.clone()));

    assert_eq!(
        record_counts(&store),
        (10, 5),
        "five streams and futures, each with two ends"
    );
    assert_eq!(store.internal().scheduler().host_task_count(), 1);
    assert_eq!((counts.polls(), counts.drops()), (0, 0));
    let tables = tables_of(&mut store);

    drop(store);

    assert_eq!(
        counts.drops(),
        6,
        "every producer and consumer, held or piped, drops with the store"
    );
    assert_eq!(counts.polls(), 0, "none of them is polled on the way out");
    // The records and the ends went with the store above. The tables
    // that held them are held by the store's host functions too, and
    // in the browser the runtime layer keeps the store for a guest call
    // that has handed over its results and that the browser has yet to
    // settle. Those drop once the browser settles the call.
    #[cfg(target_arch = "wasm32")]
    settle_until(|| tables.upgrade().is_none()).await;
    assert!(
        tables.upgrade().is_none(),
        "every shared record and every end drops with the store"
    );
    assert_eq!(
        Arc::strong_count(&idle),
        1,
        "the waker kept for a host end drops with the store"
    );
    assert_eq!(
        idle.wakes.load(Ordering::SeqCst),
        0,
        "the waker is let go of, not woken"
    );
    drop((held_stream, held_future, instance));
}

#[wcmp_macros::test]
async fn it_closes_nothing_when_a_copy_closes_an_end_another_copy_closed() {
    let (mut store, instance) = instantiate().await;
    let mut first = make_any(&mut store, &instance).await;
    let mut second = first.clone();
    assert_eq!(write(&mut store, &instance).await, BLOCKED);

    first
        .close(&mut store.as_context_mut())
        .expect("the first copy closes the stream");
    let writable = call_u32(&mut store, &instance, "writable", &[]).await;
    assert_eq!(
        poll_event(&mut store, &instance, "writable").await,
        (STREAM_WRITE, writable, DROPPED)
    );

    second
        .close(&mut store.as_context_mut())
        .expect("Wasmtime's close finds the end in its table, and drops it again");
    assert!(
        !any_event(&mut store, &instance, "writable").await,
        "the writer is told of the drop once"
    );
    assert_eq!(
        record_counts(&store),
        (2, 1),
        "the records still wait for the guest's drop"
    );

    let again = second
        .close(&mut store.as_context_mut())
        .expect_err("the copy that closed names no end");
    assert!(not_present(&again, EndKind::StreamReadable), "{again:?}");
    call_ok(&mut store, &instance, "drop-writable", &[]).await;
    assert_eq!(record_counts(&store), (0, 0));
}

#[wcmp_macros::test]
async fn it_closes_nothing_when_the_host_closes_an_end_the_guest_dropped() {
    let (mut store, instance) = instantiate().await;
    let lowered = make_any(&mut store, &instance).await;
    let mut kept = lowered.clone();
    let index = take(&mut store, &instance, &lowered)
        .await
        .expect("the first copy lowers the end into the guest");
    call_ok(&mut store, &instance, "drop-readable", &[index]).await;

    kept.close(&mut store.as_context_mut())
        .expect("Wasmtime's guest drop and host close are the same drop");
    assert_eq!(
        write(&mut store, &instance).await,
        DROPPED,
        "the guest's drop told the writer"
    );
    assert_eq!(record_counts(&store), (2, 1));
}

#[wcmp_macros::test]
async fn it_refuses_every_use_of_a_copy_once_another_copy_closed_a_stream_the_host_created() {
    let (mut store, instance) = instantiate().await;
    let counts = Arc::new(Counts::default());
    let mut first = StreamReader::<u8>::new(&mut store.as_context_mut(), Counted(counts.clone()))
        .expect("the host creates a stream")
        .try_into_stream_any(&mut store.as_context_mut())
        .expect("the host holds the end it created");
    let second = first.clone();
    first
        .close(&mut store.as_context_mut())
        .expect("the host closes its own stream");
    assert_eq!(
        record_counts(&store),
        (0, 0),
        "the close took both ends, as Wasmtime's deletes the stream"
    );

    let lowered = take(&mut store, &instance, &second)
        .await
        .expect_err("no end is left to lower");
    assert!(
        invalid_handle(&lowered, "resource not present"),
        "{lowered:?}"
    );
    let piped =
        pipe(&mut store, &second, Counted(counts.clone())).expect_err("no end is left to pipe");
    assert!(not_present(&piped, EndKind::StreamReadable), "{piped:?}");
    let converted = StreamReader::<u8>::try_from_stream_any(second.clone())
        .expect("the payload type still matches")
        .try_into_stream_any(&mut store.as_context_mut())
        .expect_err("no end is left to convert");
    assert!(
        not_present(&converted, EndKind::StreamReadable),
        "{converted:?}"
    );
    let closed = close_copy(&mut store, &second).expect_err("no end is left to close");
    assert!(not_present(&closed, EndKind::StreamReadable), "{closed:?}");
    assert_eq!(
        closed.to_string(),
        "copy error: resource not present",
        "the cause carries Wasmtime's message"
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_copy_the_lower_or_pipe_of_an_end_another_copy_closed() {
    let (mut store, instance) = instantiate().await;
    let mut first = make_any(&mut store, &instance).await;
    let second = first.clone();
    first
        .close(&mut store.as_context_mut())
        .expect("the first copy closes the stream");

    let counts = Arc::new(Counts::default());
    let piped = pipe(&mut store, &second, Counted(counts.clone()))
        .expect_err("a dropped end does not open again for a consumer");
    assert!(not_held(&piped, EndKind::StreamReadable), "{piped:?}");
    assert_eq!(
        (counts.polls(), counts.drops()),
        (0, 1),
        "the refused consumer is dropped unpolled"
    );
    StreamReader::<u8>::try_from_stream_any(second.clone())
        .expect("the payload type still matches")
        .try_into_stream_any(&mut store.as_context_mut())
        .expect("a conversion checks only that the end is in the store");
    assert_eq!(
        write(&mut store, &instance).await,
        DROPPED,
        "the writer still sees the close"
    );

    // The refused lower is a trap of the call, and a trap poisons the
    // store, so it is the last guest entry.
    let lowered = take(&mut store, &instance, &second)
        .await
        .expect_err("a dropped end enters no guest's table");
    assert!(invalid_handle(&lowered, NOT_HELD), "{lowered:?}");
}

#[wcmp_macros::test]
async fn it_refuses_a_copy_every_use_but_conversion_of_an_end_another_copy_lowered() {
    let (mut store, instance) = instantiate().await;
    let lowered = make_any(&mut store, &instance).await;
    let kept = lowered.clone();
    take(&mut store, &instance, &lowered)
        .await
        .expect("the first copy lowers the end into the guest");

    let again = take(&mut store, &instance, &kept)
        .await
        .expect_err("the guest's table holds the end");
    assert!(invalid_handle(&again, NOT_HELD), "{again:?}");
    let piped = pipe(&mut store, &kept, Counted(Arc::new(Counts::default())))
        .expect_err("the guest reads the end");
    assert!(not_held(&piped, EndKind::StreamReadable), "{piped:?}");
    let closed = close_copy(&mut store, &kept).expect_err("the guest holds the end");
    assert!(not_held(&closed, EndKind::StreamReadable), "{closed:?}");
    StreamReader::<u8>::try_from_stream_any(kept)
        .expect("the payload type still matches")
        .try_into_stream_any(&mut store.as_context_mut())
        .expect("a conversion checks only that the end is in the store");
}

#[wcmp_macros::test]
async fn it_gives_every_copy_back_an_end_the_guest_returns() {
    let (mut store, instance) = instantiate().await;
    let lowered = make_any(&mut store, &instance).await;
    let mut kept = lowered.clone();
    let index = take(&mut store, &instance, &lowered)
        .await
        .expect("the first copy lowers the end into the guest");
    let returned = match call(&mut store, &instance, "give", &[index]).await {
        Ok(Some(Val::Stream(stream))) => stream,
        other => panic!("`give` answered {other:?}"),
    };
    assert_eq!(returned, kept, "the end comes back under its identity");

    assert_eq!(write(&mut store, &instance).await, BLOCKED);
    kept.close(&mut store.as_context_mut())
        .expect("the host holds the end again, through every copy");
    let writable = call_u32(&mut store, &instance, "writable", &[]).await;
    assert_eq!(
        poll_event(&mut store, &instance, "writable").await,
        (STREAM_WRITE, writable, DROPPED)
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_copy_the_lower_of_an_end_another_copy_piped() {
    let (mut store, instance) = instantiate().await;
    let piped = make_any(&mut store, &instance).await;
    let kept = piped.clone();
    let counts = Arc::new(Counts::default());
    pipe(&mut store, &piped, Counted(counts.clone())).expect("the first copy pipes the end");

    StreamReader::<u8>::try_from_stream_any(kept.clone())
        .expect("the payload type still matches")
        .try_into_stream_any(&mut store.as_context_mut())
        .expect("a conversion checks only that the end is in the store");
    assert_eq!(write(&mut store, &instance).await, BLOCKED);
    assert!(
        counts.polls() > 0 && counts.drops() == 0,
        "the first consumer serves the writer"
    );

    // The refused lower is a trap of the call, and a trap poisons the
    // store, so it is the last guest entry. The poisoned store drops
    // the consumer with the rest of its host work.
    let lowered = take(&mut store, &instance, &kept)
        .await
        .expect_err("a consumer reads the end");
    assert!(invalid_handle(&lowered, NOT_HELD), "{lowered:?}");
    assert_eq!(counts.drops(), 1, "the trap dropped the first consumer");
}

#[wcmp_macros::test]
async fn it_refuses_a_copy_the_pipe_or_close_of_an_end_whose_consumer_serves_a_write() {
    let (mut store, instance) = instantiate().await;
    let piped = make_any(&mut store, &instance).await;
    let kept = piped.clone();
    let counts = Arc::new(Counts::default());
    pipe(&mut store, &piped, Counted(counts.clone())).expect("the first copy pipes the end");
    assert_eq!(
        write(&mut store, &instance).await,
        BLOCKED,
        "the consumer holds the write in flight"
    );

    let refused = Arc::new(Counts::default());
    let again =
        pipe(&mut store, &kept, Counted(refused.clone())).expect_err("a write is in flight");
    assert!(not_held(&again, EndKind::StreamReadable), "{again:?}");
    let closed = close_copy(&mut store, &kept).expect_err("a write is in flight");
    assert!(not_held(&closed, EndKind::StreamReadable), "{closed:?}");

    assert_eq!(
        (refused.polls(), refused.drops()),
        (0, 1),
        "only the refused consumer is dropped"
    );
    assert!(
        counts.polls() > 0 && counts.drops() == 0,
        "the first consumer still serves the write"
    );
    assert!(
        !any_event(&mut store, &instance, "writable").await,
        "the write is still in flight"
    );
}

#[wcmp_macros::test]
async fn it_replaces_the_consumer_of_an_end_another_copy_piped_with_no_write_in_flight() {
    let (mut store, instance) = instantiate().await;
    let piped = make_any(&mut store, &instance).await;
    let kept = piped.clone();
    let first = Arc::new(Counts::default());
    pipe(&mut store, &piped, Taking(Counted(first.clone()))).expect("the first copy pipes");
    assert_eq!(
        write(&mut store, &instance).await,
        4 << 4,
        "the first consumer takes the four bytes at once, so no write is in flight"
    );
    assert_eq!(first.polls(), 1);

    let second = Arc::new(Counts::default());
    pipe(&mut store, &kept, Counted(second.clone()))
        .expect("a second pipe with no write in flight replaces the consumer");
    assert_eq!(
        (first.polls(), first.drops()),
        (1, 1),
        "the replaced consumer is dropped without another poll"
    );
    assert_eq!((second.polls(), second.drops()), (0, 0));

    assert_eq!(write(&mut store, &instance).await, BLOCKED);
    assert!(
        second.polls() > 0 && second.drops() == 0,
        "the next write polls the new consumer"
    );
    assert_eq!(
        first.polls(),
        1,
        "the replaced consumer is never polled again"
    );
    assert_eq!(
        record_counts(&store),
        (2, 1),
        "the stream is still open between the guest and the new consumer"
    );
}

#[wcmp_macros::test]
async fn it_replaces_the_consumer_of_a_future_another_copy_piped_with_no_write_in_flight() {
    let (mut store, instance) = instantiate().await;
    let piped = make_future(&mut store, &instance)
        .await
        .try_into_future_any(&mut store.as_context_mut())
        .expect("the host holds the end the guest handed it");
    let first = Arc::new(Counts::default());
    FutureReader::<u32>::try_from_future_any(piped.clone())
        .expect("the future carries `u32`")
        .pipe(&mut store.as_context_mut(), Counted(first.clone()))
        .expect("the first copy pipes");

    let second = Arc::new(Counts::default());
    FutureReader::<u32>::try_from_future_any(piped)
        .expect("the future carries `u32`")
        .pipe(&mut store.as_context_mut(), Counted(second.clone()))
        .expect("a second pipe with no write in flight replaces the consumer");
    assert_eq!(
        (first.polls(), first.drops()),
        (0, 1),
        "the replaced consumer is dropped unpolled"
    );

    assert_eq!(future_write(&mut store, &instance).await, BLOCKED);
    assert!(
        second.polls() > 0 && second.drops() == 0,
        "the write polls the new consumer"
    );
    assert_eq!(first.polls(), 0, "the replaced consumer is never polled");
}

#[wcmp_macros::test]
async fn it_closes_an_end_another_copy_piped_with_no_write_in_flight() {
    let (mut store, instance) = instantiate().await;
    let piped = make_any(&mut store, &instance).await;
    let kept = piped.clone();
    let counts = Arc::new(Counts::default());
    pipe(&mut store, &piped, Counted(counts.clone())).expect("the first copy pipes the end");

    close_copy(&mut store, &kept).expect("a close with no write in flight drops the consumer");
    assert_eq!(
        (counts.polls(), counts.drops()),
        (0, 1),
        "the consumer is dropped unpolled"
    );

    let writable = call_u32(&mut store, &instance, "writable", &[]).await;
    assert_eq!(
        poll_event(&mut store, &instance, "writable").await,
        (STREAM_WRITE, writable, DROPPED),
        "the idle writer is given the dropped result"
    );
    close_copy(&mut store, &kept).expect("a close of an end closed already does nothing");
    call_ok(&mut store, &instance, "drop-writable", &[]).await;
    assert_eq!(
        record_counts(&store),
        (0, 0),
        "the guest's drop is the second of the pair"
    );
}

#[wcmp_macros::test]
async fn it_closes_a_future_another_copy_piped_with_no_write_in_flight() {
    let (mut store, instance) = instantiate().await;
    let mut piped = make_future(&mut store, &instance)
        .await
        .try_into_future_any(&mut store.as_context_mut())
        .expect("the host holds the end the guest handed it");
    let counts = Arc::new(Counts::default());
    FutureReader::<u32>::try_from_future_any(piped.clone())
        .expect("the future carries `u32`")
        .pipe(&mut store.as_context_mut(), Counted(counts.clone()))
        .expect("the first copy pipes");

    piped
        .close(&mut store.as_context_mut())
        .expect("a close with no write in flight drops the consumer");
    assert_eq!(
        (counts.polls(), counts.drops()),
        (0, 1),
        "the consumer is dropped unpolled"
    );

    let writable = call_u32(&mut store, &instance, "future-writable", &[]).await;
    assert_eq!(
        poll_event(&mut store, &instance, "future-writable").await,
        (FUTURE_WRITE, writable, DROPPED),
        "the idle writer is given the dropped result"
    );
    call_ok(&mut store, &instance, "future-drop-writable", &[]).await;
    assert_eq!(
        record_counts(&store),
        (0, 0),
        "a writable future end told of the drop is done, and drops cleanly"
    );
}

#[wcmp_macros::test]
async fn it_refuses_every_copy_of_a_stream_the_host_created_and_piped_to_itself() {
    let (mut store, instance) = instantiate().await;
    let counts = Arc::new(Counts::default());
    let piped = StreamReader::<u8>::new(&mut store.as_context_mut(), Counted(counts.clone()))
        .expect("the host creates a stream")
        .try_into_stream_any(&mut store.as_context_mut())
        .expect("the host holds the end it created");
    let kept = piped.clone();
    pipe(&mut store, &piped, Counted(counts.clone())).expect("the host pipes its own stream");

    let refused = Arc::new(Counts::default());
    let again =
        pipe(&mut store, &kept, Counted(refused.clone())).expect_err("the end has a consumer");
    assert!(not_held(&again, EndKind::StreamReadable), "{again:?}");
    let closed = close_copy(&mut store, &kept).expect_err("the host's pipe reads the end");
    assert!(not_held(&closed, EndKind::StreamReadable), "{closed:?}");

    assert_eq!(refused.drops(), 1, "only the refused consumer is dropped");
    assert_eq!(
        counts.drops(),
        0,
        "the pipe keeps its producer and consumer"
    );
    assert_eq!(store.internal().scheduler().host_task_count(), 1);

    // The refused lower is a trap of the call, and a trap poisons the
    // store, which drops the pipe's producer and consumer with the
    // host task that runs them.
    let lowered = take(&mut store, &instance, &kept)
        .await
        .expect_err("the host's pipe reads the end");
    assert!(invalid_handle(&lowered, NOT_HELD), "{lowered:?}");
    assert_eq!(
        counts.drops(),
        2,
        "the trap dropped the pipe's producer and consumer"
    );
    assert_eq!(store.internal().scheduler().host_task_count(), 0);
}

#[wcmp_macros::test]
async fn it_leaves_a_value_naming_no_end_after_a_refused_close() {
    let (mut store, instance) = instantiate().await;
    let lowered = make_any(&mut store, &instance).await;
    let mut kept = lowered.clone();
    take(&mut store, &instance, &lowered)
        .await
        .expect("the first copy lowers the end into the guest");

    let refused = kept
        .close(&mut store.as_context_mut())
        .expect_err("the guest holds the end");
    assert!(not_held(&refused, EndKind::StreamReadable), "{refused:?}");
    let again = kept
        .close(&mut store.as_context_mut())
        .expect_err("Wasmtime's close replaces the id before it looks the end up");
    assert!(not_present(&again, EndKind::StreamReadable), "{again:?}");
}

#[wcmp_macros::test]
async fn it_closes_a_stream_through_an_accessor_inside_a_poll() {
    let (mut store, instance) = instantiate().await;
    let mut reader = make(&mut store, &instance).await;
    assert_eq!(write(&mut store, &instance).await, BLOCKED);

    store
        .run_concurrent(async move |accessor| reader.close_with(accessor))
        .await
        .expect("run the closure")
        .expect("the close reaches the store");

    let writable = call_u32(&mut store, &instance, "writable", &[]).await;
    assert_eq!(
        poll_event(&mut store, &instance, "writable").await,
        (STREAM_WRITE, writable, DROPPED)
    );
}

#[wcmp_macros::test]
async fn it_closes_a_guarded_future_reader_that_drops_inside_a_poll() {
    let (mut store, instance) = instantiate().await;
    let reader = make_future(&mut store, &instance).await;
    assert_eq!(future_write(&mut store, &instance).await, BLOCKED);

    store
        .run_concurrent(async move |accessor| {
            drop(GuardedFutureReader::new(accessor.clone(), reader));
        })
        .await
        .expect("run the closure");

    let writable = call_u32(&mut store, &instance, "future-writable", &[]).await;
    assert_eq!(
        poll_event(&mut store, &instance, "future-writable").await,
        (FUTURE_WRITE, writable, DROPPED),
        "the guard's drop closed the future"
    );
}

#[wcmp_macros::test]
async fn it_gives_the_future_reader_back_from_a_guard_without_closing() {
    let (mut store, instance) = instantiate().await;
    let reader = make_future(&mut store, &instance).await;
    assert_eq!(future_write(&mut store, &instance).await, BLOCKED);

    let mut reader = store
        .run_concurrent(async move |accessor| reader.guard(accessor.clone()).into_future())
        .await
        .expect("run the closure");
    assert!(
        !any_event(&mut store, &instance, "future-writable").await,
        "a guard that gave its reader back closed nothing"
    );

    reader
        .close(&mut store.as_context_mut())
        .expect("the reader given back closes");
    let writable = call_u32(&mut store, &instance, "future-writable", &[]).await;
    assert_eq!(
        poll_event(&mut store, &instance, "future-writable").await,
        (FUTURE_WRITE, writable, DROPPED)
    );
}

#[wcmp_macros::test]
async fn it_leaks_the_end_of_a_guarded_future_reader_that_drops_outside_a_poll() {
    let (mut store, instance) = instantiate().await;
    let reader = make_future(&mut store, &instance).await;
    assert_eq!(future_write(&mut store, &instance).await, BLOCKED);

    let guard = store
        .run_concurrent(async move |accessor| reader.guard(accessor.clone()))
        .await
        .expect("run the closure");
    drop(guard);

    assert!(
        !any_event(&mut store, &instance, "future-writable").await,
        "a guard dropped outside a poll cannot reach the store"
    );
    assert_eq!(record_counts(&store), (2, 1), "its end leaks");
}

/// Let the browser run microtasks until `done` answers `true`, or give
/// up after enough of them for any call the browser runs to have
/// settled.
#[cfg(target_arch = "wasm32")]
async fn settle_until(done: impl Fn() -> bool) {
    for _ in 0..64 {
        if done() {
            return;
        }
        wasm_bindgen_futures::JsFuture::from(js_sys::Promise::resolve(
            &wasm_bindgen::JsValue::UNDEFINED,
        ))
        .await
        .expect("a resolved promise");
    }
}
