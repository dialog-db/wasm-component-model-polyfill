//! Baseline tests for a stream and a future the host writes through a
//! producer.
//!
//! A host creates a stream with `StreamReader::new` and a future with
//! `FutureReader::new`, each over a producer, and hands the reader to
//! a guest: as an argument of a typed call, or as the result of a
//! typed host function. The guest's read polls the producer inside a
//! turn of the store. A poll that is ready is delivered before the
//! read returns, so the guest sees the result and not the blocked
//! sentinel. A pending poll leaves the read to a later turn, which
//! polls the producer again and fills the end's event once it is
//! ready. Items beyond what the read can take stay with the end for
//! the next read, a zero-length read reaches the producer as a
//! destination with no capacity, and a producer's error fails the
//! guest's built-in.
//!
//! Most tests drive one component whose synchronous exports each call
//! one built-in, so a test starts a read, polls a set for its event,
//! and reads what arrived in the component's memory, one call at a
//! time. Each call is a driver of the store, and the turns it runs
//! are the ones that poll the producer. The failure of a later poll
//! is proved through a callback export instead, whose task waits for
//! the read and is the task the failure traps.

#![cfg(test)]

use core::pin::Pin;
use core::task::{Context, Poll};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use wasm_component_model_polyfill::{
    Accessor, Component, ComponentValue, CopyCause, Destination, Engine, EngineConfig, Error,
    FutureProducer, FutureReader, HostCall, Instance, Linker, Store, StoreContext, StreamProducer,
    StreamReader, StreamResult, Val,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// The word a copy returns when it has not finished.
const BLOCKED: u32 = 0xffff_ffff;

/// The code a poll of a set that holds no event delivers.
const EVENT_NONE: u32 = 0;

/// The code of the event a read on a stream end delivers.
const STREAM_READ: u32 = 2;

/// The code of the event a read on a future end delivers.
const FUTURE_READ: u32 = 4;

/// The result a completed copy packs into its low four bits.
const COMPLETED: u32 = 0;

/// The result a copy that found the other end dropped packs.
const DROPPED: u32 = 1;

/// The most calls a test makes while it waits for a later turn to
/// fill an event. Each call is a driver that runs turns, so an event
/// one or two turns away lands well inside it.
const CALLS_UNTIL_AN_EVENT: usize = 8;

/// A component whose synchronous exports take a `stream<u8>`, a
/// `stream<string>`, and a `future<u32>` from the host and read them,
/// one built-in per call.
///
/// `take`, `take-strings`, and `take-future` return the index the end
/// they are given took in the component's table. `read`,
/// `read-strings`, and `future-read` start an asynchronous read; a
/// read of strings allocates each string's bytes through the
/// component's `cabi_realloc`, a bump allocator that starts at 4096.
/// `read-sync` reads synchronously from a synchronous export, and
/// `read-sync-async` does the same from an `async` export with a
/// callback and returns the packed result through `task.return`.
/// `poll` polls a set and writes the event it delivers at address 0:
/// the end's index there and the packed result at address 4. `peek`
/// reads a word of memory. `open` calls the imported host function
/// `open`, which returns a stream, and returns the index of the end
/// it was given; `open-later` does the same through `open-later`, an
/// `async` import lowered synchronously.
///
/// `take-list` takes a `list<stream<u8>>`, which reaches it in
/// memory, and returns the address of the list's elements, each the
/// index of an end. `take-list-async` does the same as an `async`
/// export with a callback, and returns the address through
/// `task.return`.
const READS_HOST_ENDS: &[u8] = component!(
    r#"
    (component
      (type $s (stream u8))
      (type $t (stream string))
      (type $f (future u32))
      (import "open" (func $open (result $s)))
      (import "open-later" (func $open-later async (result $s)))
      (core module $libc
        (memory (export "memory") 1)
        (global $bump (mut i32) (i32.const 4096))
        (func (export "cabi_realloc")
              (param $old i32) (param $old-size i32) (param $align i32) (param $size i32)
              (result i32)
          (local $ptr i32)
          global.get $bump local.get $align i32.add i32.const 1 i32.sub
          local.get $align i32.const 1 i32.sub i32.const -1 i32.xor i32.and
          local.set $ptr
          local.get $ptr local.get $size i32.add global.set $bump
          local.get $ptr)
        (func (export "peek") (param i32) (result i32) (i32.load (local.get 0))))
      (core instance $libc (instantiate $libc))

      (core func $read (canon stream.read $s async (memory (core memory $libc "memory"))))
      (core func $read-sync (canon stream.read $s (memory (core memory $libc "memory"))))
      (core func $read-strings
        (canon stream.read $t async (memory (core memory $libc "memory"))
          (realloc (core func $libc "cabi_realloc"))))
      (core func $future-read
        (canon future.read $f async (memory (core memory $libc "memory"))))
      (core func $drop-readable (canon stream.drop-readable $s))
      (core func $open (canon lower (func $open)))
      (core func $open-later (canon lower (func $open-later)))
      (core func $set-new (canon waitable-set.new))
      (core func $poll (canon waitable-set.poll (memory (core memory $libc "memory"))))
      (core func $join (canon waitable.join))
      (core func $task-return (canon task.return (result u32)))
      (core func $cancel-read (canon stream.cancel-read $s async))
      (core func $cancel-read-sync (canon stream.cancel-read $s))
      (core func $future-cancel-read (canon future.cancel-read $f async))

      (core module $m
        (import "" "stream.read" (func $read (param i32 i32 i32) (result i32)))
        (import "" "stream.read-sync" (func $read-sync (param i32 i32 i32) (result i32)))
        (import "" "stream.read-strings" (func $read-strings (param i32 i32 i32) (result i32)))
        (import "" "future.read" (func $future-read (param i32 i32) (result i32)))
        (import "" "stream.drop-readable" (func $drop-readable (param i32)))
        (import "" "open" (func $open (result i32)))
        (import "" "open-later" (func $open-later (result i32)))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (import "" "waitable-set.poll" (func $poll (param i32 i32) (result i32)))
        (import "" "waitable.join" (func $join (param i32 i32)))
        (import "" "task.return" (func $task-return (param i32)))
        (import "" "stream.cancel-read" (func $cancel-read (param i32) (result i32)))
        (import "" "stream.cancel-read-sync" (func $cancel-read-sync (param i32) (result i32)))
        (import "" "future.cancel-read" (func $future-cancel-read (param i32) (result i32)))
        (func (export "take") (param i32) (result i32) (local.get 0))
        (func (export "take-list") (param i32 i32) (result i32) (local.get 0))
        (func (export "take-list-async") (param i32 i32) (result i32)
          (call $task-return (local.get 0))
          (i32.const 0))
        (func (export "take-list-async-callback") (param i32 i32 i32) (result i32)
          (unreachable))
        (func (export "read") (param i32 i32 i32) (result i32)
          (call $read (local.get 0) (local.get 1) (local.get 2)))
        (func (export "read-sync") (param i32 i32 i32) (result i32)
          (call $read-sync (local.get 0) (local.get 1) (local.get 2)))
        (func (export "read-sync-async") (param i32 i32 i32) (result i32)
          (call $task-return (call $read-sync (local.get 0) (local.get 1) (local.get 2)))
          (i32.const 0))
        (func (export "read-strings") (param i32 i32 i32) (result i32)
          (call $read-strings (local.get 0) (local.get 1) (local.get 2)))
        (func (export "future-read") (param i32 i32) (result i32)
          (call $future-read (local.get 0) (local.get 1)))
        (func (export "drop-readable") (param i32) (call $drop-readable (local.get 0)))
        (func (export "cancel-read") (param i32) (result i32) (call $cancel-read (local.get 0)))
        (func (export "cancel-read-sync-async") (param i32) (result i32)
          (call $task-return (call $cancel-read-sync (local.get 0)))
          (i32.const 0))
        (func (export "cancel-read-sync") (param i32) (result i32)
          (call $cancel-read-sync (local.get 0)))
        (func (export "future-cancel-read") (param i32) (result i32)
          (call $future-cancel-read (local.get 0)))
        (func (export "open") (result i32) (call $open))
        (func (export "open-later") (result i32) (call $open-later))
        (func (export "new-set") (result i32) (call $set-new))
        (func (export "poll") (param i32) (result i32) (call $poll (local.get 0) (i32.const 0)))
        (func (export "join") (param i32 i32) (call $join (local.get 0) (local.get 1))))
      (core instance $m (instantiate $m (with "" (instance
        (export "stream.read" (func $read))
        (export "stream.read-sync" (func $read-sync))
        (export "stream.read-strings" (func $read-strings))
        (export "future.read" (func $future-read))
        (export "stream.drop-readable" (func $drop-readable))
        (export "open" (func $open))
        (export "open-later" (func $open-later))
        (export "waitable-set.new" (func $set-new))
        (export "waitable-set.poll" (func $poll))
        (export "waitable.join" (func $join))
        (export "task.return" (func $task-return))
        (export "stream.cancel-read" (func $cancel-read))
        (export "stream.cancel-read-sync" (func $cancel-read-sync))
        (export "future.cancel-read" (func $future-cancel-read))))))

      (func (export "take") (param "s" $s) (result u32) (canon lift (core func $m "take")))
      (func (export "take-strings") (param "s" $t) (result u32)
        (canon lift (core func $m "take")))
      (func (export "take-future") (param "f" $f) (result u32)
        (canon lift (core func $m "take")))
      (func (export "take-list") (param "l" (list $s)) (result u32)
        (canon lift (core func $m "take-list") (memory (core memory $libc "memory"))
          (realloc (core func $libc "cabi_realloc"))))
      (func (export "take-list-async") async (param "l" (list $s)) (result u32)
        (canon lift (core func $m "take-list-async") async (memory (core memory $libc "memory"))
          (realloc (core func $libc "cabi_realloc"))
          (callback (core func $m "take-list-async-callback"))))
      (func (export "read") (param "e" u32) (param "p" u32) (param "n" u32) (result u32)
        (canon lift (core func $m "read")))
      (func (export "read-sync") (param "e" u32) (param "p" u32) (param "n" u32) (result u32)
        (canon lift (core func $m "read-sync")))
      (func (export "read-sync-async") async (param "e" u32) (param "p" u32) (param "n" u32)
        (result u32)
        (canon lift (core func $m "read-sync-async") async
          (callback (core func $m "take-list-async-callback"))))
      (func (export "read-strings") (param "e" u32) (param "p" u32) (param "n" u32) (result u32)
        (canon lift (core func $m "read-strings")))
      (func (export "future-read") (param "e" u32) (param "p" u32) (result u32)
        (canon lift (core func $m "future-read")))
      (func (export "drop-readable") (param "e" u32) (canon lift (core func $m "drop-readable")))
      (func (export "cancel-read") (param "e" u32) (result u32)
        (canon lift (core func $m "cancel-read")))
      (func (export "cancel-read-sync-async") async (param "e" u32) (result u32)
        (canon lift (core func $m "cancel-read-sync-async") async
          (callback (core func $m "take-list-async-callback"))))
      (func (export "cancel-read-sync") (param "e" u32) (result u32)
        (canon lift (core func $m "cancel-read-sync")))
      (func (export "future-cancel-read") (param "e" u32) (result u32)
        (canon lift (core func $m "future-cancel-read")))
      (func (export "open") (result u32) (canon lift (core func $m "open")))
      (func (export "open-later") (result u32) (canon lift (core func $m "open-later")))
      (func (export "new-set") (result u32) (canon lift (core func $m "new-set")))
      (func (export "poll") (param "s" u32) (result u32) (canon lift (core func $m "poll")))
      (func (export "join") (param "w" u32) (param "s" u32) (canon lift (core func $m "join")))
      (func (export "peek") (param "p" u32) (result u32) (canon lift (core func $libc "peek"))))
    "#
);

/// A component whose `drain` export takes a `stream<u8>` and reads up
/// to four bytes of it, at address 0, and returns the packed result of
/// the read.
///
/// `drain` is lifted `async` with a callback. A read that completes
/// at once returns its result straight away. A read that blocks joins
/// the end to a set and waits on it, and the callback returns the
/// packed result the event carries. `peek` reads a word of memory.
const DRAINS_A_STREAM: &[u8] = component!(
    r#"
    (component
      (type $s (stream u8))
      (core module $libc
        (memory (export "memory") 1)
        (func (export "peek") (param i32) (result i32) (i32.load (local.get 0))))
      (core instance $libc (instantiate $libc))
      (core func $read (canon stream.read $s async (memory (core memory $libc "memory"))))
      (core func $task-return (canon task.return (result u32)))
      (core func $set-new (canon waitable-set.new))
      (core func $join (canon waitable.join))
      (core module $m
        (import "" "stream.read" (func $read (param i32 i32 i32) (result i32)))
        (import "" "task.return" (func $task-return (param i32)))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (import "" "waitable.join" (func $join (param i32 i32)))
        (func (export "drain") (param $end i32) (result i32)
          (local $result i32)
          (local $set i32)
          (local.set $result (call $read (local.get $end) (i32.const 0) (i32.const 4)))
          (if (i32.ne (local.get $result) (i32.const -1))
            (then
              (call $task-return (local.get $result))
              (return (i32.const 0))))
          (local.set $set (call $set-new))
          (call $join (local.get $end) (local.get $set))
          (i32.or (i32.shl (local.get $set) (i32.const 4)) (i32.const 2)))
        (func (export "drain-callback") (param i32 i32 i32) (result i32)
          (call $task-return (local.get 2))
          (i32.const 0)))
      (core instance $m (instantiate $m (with "" (instance
        (export "stream.read" (func $read))
        (export "task.return" (func $task-return))
        (export "waitable-set.new" (func $set-new))
        (export "waitable.join" (func $join))))))
      (func (export "drain") async (param "s" $s) (result u32)
        (canon lift (core func $m "drain") async (callback (core func $m "drain-callback"))))
      (func (export "peek") (param "p" u32) (result u32) (canon lift (core func $libc "peek"))))
    "#
);

/// What a producer answers on one poll.
enum Step {
    /// Answer pending, after waking the waker, so the next turn polls
    /// the producer again.
    Pend,
    /// Deliver these bytes and answer that more can follow.
    Deliver(&'static [u8]),
    /// Answer that the stream is over.
    End,
    /// Fail with this message.
    Fail(&'static str),
    /// Answer that the poll completed without delivering anything,
    /// which is how a producer answers a zero-length read at once.
    Ready,
    /// Deliver these bytes and then answer pending, which the
    /// contract forbids.
    DeliverThenPend(&'static [u8]),
    /// Answer that the read was cancelled, although the poll was not
    /// asked to finish, which the contract forbids.
    Cancel,
}

/// A producer that answers its polls with `steps`, in order, and
/// records the capacity each poll's destination offered.
struct Scripted {
    steps: VecDeque<Step>,
    seen: Arc<Mutex<Vec<Option<usize>>>>,
}

impl Scripted {
    /// A producer answering with `steps`, and the record of the
    /// capacity each of its polls was offered.
    fn new(steps: impl IntoIterator<Item = Step>) -> (Self, Arc<Mutex<Vec<Option<usize>>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let producer = Self {
            steps: steps.into_iter().collect(),
            seen: seen.clone(),
        };
        (producer, seen)
    }
}

impl StreamProducer<()> for Scripted {
    type Item = u8;

    fn poll_produce(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        _store: &mut StoreContext<'_, ()>,
        mut destination: Destination<'_, u8>,
        _finish: bool,
    ) -> Poll<Result<StreamResult, Error>> {
        let this = self.get_mut();
        this.seen
            .lock()
            .expect("the record of the polls")
            .push(destination.remaining());
        match this.steps.pop_front().unwrap_or(Step::End) {
            Step::Pend => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Step::Deliver(bytes) => {
                // The destination hands back the vector of an earlier
                // poll once the reader has taken all of it, so the
                // allocation is reused.
                let mut buffer = destination.take_buffer();
                buffer.extend_from_slice(bytes);
                destination.set_buffer(buffer);
                Poll::Ready(Ok(StreamResult::Completed))
            }
            Step::End => Poll::Ready(Ok(StreamResult::Dropped)),
            Step::Fail(message) => Poll::Ready(Err(Error::Internal {
                message: message.to_owned(),
            })),
            Step::Ready => Poll::Ready(Ok(StreamResult::Completed)),
            Step::DeliverThenPend(bytes) => {
                destination.set_buffer(bytes.to_vec());
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Step::Cancel => Poll::Ready(Ok(StreamResult::Cancelled)),
        }
    }
}

/// How many polls `seen` recorded.
fn polls(seen: &Arc<Mutex<Vec<Option<usize>>>>) -> usize {
    seen.lock().expect("the record of the polls").len()
}

/// Instantiate `bytes` into a store of its own. `open`, when the
/// component imports it, is a typed host function that creates a
/// stream over a producer delivering `hello` and returns its reader.
/// `open-later` is a typed host `async` function whose future waits
/// one poll, then creates a stream over a producer delivering `later`
/// through its accessor, and returns its reader.
async fn instantiate(bytes: &[u8]) -> (Store<()>, Instance) {
    // The `async` cancels need the more-async-builtins feature.
    let mut config = EngineConfig::new();
    config.wasm_component_model_more_async_builtins(true);
    let engine = Engine::with_config(&config).expect("engine");
    let component = Component::new(&engine, bytes)
        .await
        .expect("the component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap("open", |mut call: HostCall<'_, ()>, (): ()| {
            let (producer, _) = Scripted::new([Step::Deliver(b"hello")]);
            StreamReader::new(call.store(), producer)
        })
        .expect("register `open`");
    linker
        .root()
        .func_wrap_concurrent("open-later", |accessor: &Accessor<()>, (): ()| {
            let accessor = accessor.clone();
            async move {
                after_one_pending_poll(0).await?;
                let (producer, _) = Scripted::new([Step::Deliver(b"later")]);
                accessor.with(|store| StreamReader::new(store, producer))?
            }
        })
        .expect("register `open-later`");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
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

/// Hand `reader` to the guest's `take` through a typed call and
/// answer the index its end took in the guest's table.
async fn take(store: &mut Store<()>, instance: &Instance, reader: StreamReader<u8>) -> u32 {
    instance
        .get_func("take")
        .expect("the component exports `take`")
        .typed::<(StreamReader<u8>,), u32>()
        .expect("`take` takes a `stream<u8>`")
        .call(store, (reader,))
        .await
        .expect("the reader crosses into the guest")
}

/// A stream over `producer`, handed to the guest's `take`. Answers
/// the index its end took in the guest's table.
async fn stream_in_guest(store: &mut Store<()>, instance: &Instance, producer: Scripted) -> u32 {
    let reader = StreamReader::new(&mut store.as_context_mut(), producer).expect("a stream");
    take(store, instance, reader).await
}

/// Join `end` to a fresh set and poll the set, a call at a time, until
/// it delivers an event or [`CALLS_UNTIL_AN_EVENT`] calls have passed.
/// Answers the event as the guest reads it: the code, the end's
/// index, and the packed result.
async fn poll_until_an_event(
    store: &mut Store<()>,
    instance: &Instance,
    end: u32,
) -> (u32, u32, u32) {
    let set = call_u32(store, instance, "new-set", &[]).await;
    call_ok(store, instance, "join", &[end, set]).await;
    let mut code = EVENT_NONE;
    for _ in 0..CALLS_UNTIL_AN_EVENT {
        code = call_u32(store, instance, "poll", &[set]).await;
        if code != EVENT_NONE {
            break;
        }
    }
    let index = call_u32(store, instance, "peek", &[0]).await;
    let packed = call_u32(store, instance, "peek", &[4]).await;
    call_ok(store, instance, "join", &[end, 0]).await;
    (code, index, packed)
}

/// The bytes at `address` of the guest's memory, `count` of them.
async fn bytes_at(
    store: &mut Store<()>,
    instance: &Instance,
    address: u32,
    count: usize,
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(count + 4);
    let mut word = address;
    while bytes.len() < count {
        bytes.extend(
            call_u32(store, instance, "peek", &[word])
                .await
                .to_le_bytes(),
        );
        word += 4;
    }
    bytes.truncate(count);
    bytes
}

/// The packed result of a copy: `result` in the low four bits and
/// `count` above them.
fn packed(result: u32, count: u32) -> u32 {
    result | (count << 4)
}

#[wcmp_macros::test]
async fn it_feeds_a_guests_asynchronous_reads_across_several_turns() {
    let (mut store, instance) = instantiate(READS_HOST_ENDS).await;
    let (producer, seen) = Scripted::new([
        Step::Pend,
        Step::Deliver(b"abc"),
        Step::Pend,
        Step::Deliver(b"defg"),
        Step::Pend,
        Step::End,
    ]);
    let end = stream_in_guest(&mut store, &instance, producer).await;

    assert_eq!(
        call_u32(&mut store, &instance, "read", &[end, 100, 16]).await,
        BLOCKED,
        "the producer had nothing ready when the read polled it"
    );
    assert_eq!(
        poll_until_an_event(&mut store, &instance, end).await,
        (STREAM_READ, end, packed(COMPLETED, 3)),
        "a later turn polled the producer again and delivered its first chunk"
    );
    assert_eq!(bytes_at(&mut store, &instance, 100, 3).await, b"abc");

    assert_eq!(
        call_u32(&mut store, &instance, "read", &[end, 200, 16]).await,
        BLOCKED
    );
    assert_eq!(
        poll_until_an_event(&mut store, &instance, end).await,
        (STREAM_READ, end, packed(COMPLETED, 4)),
        "the second read got the second chunk in a later turn"
    );
    assert_eq!(bytes_at(&mut store, &instance, 200, 4).await, b"defg");

    assert_eq!(
        call_u32(&mut store, &instance, "read", &[end, 300, 16]).await,
        BLOCKED
    );
    assert_eq!(
        poll_until_an_event(&mut store, &instance, end).await,
        (STREAM_READ, end, packed(DROPPED, 0)),
        "the producer ended the stream, and the reader learned it"
    );
    assert_eq!(
        polls(&seen),
        6,
        "each read polled the producer once when it started and once in a \
         later turn"
    );
    assert!(
        seen.lock()
            .expect("the record of the polls")
            .iter()
            .all(|capacity| capacity.is_some()),
        "a guest reader offers the producer a capacity"
    );
    call_ok(&mut store, &instance, "drop-readable", &[end]).await;
}

#[wcmp_macros::test]
async fn it_keeps_items_beyond_the_guests_capacity_for_its_next_read() {
    let (mut store, instance) = instantiate(READS_HOST_ENDS).await;
    let (producer, seen) = Scripted::new([Step::Deliver(b"0123456789"), Step::Deliver(b"xy")]);
    let end = stream_in_guest(&mut store, &instance, producer).await;

    assert_eq!(
        call_u32(&mut store, &instance, "read", &[end, 100, 4]).await,
        packed(COMPLETED, 4),
        "the producer was ready, so the read completed before it returned"
    );
    assert_eq!(bytes_at(&mut store, &instance, 100, 4).await, b"0123");
    assert_eq!(
        call_u32(&mut store, &instance, "read", &[end, 200, 4]).await,
        packed(COMPLETED, 4),
        "the next read took the next four the end kept"
    );
    assert_eq!(bytes_at(&mut store, &instance, 200, 4).await, b"4567");
    assert_eq!(
        call_u32(&mut store, &instance, "read", &[end, 300, 4]).await,
        packed(COMPLETED, 2),
        "the read after it took the last two the end kept"
    );
    assert_eq!(bytes_at(&mut store, &instance, 300, 2).await, b"89");
    assert_eq!(
        polls(&seen),
        1,
        "the items the end kept satisfied the reads without a poll"
    );

    assert_eq!(
        call_u32(&mut store, &instance, "read", &[end, 400, 4]).await,
        packed(COMPLETED, 2),
        "once the end kept nothing, the next read polled the producer"
    );
    assert_eq!(bytes_at(&mut store, &instance, 400, 2).await, b"xy");
    assert_eq!(
        *seen.lock().expect("the record of the polls"),
        vec![Some(4), Some(4)],
        "each poll was offered the capacity of the read that made it"
    );
}

#[wcmp_macros::test]
async fn it_hands_a_zero_length_read_to_the_producer_with_no_capacity() {
    let (mut store, instance) = instantiate(READS_HOST_ENDS).await;
    let (producer, seen) = Scripted::new([Step::Ready, Step::Deliver(b"ok")]);
    let end = stream_in_guest(&mut store, &instance, producer).await;

    assert_eq!(
        call_u32(&mut store, &instance, "read", &[end, 100, 0]).await,
        packed(COMPLETED, 0),
        "the producer answered the readiness probe at once"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "read", &[end, 100, 8]).await,
        packed(COMPLETED, 2),
        "the read that followed took what the producer delivered"
    );
    assert_eq!(
        *seen.lock().expect("the record of the polls"),
        vec![Some(0), Some(8)],
        "the zero-length read reached the producer as a destination with no \
         capacity"
    );
}

#[wcmp_macros::test]
async fn it_waits_on_a_zero_length_read_the_producer_answers_later() {
    let (mut store, instance) = instantiate(READS_HOST_ENDS).await;
    let (producer, seen) = Scripted::new([Step::Pend, Step::Ready]);
    let end = stream_in_guest(&mut store, &instance, producer).await;

    assert_eq!(
        call_u32(&mut store, &instance, "read", &[end, 100, 0]).await,
        BLOCKED,
        "the producer waited for readiness rather than answering at once"
    );
    assert_eq!(
        poll_until_an_event(&mut store, &instance, end).await,
        (STREAM_READ, end, packed(COMPLETED, 0)),
        "the probe completed, with nothing moved, once the producer was ready"
    );
    assert_eq!(
        *seen.lock().expect("the record of the polls"),
        vec![Some(0), Some(0)]
    );
}

#[wcmp_macros::test]
async fn it_returns_a_stream_from_a_typed_host_function() {
    let (mut store, instance) = instantiate(READS_HOST_ENDS).await;
    let end = call_u32(&mut store, &instance, "open", &[]).await;

    assert_eq!(
        call_u32(&mut store, &instance, "read", &[end, 100, 8]).await,
        packed(COMPLETED, 5),
        "the guest read the stream the host function returned"
    );
    assert_eq!(bytes_at(&mut store, &instance, 100, 5).await, b"hello");
    assert_eq!(
        call_u32(&mut store, &instance, "read", &[end, 100, 8]).await,
        packed(DROPPED, 0),
        "the producer ended the stream after its one chunk"
    );
}

/// A future that is pending the first time it is polled and ready
/// with `value` afterwards. It wakes the waker before it parks, so the
/// next turn polls it again.
async fn after_one_pending_poll(value: u32) -> Result<u32, Error> {
    let mut parked = false;
    core::future::poll_fn(|cx| {
        if parked {
            return Poll::Ready(());
        }
        parked = true;
        cx.waker().wake_by_ref();
        Poll::Pending
    })
    .await;
    Ok(value)
}

#[wcmp_macros::test]
async fn it_resolves_a_guests_read_of_a_future_over_an_async_block() {
    let (mut store, instance) = instantiate(READS_HOST_ENDS).await;
    let reader = FutureReader::new(&mut store.as_context_mut(), after_one_pending_poll(42))
        .expect("a future");
    let end = instance
        .get_func("take-future")
        .expect("the component exports `take-future`")
        .typed::<(FutureReader<u32>,), u32>()
        .expect("`take-future` takes a `future<u32>`")
        .call(&mut store, (reader,))
        .await
        .expect("the reader crosses into the guest");

    assert_eq!(
        call_u32(&mut store, &instance, "future-read", &[end, 100]).await,
        BLOCKED,
        "the block was pending when the read polled it"
    );
    assert_eq!(
        poll_until_an_event(&mut store, &instance, end).await,
        (FUTURE_READ, end, packed(COMPLETED, 0)),
        "a later turn resolved the read"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "peek", &[100]).await,
        42,
        "the block's value landed in the guest's buffer"
    );
}

#[wcmp_macros::test]
async fn it_fails_the_guests_built_in_with_the_producers_error() {
    let (mut store, instance) = instantiate(READS_HOST_ENDS).await;
    let (producer, _) = Scripted::new([Step::Fail("the producer failed at once")]);
    let end = stream_in_guest(&mut store, &instance, producer).await;

    let failure = call(&mut store, &instance, "read", &[end, 100, 4])
        .await
        .expect_err("the read fails");
    assert!(
        failure.contains("the producer failed at once"),
        "the read failed with the producer's error, got {failure}"
    );
}

#[wcmp_macros::test]
async fn it_fails_the_reading_task_when_a_later_poll_fails() {
    let (mut store, instance) = instantiate(DRAINS_A_STREAM).await;
    let (producer, seen) = Scripted::new([Step::Pend, Step::Fail("the producer failed later")]);
    let reader = StreamReader::new(&mut store.as_context_mut(), producer).expect("a stream");

    let failure = instance
        .get_func("drain")
        .expect("the component exports `drain`")
        .typed::<(StreamReader<u8>,), u32>()
        .expect("`drain` takes a `stream<u8>`")
        .call(&mut store, (reader,))
        .await
        .expect_err("the call fails");

    assert!(
        chain(&failure).contains("the producer failed later"),
        "the task that started the read failed with the producer's error, got \
         {failure:?}"
    );
    assert_eq!(polls(&seen), 2, "the failure came from the second poll");
}

#[wcmp_macros::test]
async fn it_resumes_a_waiting_callback_task_with_what_the_producer_delivered() {
    let (mut store, instance) = instantiate(DRAINS_A_STREAM).await;
    let (producer, _) = Scripted::new([Step::Pend, Step::Deliver(b"wxyz")]);
    let reader = StreamReader::new(&mut store.as_context_mut(), producer).expect("a stream");

    let result = instance
        .get_func("drain")
        .expect("the component exports `drain`")
        .typed::<(StreamReader<u8>,), u32>()
        .expect("`drain` takes a `stream<u8>`")
        .call(&mut store, (reader,))
        .await
        .expect("the call returns");

    assert_eq!(
        result,
        packed(COMPLETED, 4),
        "the task waited on the read and was given its event"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "peek", &[0])
            .await
            .to_le_bytes(),
        *b"wxyz"
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_typed_reader_of_another_payload_type() {
    let (mut store, instance) = instantiate(READS_HOST_ENDS).await;

    let refused = instance
        .get_func("take")
        .expect("the component exports `take`")
        .typed::<(StreamReader<u32>,), u32>()
        .expect_err("`take` takes a `stream<u8>`, not a `stream<u32>`");
    assert!(
        matches!(refused, Error::TypeMismatch(_)),
        "the typed handle is refused with the type mismatch, got {refused:?}"
    );

    // The same reader crossing as an untyped value reaches the lower
    // itself, which checks the payload against the guest's type.
    let wide = StreamReader::<u32>::new(&mut store.as_context_mut(), NoWords).expect("a stream");
    let failure = instance
        .get_func("take")
        .expect("the component exports `take`")
        .call(&mut store, &[wide.to_val()])
        .await
        .expect_err("the lower refuses the reader");
    assert!(
        matches!(failure, Error::Copy(CopyCause::PayloadMismatch { .. })),
        "the lower refused a stream of another payload, got {failure:?}"
    );
}

/// A producer of `u32` items that never delivers any, for a stream
/// whose payload differs from the guest's.
struct NoWords;

impl StreamProducer<()> for NoWords {
    type Item = u32;

    fn poll_produce(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _store: &mut StoreContext<'_, ()>,
        _destination: Destination<'_, u32>,
        _finish: bool,
    ) -> Poll<Result<StreamResult, Error>> {
        Poll::Ready(Ok(StreamResult::Dropped))
    }
}

/// A producer that awaits a JavaScript promise for each chunk of
/// `chunks`, and delivers the chunk the promise resolves with.
#[cfg(target_arch = "wasm32")]
struct AwaitsPromises {
    chunks: VecDeque<&'static str>,
    pending: Option<wasm_bindgen_futures::JsFuture>,
    /// How many polls found the promise not yet resolved.
    pended: std::rc::Rc<core::cell::Cell<usize>>,
}

#[cfg(target_arch = "wasm32")]
impl StreamProducer<()> for AwaitsPromises {
    type Item = u8;

    fn poll_produce(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        _store: &mut StoreContext<'_, ()>,
        mut destination: Destination<'_, u8>,
        _finish: bool,
    ) -> Poll<Result<StreamResult, Error>> {
        let this = self.get_mut();
        if this.pending.is_none() {
            let Some(chunk) = this.chunks.pop_front() else {
                return Poll::Ready(Ok(StreamResult::Dropped));
            };
            let promise = js_sys::Promise::resolve(&wasm_bindgen::JsValue::from_str(chunk));
            this.pending = Some(wasm_bindgen_futures::JsFuture::from(promise));
        }
        let pending = this.pending.as_mut().expect("the promise being awaited");
        let resolved = match core::future::Future::poll(Pin::new(pending), cx) {
            Poll::Pending => {
                this.pended.set(this.pended.get() + 1);
                return Poll::Pending;
            }
            Poll::Ready(resolved) => resolved,
        };
        this.pending = None;
        let chunk = resolved
            .ok()
            .and_then(|value| value.as_string())
            .expect("the promise resolves with a string");
        destination.set_buffer(chunk.into_bytes());
        Poll::Ready(Ok(StreamResult::Completed))
    }
}

/// In the browser a producer awaits a JavaScript promise, which is
/// not `Send`: the promise resolves in the page's microtask queue, its
/// wake reaches the driver, and a later turn delivers the chunk.
#[cfg(target_arch = "wasm32")]
#[wcmp_macros::test]
async fn it_awaits_a_javascript_promise_and_gives_the_guest_its_items() {
    let (mut store, instance) = instantiate(DRAINS_A_STREAM).await;
    let pended = std::rc::Rc::new(core::cell::Cell::new(0));
    let producer = AwaitsPromises {
        chunks: VecDeque::from(["page"]),
        pending: None,
        pended: pended.clone(),
    };
    let reader = StreamReader::new(&mut store.as_context_mut(), producer).expect("a stream");

    let result = instance
        .get_func("drain")
        .expect("the component exports `drain`")
        .typed::<(StreamReader<u8>,), u32>()
        .expect("`drain` takes a `stream<u8>`")
        .call(&mut store, (reader,))
        .await
        .expect("the call returns");

    assert_eq!(
        result,
        packed(COMPLETED, 4),
        "the task waited on the read and was given the event a later turn \
         filled"
    );
    assert!(
        pended.get() > 0,
        "the promise was not resolved when the read first polled the producer"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "peek", &[0])
            .await
            .to_le_bytes(),
        *b"page",
        "the promise's chunk landed in the guest's buffer"
    );
}

/// Whether `failure`, the message chain of a failed call, carries
/// `cause`, the cause a producer that broke the contract fails with.
fn failed_with(failure: &str, cause: CopyCause) -> bool {
    let message = cause.to_string();
    failure.contains(&message.split_whitespace().collect::<Vec<_>>().join(" "))
}

#[wcmp_macros::test]
async fn it_fails_a_producer_that_answers_pending_after_delivering_items() {
    let (mut store, instance) = instantiate(READS_HOST_ENDS).await;
    let (producer, _) = Scripted::new([Step::DeliverThenPend(b"ab")]);
    let end = stream_in_guest(&mut store, &instance, producer).await;

    let failure = call(&mut store, &instance, "read", &[end, 100, 4])
        .await
        .expect_err("the read fails");
    assert!(
        failed_with(&failure, CopyCause::ProducerPendingAfterItems),
        "the read failed with the pending-after-items cause, got {failure}"
    );
}

#[wcmp_macros::test]
async fn it_fails_a_producer_that_completes_a_read_without_items() {
    let (mut store, instance) = instantiate(READS_HOST_ENDS).await;
    let (producer, _) = Scripted::new([Step::Ready]);
    let end = stream_in_guest(&mut store, &instance, producer).await;

    let failure = call(&mut store, &instance, "read", &[end, 100, 4])
        .await
        .expect_err("the read fails");
    assert!(
        failed_with(&failure, CopyCause::ProducerCompletedWithoutItems),
        "a read of four items completed with none fails, got {failure}"
    );
}

#[wcmp_macros::test]
async fn it_fails_a_stream_producer_that_cancels_a_read_it_was_not_asked_to_finish() {
    let (mut store, instance) = instantiate(READS_HOST_ENDS).await;
    let (producer, _) = Scripted::new([Step::Cancel]);
    let end = stream_in_guest(&mut store, &instance, producer).await;

    let failure = call(&mut store, &instance, "read", &[end, 100, 4])
        .await
        .expect_err("the read fails");
    assert!(
        failed_with(&failure, CopyCause::ProducerCancelledWithoutFinish),
        "the read failed with the cancelled-without-finish cause, got {failure}"
    );
}

/// A future producer that answers every poll with nothing, as if its
/// read had been cancelled.
struct Nothing;

impl FutureProducer<()> for Nothing {
    type Item = u32;

    fn poll_produce(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _store: &mut StoreContext<'_, ()>,
        _finish: bool,
    ) -> Poll<Result<Option<u32>, Error>> {
        Poll::Ready(Ok(None))
    }
}

/// Hand `reader` to the guest's `take-future` through a typed call
/// and answer the index its end took in the guest's table.
async fn future_in_guest(
    store: &mut Store<()>,
    instance: &Instance,
    reader: FutureReader<u32>,
) -> u32 {
    instance
        .get_func("take-future")
        .expect("the component exports `take-future`")
        .typed::<(FutureReader<u32>,), u32>()
        .expect("`take-future` takes a `future<u32>`")
        .call(store, (reader,))
        .await
        .expect("the reader crosses into the guest")
}

#[wcmp_macros::test]
async fn it_fails_a_future_producer_that_answers_nothing_it_was_not_asked_to_finish() {
    let (mut store, instance) = instantiate(READS_HOST_ENDS).await;
    let reader = FutureReader::new(&mut store.as_context_mut(), Nothing).expect("a future");
    let end = future_in_guest(&mut store, &instance, reader).await;

    let failure = call(&mut store, &instance, "future-read", &[end, 100])
        .await
        .expect_err("the read fails");
    assert!(
        failed_with(&failure, CopyCause::ProducerCancelledWithoutFinish),
        "the read failed with the cancelled-without-finish cause, got {failure}"
    );
}

/// A producer that answers as `inner` does and records its drop in
/// `dropped`.
struct Watched {
    inner: Scripted,
    dropped: Arc<AtomicBool>,
}

impl StreamProducer<()> for Watched {
    type Item = u8;

    fn poll_produce(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: &mut StoreContext<'_, ()>,
        destination: Destination<'_, u8>,
        finish: bool,
    ) -> Poll<Result<StreamResult, Error>> {
        Pin::new(&mut self.get_mut().inner).poll_produce(cx, store, destination, finish)
    }
}

impl Drop for Watched {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

#[wcmp_macros::test]
async fn it_drops_a_live_producer_when_the_guest_drops_its_readable_end() {
    let (mut store, instance) = instantiate(READS_HOST_ENDS).await;
    let dropped = Arc::new(AtomicBool::new(false));
    let (inner, _) = Scripted::new([Step::Deliver(b"ab"), Step::Deliver(b"cd")]);
    let producer = Watched {
        inner,
        dropped: dropped.clone(),
    };
    let reader = StreamReader::new(&mut store.as_context_mut(), producer).expect("a stream");
    let end = take(&mut store, &instance, reader).await;

    assert_eq!(
        call_u32(&mut store, &instance, "read", &[end, 100, 8]).await,
        packed(COMPLETED, 2),
        "the producer delivered its first chunk and can deliver more"
    );
    assert!(
        !dropped.load(Ordering::SeqCst),
        "the producer is live after a read it completed"
    );
    call_ok(&mut store, &instance, "drop-readable", &[end]).await;
    assert!(
        dropped.load(Ordering::SeqCst),
        "dropping the guest's readable end dropped the live producer"
    );
}

#[wcmp_macros::test]
async fn it_delivers_the_items_a_dropped_producer_left_waiting_before_the_end() {
    let (mut store, instance) = instantiate(READS_HOST_ENDS).await;
    let reader = StreamReader::new(&mut store.as_context_mut(), b"abcdef".to_vec())
        .expect("a stream over a vector");
    let end = take(&mut store, &instance, reader).await;

    assert_eq!(
        call_u32(&mut store, &instance, "read", &[end, 100, 4]).await,
        packed(COMPLETED, 4),
        "the vector ended the stream with six items, and the read took four"
    );
    assert_eq!(bytes_at(&mut store, &instance, 100, 4).await, b"abcd");
    assert_eq!(
        call_u32(&mut store, &instance, "read", &[end, 200, 4]).await,
        packed(DROPPED, 2),
        "the next read took the two left waiting and learned the stream ended"
    );
    assert_eq!(bytes_at(&mut store, &instance, 200, 2).await, b"ef");
}

#[wcmp_macros::test]
async fn it_writes_a_stream_from_a_boxed_slice_and_ends_an_empty_iterator_at_once() {
    let (mut store, instance) = instantiate(READS_HOST_ENDS).await;
    let items: Box<[u8]> = Box::new(*b"xyz");
    let boxed = StreamReader::new(&mut store.as_context_mut(), items).expect("a stream");
    let boxed = take(&mut store, &instance, boxed).await;
    let empty = StreamReader::new(&mut store.as_context_mut(), core::iter::empty::<u8>())
        .expect("a stream");
    let empty = take(&mut store, &instance, empty).await;

    assert_eq!(
        call_u32(&mut store, &instance, "read", &[boxed, 100, 8]).await,
        packed(DROPPED, 3),
        "the boxed slice delivered its items and ended the stream"
    );
    assert_eq!(bytes_at(&mut store, &instance, 100, 3).await, b"xyz");
    assert_eq!(
        call_u32(&mut store, &instance, "read", &[empty, 200, 8]).await,
        packed(DROPPED, 0),
        "the empty iterator ended the stream with nothing delivered"
    );
}

#[wcmp_macros::test]
async fn it_delivers_strings_through_the_readers_realloc() {
    let (mut store, instance) = instantiate(READS_HOST_ENDS).await;
    let words = vec!["one".to_owned(), "three".to_owned()];
    let reader = StreamReader::new(&mut store.as_context_mut(), words).expect("a stream");
    let end = instance
        .get_func("take-strings")
        .expect("the component exports `take-strings`")
        .typed::<(StreamReader<String>,), u32>()
        .expect("`take-strings` takes a `stream<string>`")
        .call(&mut store, (reader,))
        .await
        .expect("the reader crosses into the guest");

    assert_eq!(
        call_u32(&mut store, &instance, "read-strings", &[end, 100, 4]).await,
        packed(DROPPED, 2),
        "the read took both strings and learned the stream ended"
    );
    let mut words = Vec::new();
    for slot in [100, 108] {
        let pointer = call_u32(&mut store, &instance, "peek", &[slot]).await;
        let length = call_u32(&mut store, &instance, "peek", &[slot + 4]).await;
        assert!(
            pointer >= 4096,
            "each string's bytes came from the guest's realloc, got {pointer}"
        );
        let bytes = bytes_at(&mut store, &instance, pointer, length as usize).await;
        words.push(String::from_utf8(bytes).expect("the bytes are text"));
    }
    assert_eq!(words, ["one", "three"]);
}

#[wcmp_macros::test]
async fn it_reads_a_stream_a_host_async_function_created_through_its_accessor() {
    let (mut store, instance) = instantiate(READS_HOST_ENDS).await;
    let end = call_u32(&mut store, &instance, "open-later", &[]).await;

    assert_eq!(
        call_u32(&mut store, &instance, "read", &[end, 100, 8]).await,
        packed(COMPLETED, 5),
        "the guest read the stream the host future created after an await"
    );
    assert_eq!(bytes_at(&mut store, &instance, 100, 5).await, b"later");
}

/// Two streams over boxed slices, `left` and `right`, as the host
/// hands them to the guest in a list.
fn two_streams(store: &mut Store<()>) -> Vec<StreamReader<u8>> {
    [&b"left"[..], &b"right"[..]]
        .into_iter()
        .map(|bytes| {
            let items: Box<[u8]> = bytes.into();
            StreamReader::new(&mut store.as_context_mut(), items).expect("a stream")
        })
        .collect()
}

/// Read each end the guest holds at the list at `address`, two of
/// them, and answer what each delivered.
async fn read_listed_ends(
    store: &mut Store<()>,
    instance: &Instance,
    address: u32,
) -> Vec<Vec<u8>> {
    let mut read = Vec::new();
    for (i, slot) in [address, address + 4].into_iter().enumerate() {
        let end = call_u32(store, instance, "peek", &[slot]).await;
        let buffer = 200 + 100 * i as u32;
        let result = call_u32(store, instance, "read", &[end, buffer, 8]).await;
        read.push(bytes_at(store, instance, buffer, (result >> 4) as usize).await);
    }
    read
}

#[wcmp_macros::test]
async fn it_lowers_readers_in_a_list_through_memory() {
    let (mut store, instance) = instantiate(READS_HOST_ENDS).await;
    let readers = two_streams(&mut store);

    let address = instance
        .get_func("take-list")
        .expect("the component exports `take-list`")
        .typed::<(Vec<StreamReader<u8>>,), u32>()
        .expect("`take-list` takes a `list<stream<u8>>`")
        .call(&mut store, (readers,))
        .await
        .expect("the readers cross into the guest");

    assert_eq!(
        read_listed_ends(&mut store, &instance, address).await,
        [b"left".to_vec(), b"right".to_vec()],
        "each end the list carried reads its own stream"
    );
}

#[wcmp_macros::test]
async fn it_lowers_readers_in_a_list_into_an_async_export() {
    let (mut store, instance) = instantiate(READS_HOST_ENDS).await;
    let readers = two_streams(&mut store);

    let address = instance
        .get_func("take-list-async")
        .expect("the component exports `take-list-async`")
        .typed::<(Vec<StreamReader<u8>>,), u32>()
        .expect("`take-list-async` takes a `list<stream<u8>>`")
        .call(&mut store, (readers,))
        .await
        .expect("the readers cross into the guest");

    assert_eq!(
        read_listed_ends(&mut store, &instance, address).await,
        [b"left".to_vec(), b"right".to_vec()],
        "each end the list carried reads its own stream"
    );
}

/// The result a cancelled copy packs.
const CANCELLED: u32 = 2;

/// The message a cancel of an end with no read pending traps with.
const NO_READ_PENDING: &str = "stream or future read cancelled when no read is pending";

/// A stream producer that has nothing until its read is cancelled.
/// Until then it answers pending without waking anyone, so only the
/// cancel's wake reaches it. Asked to finish, it delivers `on_finish`
/// and answers completed, or answers cancelled when that is empty.
/// Later reads get `after` and completed. It records the `finish`
/// flag of each poll.
struct AwaitsCancel {
    on_finish: &'static [u8],
    after: &'static [u8],
    cancelled: bool,
    finishes: Arc<Mutex<Vec<bool>>>,
}

impl AwaitsCancel {
    /// The producer, and the record of the `finish` flag of each of its
    /// polls.
    fn new(on_finish: &'static [u8], after: &'static [u8]) -> (Self, Arc<Mutex<Vec<bool>>>) {
        let finishes = Arc::new(Mutex::new(Vec::new()));
        let producer = Self {
            on_finish,
            after,
            cancelled: false,
            finishes: finishes.clone(),
        };
        (producer, finishes)
    }
}

impl StreamProducer<()> for AwaitsCancel {
    type Item = u8;

    fn poll_produce(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _store: &mut StoreContext<'_, ()>,
        mut destination: Destination<'_, u8>,
        finish: bool,
    ) -> Poll<Result<StreamResult, Error>> {
        let this = self.get_mut();
        this.finishes
            .lock()
            .expect("the record of the polls")
            .push(finish);
        if finish {
            this.cancelled = true;
            if this.on_finish.is_empty() {
                return Poll::Ready(Ok(StreamResult::Cancelled));
            }
            destination.set_buffer(this.on_finish.to_vec());
            return Poll::Ready(Ok(StreamResult::Completed));
        }
        if !this.cancelled {
            return Poll::Pending;
        }
        destination.set_buffer(this.after.to_vec());
        Poll::Ready(Ok(StreamResult::Completed))
    }
}

/// Whether the last poll `finishes` recorded was asked to finish, and
/// none before it was.
fn finished_last(finishes: &Arc<Mutex<Vec<bool>>>) -> bool {
    let finishes = finishes.lock().expect("the record of the polls");
    finishes.last() == Some(&true) && finishes.iter().filter(|&&finish| finish).count() == 1
}

#[wcmp_macros::test]
async fn it_asks_the_producer_to_finish_when_the_guest_cancels_a_pending_read() {
    let (mut store, instance) = instantiate(READS_HOST_ENDS).await;
    let (producer, finishes) = AwaitsCancel::new(b"", b"ok");
    let reader = StreamReader::new(&mut store.as_context_mut(), producer).expect("a stream");
    let end = take(&mut store, &instance, reader).await;

    assert_eq!(
        call_u32(&mut store, &instance, "read", &[end, 100, 8]).await,
        BLOCKED,
        "the producer had nothing for the read"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "cancel-read", &[end]).await,
        BLOCKED,
        "the cancel waits for the producer to answer it"
    );
    assert_eq!(
        poll_until_an_event(&mut store, &instance, end).await,
        (STREAM_READ, end, packed(CANCELLED, 0)),
        "the producer answered the cancel, and the read ended cancelled with \
         nothing moved"
    );
    assert!(
        finished_last(&finishes),
        "the cancel woke the producer and its next poll was asked to finish, \
         got {:?}",
        finishes.lock().expect("the record of the polls")
    );
    assert_eq!(
        call_u32(&mut store, &instance, "read", &[end, 200, 8]).await,
        packed(COMPLETED, 2),
        "the end is idle again, and the producer serves the next read"
    );
    assert_eq!(bytes_at(&mut store, &instance, 200, 2).await, b"ok");
}

#[wcmp_macros::test]
async fn it_reports_what_a_finishing_producer_delivered_as_the_cancels_progress() {
    let (mut store, instance) = instantiate(READS_HOST_ENDS).await;
    let (producer, finishes) = AwaitsCancel::new(b"xyz", b"");
    let reader = StreamReader::new(&mut store.as_context_mut(), producer).expect("a stream");
    let end = take(&mut store, &instance, reader).await;

    assert_eq!(
        call_u32(&mut store, &instance, "read", &[end, 100, 8]).await,
        BLOCKED
    );
    assert_eq!(
        call_u32(&mut store, &instance, "cancel-read", &[end]).await,
        BLOCKED
    );
    assert_eq!(
        poll_until_an_event(&mut store, &instance, end).await,
        (STREAM_READ, end, packed(CANCELLED, 3)),
        "the items the producer delivered while finishing are the cancelled \
         read's progress"
    );
    assert_eq!(bytes_at(&mut store, &instance, 100, 3).await, b"xyz");
    assert!(finished_last(&finishes));
}

#[wcmp_macros::test]
async fn it_completes_a_synchronous_cancel_in_an_async_task_once_the_producer_answers() {
    let (mut store, instance) = instantiate(READS_HOST_ENDS).await;
    let (producer, finishes) = AwaitsCancel::new(b"", b"");
    let reader = StreamReader::new(&mut store.as_context_mut(), producer).expect("a stream");
    let end = take(&mut store, &instance, reader).await;

    assert_eq!(
        call_u32(&mut store, &instance, "read", &[end, 100, 8]).await,
        BLOCKED
    );
    assert_eq!(
        call_u32(&mut store, &instance, "cancel-read-sync-async", &[end]).await,
        packed(CANCELLED, 0),
        "the synchronous cancel blocked the async task until the producer \
         answered it"
    );
    assert!(finished_last(&finishes));
}

#[wcmp_macros::test]
async fn it_traps_a_synchronous_cancel_that_would_block_a_synchronous_task() {
    let (mut store, instance) = instantiate(READS_HOST_ENDS).await;
    let (producer, _) = AwaitsCancel::new(b"", b"");
    let reader = StreamReader::new(&mut store.as_context_mut(), producer).expect("a stream");
    let end = take(&mut store, &instance, reader).await;

    assert_eq!(
        call_u32(&mut store, &instance, "read", &[end, 100, 8]).await,
        BLOCKED
    );
    let failure = call(&mut store, &instance, "cancel-read-sync", &[end])
        .await
        .expect_err("the cancel cannot block a synchronous export");
    assert!(
        failure.contains("cannot block a synchronous task before returning"),
        "the producer had not answered, so the cancel would block, got {failure}"
    );
}

#[wcmp_macros::test]
async fn it_traps_a_second_cancel_while_the_first_waits_on_the_producer() {
    let (mut store, instance) = instantiate(READS_HOST_ENDS).await;
    let (producer, _) = AwaitsCancel::new(b"", b"");
    let reader = StreamReader::new(&mut store.as_context_mut(), producer).expect("a stream");
    let end = take(&mut store, &instance, reader).await;

    assert_eq!(
        call_u32(&mut store, &instance, "read", &[end, 100, 8]).await,
        BLOCKED
    );
    assert_eq!(
        call_u32(&mut store, &instance, "cancel-read", &[end]).await,
        BLOCKED
    );
    let failure = call(&mut store, &instance, "cancel-read", &[end])
        .await
        .expect_err("the second cancel traps");
    assert!(
        failure.contains(NO_READ_PENDING),
        "the reference traps a cancel of an end already cancelling, where \
         Wasmtime waits again; got {failure}"
    );
}

/// A future producer that has nothing until its read is cancelled,
/// answers the cancel with no value, and has `value` for the read
/// after. Until the cancel it answers pending without waking anyone.
struct FutureAwaitsCancel {
    value: u32,
    cancelled: bool,
    polls: Arc<Mutex<Vec<bool>>>,
}

impl FutureProducer<()> for FutureAwaitsCancel {
    type Item = u32;

    fn poll_produce(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _store: &mut StoreContext<'_, ()>,
        finish: bool,
    ) -> Poll<Result<Option<u32>, Error>> {
        let this = self.get_mut();
        this.polls
            .lock()
            .expect("the record of the polls")
            .push(finish);
        if finish {
            this.cancelled = true;
            return Poll::Ready(Ok(None));
        }
        if this.cancelled {
            Poll::Ready(Ok(Some(this.value)))
        } else {
            Poll::Pending
        }
    }
}

#[wcmp_macros::test]
async fn it_polls_a_future_producer_again_after_it_answered_a_cancel_with_nothing() {
    let (mut store, instance) = instantiate(READS_HOST_ENDS).await;
    let polls = Arc::new(Mutex::new(Vec::new()));
    let producer = FutureAwaitsCancel {
        value: 7,
        cancelled: false,
        polls: polls.clone(),
    };
    let reader = FutureReader::new(&mut store.as_context_mut(), producer).expect("a future");
    let end = future_in_guest(&mut store, &instance, reader).await;

    assert_eq!(
        call_u32(&mut store, &instance, "future-read", &[end, 100]).await,
        BLOCKED
    );
    assert_eq!(
        call_u32(&mut store, &instance, "future-cancel-read", &[end]).await,
        BLOCKED,
        "the cancel waits for the producer to answer it"
    );
    assert_eq!(
        poll_until_an_event(&mut store, &instance, end).await,
        (FUTURE_READ, end, packed(CANCELLED, 0)),
        "the producer answered the cancel with no value"
    );
    assert!(finished_last(&polls));
    assert_eq!(
        call_u32(&mut store, &instance, "future-read", &[end, 200]).await,
        packed(COMPLETED, 0),
        "the next read polled the producer again and took its value"
    );
    assert_eq!(call_u32(&mut store, &instance, "peek", &[200]).await, 7);
}

#[wcmp_macros::test]
async fn it_fails_a_synchronous_read_against_a_pending_producer_with_the_stack_switch_cause() {
    // The read blocks an `async` task, so the nested turn may run any
    // work of the store. There is none, but the producer's end is a
    // host task that stays pending, and only a real suspension can
    // wait for the executor to wake it. The store is not idle, so the
    // cause is not the deadlock.
    let (mut store, instance) = instantiate(READS_HOST_ENDS).await;
    let (producer, finishes) = AwaitsCancel::new(b"", b"");
    let reader = StreamReader::new(&mut store.as_context_mut(), producer).expect("a stream");
    let end = take(&mut store, &instance, reader).await;

    let failure = call(&mut store, &instance, "read-sync-async", &[end, 100, 8])
        .await
        .expect_err("a read the producer never answers cannot return");
    assert!(
        failure.contains(
            "blocking here requires a stack switch, but the target has no suspend provider"
        ),
        "the pending producer is a pending host task, got {failure}"
    );
    let finishes = finishes.lock().expect("the record of the polls");
    assert!(
        !finishes.is_empty() && finishes.iter().all(|&finish| !finish),
        "the read polled the producer and never asked it to finish, got {finishes:?}"
    );
}

#[wcmp_macros::test]
async fn it_fails_a_synchronous_read_against_a_pending_producer_in_a_synchronous_export_as_unable_to_block()
 {
    // The same read from a sync-typed export. That call is in
    // progress and must not block, which turns the stack-switch cause
    // into the cannot-block cause.
    let (mut store, instance) = instantiate(READS_HOST_ENDS).await;
    let (producer, _) = AwaitsCancel::new(b"", b"");
    let reader = StreamReader::new(&mut store.as_context_mut(), producer).expect("a stream");
    let end = take(&mut store, &instance, reader).await;

    let failure = call(&mut store, &instance, "read-sync", &[end, 100, 8])
        .await
        .expect_err("a synchronous export cannot block on the producer");
    assert!(
        failure.contains("cannot block a synchronous task before returning"),
        "a sync-typed call is in progress, got {failure}"
    );
}
