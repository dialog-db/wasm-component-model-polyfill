//! Baseline tests for a stream and a future the host reads through a
//! consumer.
//!
//! A guest creates a stream or a future and hands its readable end to
//! the host: as the result of a typed call, or as the argument of a
//! typed host function. The end arrives as a `StreamReader` or a
//! `FutureReader`, and the guest's entry for it is gone. The host
//! pipes the reader to a consumer, and each write of the guest polls
//! the consumer inside a turn of the store. A poll that is ready
//! completes the write before the built-in returns. A pending poll
//! leaves the write to a later turn, which polls the consumer again
//! and fills the writer's event once it is ready. A consumer that took
//! items and answered pending holds the write back, a cancel of the
//! write asks the consumer to finish, and a consumer's error fails the
//! guest's built-in. A stream the host created and piped to a consumer
//! of its own copies inside a turn with no guest at all.
//!
//! Most tests drive one component whose synchronous exports each call
//! one built-in, so a test starts a write, polls a set for its event,
//! and reads what reached the consumer, one call at a time. Each call
//! is a driver of the store, and the turns it runs are the ones that
//! poll the consumer.

#![cfg(test)]

use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use wasm_component_model_polyfill::{
    Component, CopyCause, Engine, EngineConfig, Error, FutureConsumer, FutureReader, HostCall,
    Instance, Linker, Source, Store, StoreContext, StreamConsumer, StreamReader, StreamResult, Val,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// The word a copy returns when it has not finished.
const BLOCKED: u32 = 0xffff_ffff;

/// The code a poll of a set that holds no event delivers.
const EVENT_NONE: u32 = 0;

/// The code of the event a write on a stream end delivers.
const STREAM_WRITE: u32 = 3;

/// The code of the event a write on a future end delivers.
const FUTURE_WRITE: u32 = 5;

/// The result a completed copy packs into its low four bits.
const COMPLETED: u32 = 0;

/// The result a copy that found the other end dropped packs.
const DROPPED: u32 = 1;

/// The result a cancelled copy packs.
const CANCELLED: u32 = 2;

/// The most calls a test makes while it waits for a later turn to
/// fill an event. Each call is a driver that runs turns, so an event
/// one or two turns away lands well inside it.
const CALLS_UNTIL_AN_EVENT: usize = 8;

/// A component that creates a `stream<u8>` and a `future<u32>` and
/// writes them, one built-in per synchronous export.
///
/// `make` creates a stream, keeps its writable end, and returns its
/// readable end; `make-future` does the same for a future.
/// `hand-over` creates a stream, keeps its writable end, and passes
/// its readable end to the imported host function `consume`. Each of
/// the three keeps the index the readable end had, which
/// `last-readable` returns and `drop-readable` drops. `write` and
/// `future-write` start an asynchronous write of the kept writable
/// end from a pointer, and `cancel-write` and `future-cancel-write`
/// cancel it asynchronously. `drop-writable` drops the stream's
/// writable end. `writable` and `future-writable` return the kept
/// indices. `poll` polls a set and writes the event it delivers at
/// address 0: the end's index there and the packed result at address
/// 4. `make-strings` creates a `stream<string>` and `write-strings`
/// writes it the way `write` writes the stream of bytes. `peek` reads
/// a word of memory and `poke` writes one.
const WRITES_TO_THE_HOST: &[u8] = component!(
    r#"
    (component
      (type $s (stream u8))
      (type $f (future u32))
      (type $t (stream string))
      (import "consume" (func $consume (param "s" $s)))
      (core module $libc
        (memory (export "memory") 1)
        (func (export "peek") (param i32) (result i32) (i32.load (local.get 0)))
        (func (export "poke") (param i32 i32) (i32.store (local.get 0) (local.get 1))))
      (core instance $libc (instantiate $libc))

      (core func $stream-new (canon stream.new $s))
      (core func $future-new (canon future.new $f))
      (core func $strings-new (canon stream.new $t))
      (core func $write-strings
        (canon stream.write $t async (memory (core memory $libc "memory"))))
      (core func $write (canon stream.write $s async (memory (core memory $libc "memory"))))
      (core func $future-write
        (canon future.write $f async (memory (core memory $libc "memory"))))
      (core func $cancel-write (canon stream.cancel-write $s async))
      (core func $future-cancel-write (canon future.cancel-write $f async))
      (core func $drop-readable (canon stream.drop-readable $s))
      (core func $drop-writable (canon stream.drop-writable $s))
      (core func $consume (canon lower (func $consume)))
      (core func $set-new (canon waitable-set.new))
      (core func $poll (canon waitable-set.poll (memory (core memory $libc "memory"))))
      (core func $join (canon waitable.join))

      (core module $m
        (import "" "stream.new" (func $stream-new (result i64)))
        (import "" "future.new" (func $future-new (result i64)))
        (import "" "strings.new" (func $strings-new (result i64)))
        (import "" "write-strings" (func $write-strings (param i32 i32 i32) (result i32)))
        (import "" "stream.write" (func $write (param i32 i32 i32) (result i32)))
        (import "" "future.write" (func $future-write (param i32 i32) (result i32)))
        (import "" "stream.cancel-write" (func $cancel-write (param i32) (result i32)))
        (import "" "future.cancel-write" (func $future-cancel-write (param i32) (result i32)))
        (import "" "stream.drop-readable" (func $drop-readable (param i32)))
        (import "" "stream.drop-writable" (func $drop-writable (param i32)))
        (import "" "consume" (func $consume (param i32)))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (import "" "waitable-set.poll" (func $poll (param i32 i32) (result i32)))
        (import "" "waitable.join" (func $join (param i32 i32)))
        (global $r (mut i32) (i32.const 0))
        (global $w (mut i32) (i32.const 0))
        (global $fw (mut i32) (i32.const 0))
        (global $tw (mut i32) (i32.const 0))
        (func $keep (param $pair i64) (result i32)
          (global.set $r (i32.wrap_i64 (local.get $pair)))
          (global.set $w (i32.wrap_i64 (i64.shr_u (local.get $pair) (i64.const 32))))
          (global.get $r))
        (func (export "make") (result i32) (call $keep (call $stream-new)))
        (func (export "make-future") (result i32)
          (local $pair i64)
          (local.set $pair (call $future-new))
          (global.set $r (i32.wrap_i64 (local.get $pair)))
          (global.set $fw (i32.wrap_i64 (i64.shr_u (local.get $pair) (i64.const 32))))
          (global.get $r))
        (func (export "hand-over") (call $consume (call $keep (call $stream-new))))
        (func (export "make-strings") (result i32)
          (local $pair i64)
          (local.set $pair (call $strings-new))
          (global.set $tw (i32.wrap_i64 (i64.shr_u (local.get $pair) (i64.const 32))))
          (i32.wrap_i64 (local.get $pair)))
        (func (export "write-strings") (param i32 i32) (result i32)
          (call $write-strings (global.get $tw) (local.get 0) (local.get 1)))
        (func (export "last-readable") (result i32) (global.get $r))
        (func (export "drop-readable") (param i32) (call $drop-readable (local.get 0)))
        (func (export "write") (param i32 i32) (result i32)
          (call $write (global.get $w) (local.get 0) (local.get 1)))
        (func (export "future-write") (param i32) (result i32)
          (call $future-write (global.get $fw) (local.get 0)))
        (func (export "cancel-write") (result i32) (call $cancel-write (global.get $w)))
        (func (export "future-cancel-write") (result i32)
          (call $future-cancel-write (global.get $fw)))
        (func (export "drop-writable") (call $drop-writable (global.get $w)))
        (func (export "writable") (result i32) (global.get $w))
        (func (export "future-writable") (result i32) (global.get $fw))
        (func (export "new-set") (result i32) (call $set-new))
        (func (export "poll") (param i32) (result i32) (call $poll (local.get 0) (i32.const 0)))
        (func (export "join") (param i32 i32) (call $join (local.get 0) (local.get 1))))
      (core instance $m (instantiate $m (with "" (instance
        (export "stream.new" (func $stream-new))
        (export "future.new" (func $future-new))
        (export "strings.new" (func $strings-new))
        (export "write-strings" (func $write-strings))
        (export "stream.write" (func $write))
        (export "future.write" (func $future-write))
        (export "stream.cancel-write" (func $cancel-write))
        (export "future.cancel-write" (func $future-cancel-write))
        (export "stream.drop-readable" (func $drop-readable))
        (export "stream.drop-writable" (func $drop-writable))
        (export "consume" (func $consume))
        (export "waitable-set.new" (func $set-new))
        (export "waitable-set.poll" (func $poll))
        (export "waitable.join" (func $join))))))

      (func (export "make") (result $s) (canon lift (core func $m "make")))
      (func (export "make-future") (result $f) (canon lift (core func $m "make-future")))
      (func (export "hand-over") (canon lift (core func $m "hand-over")))
      (func (export "make-strings") (result $t) (canon lift (core func $m "make-strings")))
      (func (export "write-strings") (param "p" u32) (param "n" u32) (result u32)
        (canon lift (core func $m "write-strings")))
      (func (export "last-readable") (result u32) (canon lift (core func $m "last-readable")))
      (func (export "drop-readable") (param "e" u32) (canon lift (core func $m "drop-readable")))
      (func (export "write") (param "p" u32) (param "n" u32) (result u32)
        (canon lift (core func $m "write")))
      (func (export "future-write") (param "p" u32) (result u32)
        (canon lift (core func $m "future-write")))
      (func (export "cancel-write") (result u32) (canon lift (core func $m "cancel-write")))
      (func (export "future-cancel-write") (result u32)
        (canon lift (core func $m "future-cancel-write")))
      (func (export "drop-writable") (canon lift (core func $m "drop-writable")))
      (func (export "writable") (result u32) (canon lift (core func $m "writable")))
      (func (export "future-writable") (result u32) (canon lift (core func $m "future-writable")))
      (func (export "new-set") (result u32) (canon lift (core func $m "new-set")))
      (func (export "poll") (param "s" u32) (result u32) (canon lift (core func $m "poll")))
      (func (export "join") (param "w" u32) (param "s" u32) (canon lift (core func $m "join")))
      (func (export "peek") (param "p" u32) (result u32) (canon lift (core func $libc "peek")))
      (func (export "poke") (param "p" u32) (param "v" u32) (canon lift (core func $libc "poke"))))
    "#
);

/// What a stream consumer answers on one poll.
#[derive(Clone, Copy)]
enum Take {
    /// Take every item offered and answer that more can follow.
    All,
    /// Take up to this many items and answer that more can follow.
    Up(usize),
    /// Take nothing and answer pending, after waking the waker, so the
    /// next turn polls the consumer again.
    Pend,
    /// Take up to this many items and answer pending, keeping the
    /// waker: the backpressure of the contract.
    Hold(usize),
    /// Take nothing and answer pending, keeping the waker, until the
    /// test opens the gate; the first poll after that answers with the
    /// next step.
    Gate,
    /// Take nothing and answer pending, keeping the waker, until a
    /// poll is asked to finish; that poll answers with the next step.
    Wait,
    /// Answer completed without taking anything.
    Nothing,
    /// Take every item offered and answer that the consumer is over.
    End,
    /// Answer that the write was cancelled, taking nothing.
    Cancel,
    /// Take up to this many items and answer that the write was
    /// cancelled.
    CancelAfter(usize),
    /// Fail with this message.
    Fail(&'static str),
}

/// What a consumer recorded, shared with the test.
#[derive(Default)]
struct Log {
    /// Every item the consumer took, in order.
    taken: Vec<u8>,
    /// The count each poll's source offered and the poll's `finish`
    /// flag.
    polls: Vec<(usize, bool)>,
    /// The waker of the last poll that kept its waker.
    kept: Option<Waker>,
    /// Whether the test opened the gate.
    open: bool,
    /// Whether the consumer was dropped.
    dropped: bool,
    /// A waker to wake when the consumer is dropped.
    watcher: Option<Waker>,
}

/// The log a consumer shares with the test.
type Shared = Arc<Mutex<Log>>;

/// Lock `log`.
fn lock(log: &Shared) -> std::sync::MutexGuard<'_, Log> {
    log.lock().expect("the consumer's log")
}

/// A stream consumer that answers its polls with `steps`, in order,
/// and takes every item once they run out.
struct Scripted {
    steps: VecDeque<Take>,
    log: Shared,
}

impl Scripted {
    /// A consumer answering with `steps`, and the log it keeps.
    fn new(steps: impl IntoIterator<Item = Take>) -> (Self, Shared) {
        let log = Shared::default();
        let consumer = Self {
            steps: steps.into_iter().collect(),
            log: log.clone(),
        };
        (consumer, log)
    }
}

impl Drop for Scripted {
    fn drop(&mut self) {
        let watcher = self.log.lock().ok().and_then(|mut log| {
            log.dropped = true;
            log.watcher.take()
        });
        if let Some(watcher) = watcher {
            watcher.wake();
        }
    }
}

/// Take up to `count` items out of `source` into the log.
fn take(
    store: &mut StoreContext<'_, ()>,
    source: &mut Source<'_, u8>,
    count: usize,
    log: &mut Log,
) -> Result<(), Error> {
    let mut items = Vec::new();
    source.read(store, &mut items, count)?;
    log.taken.extend(items);
    Ok(())
}

impl StreamConsumer<()> for Scripted {
    type Item = u8;

    fn poll_consume(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: &mut StoreContext<'_, ()>,
        mut source: Source<'_, u8>,
        finish: bool,
    ) -> Poll<Result<StreamResult, Error>> {
        let this = self.get_mut();
        let mut log = lock(&this.log);
        log.polls.push((source.remaining(), finish));
        let all = source.remaining();
        let mut step = this.steps.pop_front().unwrap_or(Take::All);
        loop {
            let held = match step {
                Take::Gate => !log.open,
                Take::Wait => !finish,
                _ => break,
            };
            if held {
                this.steps.push_front(step);
                log.kept = Some(cx.waker().clone());
                return Poll::Pending;
            }
            step = this.steps.pop_front().unwrap_or(Take::All);
        }
        let (count, answer) = match step {
            Take::All => (all, Poll::Ready(StreamResult::Completed)),
            Take::Up(count) => (count, Poll::Ready(StreamResult::Completed)),
            Take::Pend => {
                cx.waker().wake_by_ref();
                (0, Poll::Pending)
            }
            Take::Hold(count) => {
                log.kept = Some(cx.waker().clone());
                (count, Poll::Pending)
            }
            Take::Gate | Take::Wait => unreachable!("a held step answered above"),
            Take::Nothing => (0, Poll::Ready(StreamResult::Completed)),
            Take::End => (all, Poll::Ready(StreamResult::Dropped)),
            Take::Cancel => (0, Poll::Ready(StreamResult::Cancelled)),
            Take::CancelAfter(count) => (count, Poll::Ready(StreamResult::Cancelled)),
            Take::Fail(message) => {
                return Poll::Ready(Err(Error::Internal {
                    message: message.to_owned(),
                }));
            }
        };
        take(store, &mut source, count, &mut log)?;
        answer.map(Ok)
    }
}

/// Open the gate of the consumer behind `log` and wake the waker it
/// kept.
fn release(log: &Shared) {
    let kept = {
        let mut log = lock(log);
        log.open = true;
        log.kept.take()
    };
    kept.expect("the consumer kept a waker").wake();
}

/// Whether the last poll `log` recorded was asked to finish, and none
/// before it was.
fn finished_last(log: &Shared) -> bool {
    let log = lock(log);
    let finishes: Vec<bool> = log.polls.iter().map(|&(_, finish)| finish).collect();
    finishes.last() == Some(&true) && finishes.iter().filter(|&&finish| finish).count() == 1
}

/// Instantiate [`WRITES_TO_THE_HOST`] into a store of its own. The
/// import `consume` is a typed host function that takes a
/// `StreamReader<u8>` and pipes it to the consumer `consumer` holds.
async fn instantiate(consumer: Option<Scripted>) -> (Store<()>, Instance) {
    // The `async` cancels need the more-async-builtins feature.
    let mut config = EngineConfig::new();
    config.wasm_component_model_more_async_builtins(true);
    let engine = Engine::with_config(&config).expect("engine");
    let component = Component::new(&engine, WRITES_TO_THE_HOST)
        .await
        .expect("the component parses");
    let slot = Arc::new(Mutex::new(consumer));
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap(
            "consume",
            move |mut call: HostCall<'_, ()>, (reader,): (StreamReader<u8>,)| {
                let consumer = slot
                    .lock()
                    .expect("the consumer's slot")
                    .take()
                    .expect("a consumer for the stream");
                reader.pipe(call.store(), consumer)
            },
        )
        .expect("register `consume`");
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

/// A stream the guest created, piped to `consumer`.
async fn piped(store: &mut Store<()>, instance: &Instance, consumer: Scripted) {
    let reader = make(store, instance).await;
    reader
        .pipe(&mut store.as_context_mut(), consumer)
        .expect("the host pipes the reader");
}

/// Write `bytes` at `address` of the guest's memory.
async fn poke(store: &mut Store<()>, instance: &Instance, address: u32, bytes: &[u8]) {
    for (i, chunk) in bytes.chunks(4).enumerate() {
        let mut word = [0u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        call_ok(
            store,
            instance,
            "poke",
            &[address + 4 * i as u32, u32::from_le_bytes(word)],
        )
        .await;
    }
}

/// Write `bytes` into the guest's memory at 100 and start the guest's
/// asynchronous write of them. Answers what the write returned.
async fn write(store: &mut Store<()>, instance: &Instance, bytes: &[u8]) -> u32 {
    poke(store, instance, 100, bytes).await;
    call_u32(store, instance, "write", &[100, bytes.len() as u32]).await
}

/// Join the end the export `end` names to a fresh set and poll the
/// set, a call at a time, until it delivers an event or
/// [`CALLS_UNTIL_AN_EVENT`] calls have passed. Answers the event as
/// the guest reads it: the code, the end's index, and the packed
/// result.
async fn poll_until_an_event(
    store: &mut Store<()>,
    instance: &Instance,
    end: &str,
) -> (u32, u32, u32) {
    let end = call_u32(store, instance, end, &[]).await;
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

/// The packed result of a copy: `result` in the low four bits and
/// `count` above them.
fn packed(result: u32, count: u32) -> u32 {
    result | (count << 4)
}

#[wcmp_macros::test]
async fn it_hands_a_guests_writes_to_a_piped_consumer_inside_turns() {
    let (mut store, instance) = instantiate(None).await;
    let (consumer, log) = Scripted::new([Take::Pend, Take::All, Take::Pend, Take::All]);
    piped(&mut store, &instance, consumer).await;
    let w = call_u32(&mut store, &instance, "writable", &[]).await;

    assert_eq!(
        write(&mut store, &instance, b"abcd").await,
        BLOCKED,
        "the consumer could take nothing when the write polled it"
    );
    assert_eq!(
        poll_until_an_event(&mut store, &instance, "writable").await,
        (STREAM_WRITE, w, packed(COMPLETED, 4)),
        "a later turn polled the consumer again and it took the write"
    );
    assert_eq!(write(&mut store, &instance, b"efg").await, BLOCKED);
    assert_eq!(
        poll_until_an_event(&mut store, &instance, "writable").await,
        (STREAM_WRITE, w, packed(COMPLETED, 3)),
        "the second write reached the consumer in a later turn"
    );
    assert_eq!(
        write(&mut store, &instance, b"hi").await,
        packed(COMPLETED, 2),
        "a consumer ready at once completes the write before it returns"
    );
    let log = lock(&log);
    assert_eq!(log.taken, b"abcdefghi");
    assert_eq!(
        log.polls,
        [(4, false), (4, false), (3, false), (3, false), (2, false)],
        "each write polled the consumer when it started and again in a \
         later turn while it was pending"
    );
    assert!(!log.dropped, "the consumer serves the stream still");
}

#[wcmp_macros::test]
async fn it_holds_the_writer_back_while_the_consumer_answers_pending_after_taking_items() {
    let (mut store, instance) = instantiate(None).await;
    let (consumer, log) = Scripted::new([Take::Hold(2), Take::Gate, Take::Nothing]);
    piped(&mut store, &instance, consumer).await;
    let w = call_u32(&mut store, &instance, "writable", &[]).await;

    assert_eq!(
        write(&mut store, &instance, b"abcd").await,
        BLOCKED,
        "the consumer took two items and answered pending"
    );
    assert_eq!(lock(&log).taken, b"ab", "the two items left the guest");
    let set = call_u32(&mut store, &instance, "new-set", &[]).await;
    call_ok(&mut store, &instance, "join", &[w, set]).await;
    for _ in 0..CALLS_UNTIL_AN_EVENT {
        assert_eq!(
            call_u32(&mut store, &instance, "poll", &[set]).await,
            EVENT_NONE,
            "the writer is not told of the write while the consumer holds it"
        );
    }
    call_ok(&mut store, &instance, "join", &[w, 0]).await;
    assert_eq!(
        lock(&log).polls,
        [(4, false), (2, false)],
        "a later poll was offered only the items the first left, and the \
         consumer went on holding the write"
    );

    release(&log);
    assert_eq!(
        poll_until_an_event(&mut store, &instance, "writable").await,
        (STREAM_WRITE, w, packed(COMPLETED, 2)),
        "the write completed once the consumer answered ready, with the \
         items the earlier poll took"
    );
    assert_eq!(lock(&log).taken, b"ab");
}

#[wcmp_macros::test]
async fn it_asks_the_consumer_to_finish_when_the_guest_cancels_a_pending_write() {
    let (mut store, instance) = instantiate(None).await;
    let (consumer, log) = Scripted::new([Take::Wait, Take::Cancel, Take::All]);
    piped(&mut store, &instance, consumer).await;
    let w = call_u32(&mut store, &instance, "writable", &[]).await;

    assert_eq!(write(&mut store, &instance, b"abc").await, BLOCKED);
    assert_eq!(
        call_u32(&mut store, &instance, "cancel-write", &[]).await,
        BLOCKED,
        "the cancel waits for the consumer to answer it"
    );
    assert_eq!(
        poll_until_an_event(&mut store, &instance, "writable").await,
        (STREAM_WRITE, w, packed(CANCELLED, 0)),
        "the consumer answered the cancel, and the write ended cancelled \
         with nothing moved"
    );
    assert!(
        finished_last(&log),
        "the cancel woke the consumer and its next poll was asked to finish, \
         got {:?}",
        lock(&log).polls
    );
    assert_eq!(
        write(&mut store, &instance, b"xyz").await,
        packed(COMPLETED, 3),
        "the end is idle again, and the consumer takes the next write"
    );
    assert_eq!(lock(&log).taken, b"xyz");
}

#[wcmp_macros::test]
async fn it_reports_what_a_finishing_consumer_took_as_the_cancels_progress() {
    let (mut store, instance) = instantiate(None).await;
    let (consumer, log) = Scripted::new([Take::Wait, Take::CancelAfter(2)]);
    piped(&mut store, &instance, consumer).await;
    let w = call_u32(&mut store, &instance, "writable", &[]).await;

    assert_eq!(write(&mut store, &instance, b"abcd").await, BLOCKED);
    assert_eq!(
        call_u32(&mut store, &instance, "cancel-write", &[]).await,
        BLOCKED
    );
    assert_eq!(
        poll_until_an_event(&mut store, &instance, "writable").await,
        (STREAM_WRITE, w, packed(CANCELLED, 2)),
        "the write ended cancelled, with the items the finishing poll took"
    );
    assert_eq!(lock(&log).taken, b"ab");
}

#[wcmp_macros::test]
async fn it_serves_a_write_the_guest_started_before_the_host_piped_the_stream() {
    let (mut store, instance) = instantiate(None).await;
    let reader = make(&mut store, &instance).await;
    let w = call_u32(&mut store, &instance, "writable", &[]).await;

    assert_eq!(
        write(&mut store, &instance, b"early").await,
        BLOCKED,
        "nothing reads the stream yet"
    );
    let (consumer, log) = Scripted::new([Take::All]);
    reader
        .pipe(&mut store.as_context_mut(), consumer)
        .expect("the host pipes the reader");
    assert_eq!(
        poll_until_an_event(&mut store, &instance, "writable").await,
        (STREAM_WRITE, w, packed(COMPLETED, 5)),
        "the next turn served the waiting write"
    );
    assert_eq!(lock(&log).taken, b"early");
}

#[wcmp_macros::test]
async fn it_completes_the_write_dropped_when_the_consumer_is_over() {
    let (mut store, instance) = instantiate(None).await;
    let (consumer, log) = Scripted::new([Take::End]);
    piped(&mut store, &instance, consumer).await;

    assert_eq!(
        write(&mut store, &instance, b"last").await,
        packed(DROPPED, 4),
        "the consumer took the write and said it takes nothing more"
    );
    assert!(lock(&log).dropped, "the host's end let the consumer go");
    let failure = call(&mut store, &instance, "write", &[100, 1])
        .await
        .expect_err("the writer was told the reader dropped");
    assert!(
        failure.contains("cannot write after being notified that the readable end dropped"),
        "{failure}"
    );
}

#[wcmp_macros::test]
async fn it_drops_the_consumer_unpolled_when_the_guest_drops_its_writable_end() {
    let (mut store, instance) = instantiate(None).await;
    let (consumer, log) = Scripted::new([]);
    piped(&mut store, &instance, consumer).await;

    call_ok(&mut store, &instance, "drop-writable", &[]).await;
    let log = lock(&log);
    assert!(log.dropped, "the consumer learned the stream ended");
    assert!(log.polls.is_empty(), "no write ever reached it");
}

#[wcmp_macros::test]
async fn it_drops_the_consumer_of_a_stream_whose_writer_dropped_before_the_pipe() {
    let (mut store, instance) = instantiate(None).await;
    let reader = make(&mut store, &instance).await;
    call_ok(&mut store, &instance, "drop-writable", &[]).await;

    let (consumer, log) = Scripted::new([]);
    reader
        .pipe(&mut store.as_context_mut(), consumer)
        .expect("a pipe of an ended stream succeeds");
    let log = lock(&log);
    assert!(log.dropped && log.polls.is_empty());
}

/// Whether `failure` is the trap of a built-in that failed with
/// `cause`.
fn failed_with(failure: &str, cause: CopyCause) -> bool {
    failure.contains(&cause.to_string())
}

#[wcmp_macros::test]
async fn it_fails_the_guests_built_in_with_the_consumers_error() {
    let (mut store, instance) = instantiate(None).await;
    let (consumer, _) = Scripted::new([Take::Fail("the sink is gone")]);
    piped(&mut store, &instance, consumer).await;

    poke(&mut store, &instance, 100, b"a").await;
    let failure = call(&mut store, &instance, "write", &[100, 1])
        .await
        .expect_err("the consumer failed the write");
    assert!(failure.contains("the sink is gone"), "{failure}");
}

#[wcmp_macros::test]
async fn it_fails_a_consumer_that_completes_a_write_without_taking_items() {
    let (mut store, instance) = instantiate(None).await;
    let (consumer, _) = Scripted::new([Take::Nothing]);
    piped(&mut store, &instance, consumer).await;

    poke(&mut store, &instance, 100, b"a").await;
    let failure = call(&mut store, &instance, "write", &[100, 1])
        .await
        .expect_err("the answer breaks the contract");
    assert!(
        failed_with(&failure, CopyCause::ConsumerCompletedWithoutItems),
        "{failure}"
    );
}

#[wcmp_macros::test]
async fn it_completes_a_zero_length_write_a_consumer_answers_without_items() {
    let (mut store, instance) = instantiate(None).await;
    let (consumer, log) = Scripted::new([Take::Nothing]);
    piped(&mut store, &instance, consumer).await;

    assert_eq!(
        call_u32(&mut store, &instance, "write", &[100, 0]).await,
        packed(COMPLETED, 0),
        "a write of nothing asks whether the stream is ready"
    );
    assert_eq!(lock(&log).polls, [(0, false)]);
}

#[wcmp_macros::test]
async fn it_fails_a_consumer_that_cancels_a_write_it_was_not_asked_to_finish() {
    let (mut store, instance) = instantiate(None).await;
    let (consumer, _) = Scripted::new([Take::Cancel]);
    piped(&mut store, &instance, consumer).await;

    poke(&mut store, &instance, 100, b"a").await;
    let failure = call(&mut store, &instance, "write", &[100, 1])
        .await
        .expect_err("the answer breaks the contract");
    assert!(
        failed_with(&failure, CopyCause::ConsumerCancelledWithoutFinish),
        "{failure}"
    );
}

#[wcmp_macros::test]
async fn it_returns_a_stream_reader_from_a_typed_call_and_takes_the_guests_entry() {
    let (mut store, instance) = instantiate(None).await;
    let reader = make(&mut store, &instance).await;
    let index = call_u32(&mut store, &instance, "last-readable", &[]).await;

    let failure = call(&mut store, &instance, "drop-readable", &[index])
        .await
        .expect_err("the guest's entry for the readable end is gone");
    assert!(
        failure.contains(&format!("unknown handle index {index}")),
        "{failure}"
    );

    let (consumer, log) = Scripted::new([Take::All]);
    reader
        .pipe(&mut store.as_context_mut(), consumer)
        .expect("the host pipes the reader it was handed");
    assert_eq!(
        write(&mut store, &instance, b"ok").await,
        packed(COMPLETED, 2)
    );
    assert_eq!(lock(&log).taken, b"ok");
}

#[wcmp_macros::test]
async fn it_hands_a_typed_host_function_a_stream_reader_for_a_parameter() {
    let (consumer, log) = Scripted::new([Take::All]);
    let (mut store, instance) = instantiate(Some(consumer)).await;
    call_ok(&mut store, &instance, "hand-over", &[]).await;
    let index = call_u32(&mut store, &instance, "last-readable", &[]).await;

    let failure = call(&mut store, &instance, "drop-readable", &[index])
        .await
        .expect_err("the guest's entry for the readable end is gone");
    assert!(
        failure.contains(&format!("unknown handle index {index}")),
        "{failure}"
    );
    assert_eq!(
        write(&mut store, &instance, b"param").await,
        packed(COMPLETED, 5),
        "the host function piped the reader it received"
    );
    assert_eq!(lock(&log).taken, b"param");
}

#[wcmp_macros::test]
async fn it_refuses_a_typed_reader_of_another_payload_type() {
    let (mut store, instance) = instantiate(None).await;
    let refused = instance
        .get_func("make")
        .expect("the component exports `make`")
        .typed::<(), StreamReader<u32>>();
    assert!(
        matches!(refused, Err(Error::TypeMismatch(_))),
        "a `stream<u8>` does not lift as a `StreamReader<u32>`"
    );
    // The guest's end stays where it was.
    let _ = make(&mut store, &instance).await;
}

/// What a future consumer answers on one poll.
#[derive(Clone, Copy)]
enum Receive {
    /// Take the value and answer ready.
    Take,
    /// Take nothing and answer pending, after waking the waker.
    Pend,
    /// Take nothing and answer pending, keeping the waker, until a
    /// poll is asked to finish; that poll answers with the next step.
    Wait,
    /// Answer ready without taking the value.
    Skip,
}

/// A future consumer that answers its polls with `steps`, in order,
/// and records the value it takes and the `finish` flag of each poll.
struct Receives {
    steps: VecDeque<Receive>,
    value: Arc<Mutex<FutureLog>>,
}

/// What a future consumer recorded, shared with the test.
#[derive(Default)]
struct FutureLog {
    value: Option<u32>,
    finishes: Vec<bool>,
    kept: Option<Waker>,
    dropped: bool,
    watcher: Option<Waker>,
}

impl Receives {
    /// A consumer answering with `steps`, and the log it keeps.
    fn new(steps: impl IntoIterator<Item = Receive>) -> (Self, Arc<Mutex<FutureLog>>) {
        let value = Arc::new(Mutex::new(FutureLog::default()));
        let consumer = Self {
            steps: steps.into_iter().collect(),
            value: value.clone(),
        };
        (consumer, value)
    }
}

impl Drop for Receives {
    fn drop(&mut self) {
        let watcher = self.value.lock().ok().and_then(|mut log| {
            log.dropped = true;
            log.watcher.take()
        });
        if let Some(watcher) = watcher {
            watcher.wake();
        }
    }
}

impl FutureConsumer<()> for Receives {
    type Item = u32;

    fn poll_consume(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: &mut StoreContext<'_, ()>,
        mut source: Source<'_, u32>,
        finish: bool,
    ) -> Poll<Result<(), Error>> {
        let this = self.get_mut();
        let mut log = this.value.lock().expect("the consumer's log");
        log.finishes.push(finish);
        let mut step = this.steps.pop_front().unwrap_or(Receive::Take);
        if let Receive::Wait = step {
            if !finish {
                this.steps.push_front(step);
                log.kept = Some(cx.waker().clone());
                return Poll::Pending;
            }
            step = this.steps.pop_front().unwrap_or(Receive::Take);
        }
        match step {
            Receive::Take => {
                let mut value = Vec::new();
                source.read(store, &mut value, 1)?;
                log.value = value.pop();
                Poll::Ready(Ok(()))
            }
            Receive::Pend => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Receive::Wait => unreachable!("a waiting step answered above"),
            Receive::Skip => Poll::Ready(Ok(())),
        }
    }
}

/// Call the guest's `make-future` through a typed call, and pipe the
/// reader it hands the host to `consumer`.
async fn piped_future(store: &mut Store<()>, instance: &Instance, consumer: Receives) {
    let reader = instance
        .get_func("make-future")
        .expect("the component exports `make-future`")
        .typed::<(), FutureReader<u32>>()
        .expect("`make-future` returns a `future<u32>`")
        .call(&mut *store, ())
        .await
        .expect("the readable end crosses to the host");
    reader
        .pipe(&mut store.as_context_mut(), consumer)
        .expect("the host pipes the reader");
}

#[wcmp_macros::test]
async fn it_gives_a_future_consumer_the_guests_one_value() {
    let (mut store, instance) = instantiate(None).await;
    let (consumer, log) = Receives::new([Receive::Take]);
    piped_future(&mut store, &instance, consumer).await;

    call_ok(&mut store, &instance, "poke", &[100, 42]).await;
    assert_eq!(
        call_u32(&mut store, &instance, "future-write", &[100]).await,
        packed(COMPLETED, 0),
        "the consumer took the value when the write polled it"
    );
    let log = log.lock().expect("the consumer's log");
    assert_eq!(log.value, Some(42));
    assert!(
        log.dropped,
        "a future is read once, so its consumer is let go"
    );
}

#[wcmp_macros::test]
async fn it_gives_a_future_consumer_the_value_in_a_later_turn() {
    let (mut store, instance) = instantiate(None).await;
    let (consumer, log) = Receives::new([Receive::Pend, Receive::Take]);
    piped_future(&mut store, &instance, consumer).await;
    let w = call_u32(&mut store, &instance, "future-writable", &[]).await;

    call_ok(&mut store, &instance, "poke", &[100, 7]).await;
    assert_eq!(
        call_u32(&mut store, &instance, "future-write", &[100]).await,
        BLOCKED
    );
    assert_eq!(
        poll_until_an_event(&mut store, &instance, "future-writable").await,
        (FUTURE_WRITE, w, packed(COMPLETED, 0))
    );
    assert_eq!(log.lock().expect("the consumer's log").value, Some(7));
}

#[wcmp_macros::test]
async fn it_leaves_a_futures_value_with_the_writer_when_a_cancel_finishes_the_consumer() {
    let (mut store, instance) = instantiate(None).await;
    let (consumer, log) = Receives::new([Receive::Wait, Receive::Skip, Receive::Take]);
    piped_future(&mut store, &instance, consumer).await;
    let w = call_u32(&mut store, &instance, "future-writable", &[]).await;

    call_ok(&mut store, &instance, "poke", &[100, 9]).await;
    assert_eq!(
        call_u32(&mut store, &instance, "future-write", &[100]).await,
        BLOCKED
    );
    assert_eq!(
        call_u32(&mut store, &instance, "future-cancel-write", &[]).await,
        BLOCKED
    );
    assert_eq!(
        poll_until_an_event(&mut store, &instance, "future-writable").await,
        (FUTURE_WRITE, w, packed(CANCELLED, 0)),
        "the consumer answered the cancel without the value"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "future-write", &[100]).await,
        packed(COMPLETED, 0),
        "the future can be written again, and the consumer takes it"
    );
    let log = log.lock().expect("the consumer's log");
    assert_eq!(
        log.finishes.iter().filter(|&&finish| finish).count(),
        1,
        "only the poll after the cancel was asked to finish"
    );
    assert_eq!(log.value, Some(9));
}

#[wcmp_macros::test]
async fn it_fails_a_future_consumer_that_answers_without_the_value_unasked() {
    let (mut store, instance) = instantiate(None).await;
    let (consumer, _) = Receives::new([Receive::Skip]);
    piped_future(&mut store, &instance, consumer).await;

    let failure = call(&mut store, &instance, "future-write", &[100])
        .await
        .expect_err("the answer breaks the contract");
    assert!(
        failed_with(&failure, CopyCause::ConsumerCancelledWithoutFinish),
        "{failure}"
    );
}

/// Run turns of `store` until the consumer that keeps `log` is
/// dropped, which the pipe does once it is over.
async fn run_until_dropped(store: &mut Store<()>, log: &Shared) {
    store
        .run_concurrent(async |_accessor| {
            core::future::poll_fn(|cx| {
                let mut log = lock(log);
                if log.dropped {
                    return Poll::Ready(());
                }
                log.watcher = Some(cx.waker().clone());
                Poll::Pending
            })
            .await;
        })
        .await
        .expect("the turns run");
}

#[wcmp_macros::test]
async fn it_copies_a_host_stream_piped_to_a_host_consumer_inside_a_turn() {
    let engine = Engine::new().expect("engine");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let reader = StreamReader::new(&mut store.as_context_mut(), b"no guest".to_vec())
        .expect("a stream the host writes");
    let (consumer, log) = Scripted::new([Take::Up(3), Take::Pend, Take::All]);
    reader
        .pipe(&mut store.as_context_mut(), consumer)
        .expect("the host pipes its own stream");
    assert!(
        lock(&log).polls.is_empty(),
        "the pipe copies inside a turn, not inside the call that made it"
    );

    run_until_dropped(&mut store, &log).await;
    let log = lock(&log);
    assert_eq!(log.taken, b"no guest", "every item reached the consumer");
    assert_eq!(
        log.polls,
        [(8, false), (5, false), (5, false)],
        "the consumer was offered what the producer delivered, less what \
         it had taken"
    );
}

#[wcmp_macros::test]
async fn it_hands_a_host_futures_value_to_a_host_consumer_inside_a_turn() {
    let engine = Engine::new().expect("engine");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let reader = FutureReader::new(&mut store.as_context_mut(), async { Ok::<_, Error>(5u32) })
        .expect("a future the host writes");
    let (consumer, log) = Receives::new([Receive::Take]);
    reader
        .pipe(&mut store.as_context_mut(), consumer)
        .expect("the host pipes its own future");

    store
        .run_concurrent(async |_accessor| {
            core::future::poll_fn(|cx| {
                let mut log = log.lock().expect("the consumer's log");
                if log.dropped {
                    return Poll::Ready(());
                }
                log.watcher = Some(cx.waker().clone());
                Poll::Pending
            })
            .await;
        })
        .await
        .expect("the turns run");
    assert_eq!(log.lock().expect("the consumer's log").value, Some(5));
}

/// A consumer of strings that takes every string offered and keeps
/// them.
struct Strings(Arc<Mutex<Vec<String>>>);

impl StreamConsumer<()> for Strings {
    type Item = String;

    fn poll_consume(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        store: &mut StoreContext<'_, ()>,
        mut source: Source<'_, String>,
        _finish: bool,
    ) -> Poll<Result<StreamResult, Error>> {
        let mut items = Vec::new();
        let count = source.remaining();
        source.read(store, &mut items, count)?;
        self.0.lock().expect("the strings").extend(items);
        Poll::Ready(Ok(StreamResult::Completed))
    }
}

#[wcmp_macros::test]
async fn it_lifts_each_string_a_guest_writes_out_of_its_memory() {
    let (mut store, instance) = instantiate(None).await;
    let reader = instance
        .get_func("make-strings")
        .expect("the component exports `make-strings`")
        .typed::<(), StreamReader<String>>()
        .expect("`make-strings` returns a `stream<string>`")
        .call(&mut store, ())
        .await
        .expect("the readable end crosses to the host");
    let strings = Arc::new(Mutex::new(Vec::new()));
    reader
        .pipe(&mut store.as_context_mut(), Strings(strings.clone()))
        .expect("the host pipes the reader");

    poke(&mut store, &instance, 300, b"hello").await;
    poke(&mut store, &instance, 320, b"wasm").await;
    for (address, word) in [(200, 300), (204, 5), (208, 320), (212, 4)] {
        call_ok(&mut store, &instance, "poke", &[address, word]).await;
    }
    assert_eq!(
        call_u32(&mut store, &instance, "write-strings", &[200, 2]).await,
        packed(COMPLETED, 2)
    );
    assert_eq!(
        *strings.lock().expect("the strings"),
        ["hello", "wasm"],
        "each string was lifted out of the writer's memory as it was taken"
    );
}
