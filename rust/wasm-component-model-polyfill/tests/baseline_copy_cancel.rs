//! Baseline tests for `stream.cancel-read`, `stream.cancel-write`,
//! `future.cancel-read`, and `future.cancel-write`.
//!
//! A cancel ends the copy in progress on one end and returns the
//! packed result that reports it. A copy that already completed left
//! its event on the end, and the cancel returns it with the progress
//! the copy made: a stream's as the cancelled result, and a future's
//! as the completed one. A copy that is still the pending side takes
//! the cancelled result with nothing moved. Either way the end is idle
//! afterwards, unless the result reports that the other end dropped.
//! A cancel on an end that is not copying traps with Wasmtime's
//! no-copy-pending message for the direction.
//!
//! Every test drives one component whose synchronous exports each call
//! one built-in on a `stream<u8>` or a `future<u8>`, so a test starts
//! copies and cancels them one call at a time, and reads what arrived
//! in the component's memory.

#![cfg(test)]

use wasm_component_model_polyfill::{
    Component, Engine, EngineConfig, Error, Instance, Linker, Store, Val,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// The word a copy returns when it has not finished.
const BLOCKED: u32 = 0xffff_ffff;

/// The result of a copy that moved what it could.
const COMPLETED: u32 = 0;

/// The result of a copy that found the other end dropped.
const DROPPED: u32 = 1;

/// The result of a copy that was cancelled.
const CANCELLED: u32 = 2;

/// A component whose synchronous exports are the copy, cancel, and
/// drop built-ins of a `stream<u8>` and a `future<u8>`, over one
/// memory the tests read and write through `peek` and `poke`.
///
/// Every copy is declared `async`. The cancels are declared `async`
/// too, and `cancel-read-sync` and `cancel-write-sync` are the
/// stream's cancels without it.
const COPY_CANCELS: &[u8] = component!(
    r#"
    (component
      (core module $libc
        (memory (export "memory") 1)
        (func (export "peek") (param i32) (result i32) (i32.load (local.get 0)))
        (func (export "poke") (param i32 i32) (i32.store (local.get 0) (local.get 1))))
      (core instance $libc (instantiate $libc))

      (type $s (stream u8))
      (type $f (future u8))
      (core func $stream-new (canon stream.new $s))
      (core func $read (canon stream.read $s async (memory (core memory $libc "memory"))))
      (core func $write (canon stream.write $s async (memory (core memory $libc "memory"))))
      (core func $cancel-read (canon stream.cancel-read $s async))
      (core func $cancel-write (canon stream.cancel-write $s async))
      (core func $cancel-read-sync (canon stream.cancel-read $s))
      (core func $cancel-write-sync (canon stream.cancel-write $s))
      (core func $drop-writable (canon stream.drop-writable $s))
      (core func $future-new (canon future.new $f))
      (core func $future-read (canon future.read $f async (memory (core memory $libc "memory"))))
      (core func $future-write
        (canon future.write $f async (memory (core memory $libc "memory"))))
      (core func $future-cancel-read (canon future.cancel-read $f async))
      (core func $future-cancel-write (canon future.cancel-write $f async))

      (core module $m
        (import "" "stream.new" (func $stream-new (result i64)))
        (import "" "stream.read" (func $read (param i32 i32 i32) (result i32)))
        (import "" "stream.write" (func $write (param i32 i32 i32) (result i32)))
        (import "" "stream.cancel-read" (func $cancel-read (param i32) (result i32)))
        (import "" "stream.cancel-write" (func $cancel-write (param i32) (result i32)))
        (import "" "stream.cancel-read-sync" (func $cancel-read-sync (param i32) (result i32)))
        (import "" "stream.cancel-write-sync" (func $cancel-write-sync (param i32) (result i32)))
        (import "" "stream.drop-writable" (func $drop-writable (param i32)))
        (import "" "future.new" (func $future-new (result i64)))
        (import "" "future.read" (func $future-read (param i32 i32) (result i32)))
        (import "" "future.write" (func $future-write (param i32 i32) (result i32)))
        (import "" "future.cancel-read" (func $future-cancel-read (param i32) (result i32)))
        (import "" "future.cancel-write" (func $future-cancel-write (param i32) (result i32)))
        (func (export "new-stream") (result i64) (call $stream-new))
        (func (export "read") (param i32 i32 i32) (result i32)
          (call $read (local.get 0) (local.get 1) (local.get 2)))
        (func (export "write") (param i32 i32 i32) (result i32)
          (call $write (local.get 0) (local.get 1) (local.get 2)))
        (func (export "cancel-read") (param i32) (result i32) (call $cancel-read (local.get 0)))
        (func (export "cancel-write") (param i32) (result i32) (call $cancel-write (local.get 0)))
        (func (export "cancel-read-sync") (param i32) (result i32)
          (call $cancel-read-sync (local.get 0)))
        (func (export "cancel-write-sync") (param i32) (result i32)
          (call $cancel-write-sync (local.get 0)))
        (func (export "drop-writable") (param i32) (call $drop-writable (local.get 0)))
        (func (export "new-future") (result i64) (call $future-new))
        (func (export "future-read") (param i32 i32) (result i32)
          (call $future-read (local.get 0) (local.get 1)))
        (func (export "future-write") (param i32 i32) (result i32)
          (call $future-write (local.get 0) (local.get 1)))
        (func (export "future-cancel-read") (param i32) (result i32)
          (call $future-cancel-read (local.get 0)))
        (func (export "future-cancel-write") (param i32) (result i32)
          (call $future-cancel-write (local.get 0))))
      (core instance $m (instantiate $m (with "" (instance
        (export "stream.new" (func $stream-new))
        (export "stream.read" (func $read))
        (export "stream.write" (func $write))
        (export "stream.cancel-read" (func $cancel-read))
        (export "stream.cancel-write" (func $cancel-write))
        (export "stream.cancel-read-sync" (func $cancel-read-sync))
        (export "stream.cancel-write-sync" (func $cancel-write-sync))
        (export "stream.drop-writable" (func $drop-writable))
        (export "future.new" (func $future-new))
        (export "future.read" (func $future-read))
        (export "future.write" (func $future-write))
        (export "future.cancel-read" (func $future-cancel-read))
        (export "future.cancel-write" (func $future-cancel-write))))))

      (func (export "new-stream") (result u64) (canon lift (core func $m "new-stream")))
      (func (export "read") (param "e" u32) (param "p" u32) (param "n" u32) (result u32)
        (canon lift (core func $m "read")))
      (func (export "write") (param "e" u32) (param "p" u32) (param "n" u32) (result u32)
        (canon lift (core func $m "write")))
      (func (export "cancel-read") (param "e" u32) (result u32)
        (canon lift (core func $m "cancel-read")))
      (func (export "cancel-write") (param "e" u32) (result u32)
        (canon lift (core func $m "cancel-write")))
      (func (export "cancel-read-sync") (param "e" u32) (result u32)
        (canon lift (core func $m "cancel-read-sync")))
      (func (export "cancel-write-sync") (param "e" u32) (result u32)
        (canon lift (core func $m "cancel-write-sync")))
      (func (export "drop-writable") (param "e" u32) (canon lift (core func $m "drop-writable")))
      (func (export "new-future") (result u64) (canon lift (core func $m "new-future")))
      (func (export "future-read") (param "e" u32) (param "p" u32) (result u32)
        (canon lift (core func $m "future-read")))
      (func (export "future-write") (param "e" u32) (param "p" u32) (result u32)
        (canon lift (core func $m "future-write")))
      (func (export "future-cancel-read") (param "e" u32) (result u32)
        (canon lift (core func $m "future-cancel-read")))
      (func (export "future-cancel-write") (param "e" u32) (result u32)
        (canon lift (core func $m "future-cancel-write")))
      (func (export "peek") (param "p" u32) (result u32) (canon lift (core func $libc "peek")))
      (func (export "poke") (param "p" u32) (param "v" u32)
        (canon lift (core func $libc "poke"))))
    "#
);

/// Instantiate `bytes` into a store of its own, with no imports. The
/// engine accepts the built-ins the Component Model gates behind its
/// "more async built-ins" feature, of which an `async` cancel is one.
async fn instantiate(bytes: &[u8]) -> (Store<()>, Instance) {
    let mut config = EngineConfig::new();
    config.wasm_component_model_more_async_builtins(true);
    let engine = Engine::with_config(&config).expect("engine");
    let component = Component::new(&engine, bytes)
        .await
        .expect("the component parses");
    let linker: Linker<()> = Linker::new(&engine);
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

/// Call `name` and expect it to trap, reporting the message.
async fn call_trap(store: &mut Store<()>, instance: &Instance, name: &str, args: &[u32]) -> String {
    match call(store, instance, name, args).await {
        Err(message) => message,
        Ok(value) => panic!("{name} returned {value:?} rather than trapping"),
    }
}

/// Call the `new` export `name` and split the `i64` it returned into
/// the readable end's index and the writable end's.
async fn new_ends(store: &mut Store<()>, instance: &Instance, name: &str) -> (u32, u32) {
    match call(store, instance, name, &[]).await {
        Ok(Some(Val::U64(packed))) => (packed as u32, (packed >> 32) as u32),
        other => panic!("{name} answered {other:?}"),
    }
}

/// The packed result of a copy: `result` in the low four bits and
/// `count` above them.
fn packed(result: u32, count: u32) -> u32 {
    result | (count << 4)
}

/// Wasmtime's message for a cancel of a read that is not pending.
const NO_READ_PENDING: &str = "stream or future read cancelled when no read is pending";

/// Wasmtime's message for a cancel of a write that is not pending.
const NO_WRITE_PENDING: &str = "stream or future write cancelled when no write is pending";

#[wcmp_macros::test]
async fn it_traps_a_cancel_on_an_end_that_is_not_copying() {
    // A trap poisons the store, so each cancel runs against ends of a
    // store of its own.
    for (new, cancel, readable_end, expected) in [
        ("new-stream", "cancel-read", true, NO_READ_PENDING),
        ("new-stream", "cancel-write-sync", false, NO_WRITE_PENDING),
        ("new-future", "future-cancel-read", true, NO_READ_PENDING),
        ("new-future", "future-cancel-write", false, NO_WRITE_PENDING),
    ] {
        let (mut store, instance) = instantiate(COPY_CANCELS).await;
        let (readable, writable) = new_ends(&mut store, &instance, new).await;
        let end = if readable_end { readable } else { writable };
        let message = call_trap(&mut store, &instance, cancel, &[end]).await;
        assert!(message.contains(expected), "`{cancel}`: {message}");
    }
}

#[wcmp_macros::test]
async fn it_traps_a_second_cancel_once_the_first_left_the_end_idle() {
    let (mut store, instance) = instantiate(COPY_CANCELS).await;
    let (readable, _writable) = new_ends(&mut store, &instance, "new-stream").await;
    assert_eq!(
        call_u32(&mut store, &instance, "read", &[readable, 100, 4]).await,
        BLOCKED
    );
    assert_eq!(
        call_u32(&mut store, &instance, "cancel-read", &[readable]).await,
        packed(CANCELLED, 0),
        "the pending read takes the cancelled result"
    );

    let message = call_trap(&mut store, &instance, "cancel-read", &[readable]).await;
    assert!(message.contains(NO_READ_PENDING), "{message}");
}

#[wcmp_macros::test]
async fn it_reports_the_progress_a_pending_write_made_when_it_is_cancelled() {
    let (mut store, instance) = instantiate(COPY_CANCELS).await;
    let (readable, writable) = new_ends(&mut store, &instance, "new-stream").await;
    call_ok(&mut store, &instance, "poke", &[200, 0x0403_0201]).await;

    assert_eq!(
        call_u32(&mut store, &instance, "write", &[writable, 200, 4]).await,
        BLOCKED,
        "the write becomes the pending side"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "read", &[readable, 100, 2]).await,
        packed(COMPLETED, 2),
        "the read takes two of the four values"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "cancel-write", &[writable]).await,
        packed(CANCELLED, 2),
        "the cancel ends the write with the two values it moved"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "peek", &[100]).await & 0xffff,
        0x0201
    );

    // The end is idle again, so the rest of the values go in a
    // write of their own, which a cancel with nothing moved ends
    // with the cancelled result.
    assert_eq!(
        call_u32(&mut store, &instance, "write", &[writable, 202, 2]).await,
        BLOCKED
    );
    assert_eq!(
        call_u32(&mut store, &instance, "cancel-write-sync", &[writable]).await,
        packed(CANCELLED, 0),
        "a synchronous cancel of a pending write that moved nothing is cancelled at once"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "read", &[readable, 100, 2]).await,
        BLOCKED,
        "the cancelled write no longer offers its values"
    );
}

#[wcmp_macros::test]
async fn it_leaves_a_cancelled_future_end_free_to_copy_again() {
    let (mut store, instance) = instantiate(COPY_CANCELS).await;
    let (readable, writable) = new_ends(&mut store, &instance, "new-future").await;
    call_ok(&mut store, &instance, "poke", &[200, 42]).await;

    assert_eq!(
        call_u32(&mut store, &instance, "future-read", &[readable, 100]).await,
        BLOCKED
    );
    assert_eq!(
        call_u32(&mut store, &instance, "future-cancel-read", &[readable]).await,
        CANCELLED
    );
    assert_eq!(
        call_u32(&mut store, &instance, "future-write", &[writable, 200]).await,
        BLOCKED,
        "the cancelled read is not the pending side any more"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "future-read", &[readable, 100]).await,
        COMPLETED,
        "a new read takes the value"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "future-cancel-write", &[writable]).await,
        COMPLETED,
        "the write's cancel returns the completion that was waiting"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "peek", &[100]).await & 0xff,
        42
    );
}

#[wcmp_macros::test]
async fn it_turns_an_undelivered_completion_into_a_drop_once_the_cancelled_writer_drops() {
    let (mut store, instance) = instantiate(COPY_CANCELS).await;
    let (readable, writable) = new_ends(&mut store, &instance, "new-stream").await;

    // A read of two fills from one write, and a second write finds it
    // full: the read completes, and the write takes its place as the
    // pending side. Cancelling the write leaves the writable end idle,
    // while the read still holds its completion undelivered, and is
    // no longer the pending side.
    assert_eq!(
        call_u32(&mut store, &instance, "read", &[readable, 100, 2]).await,
        BLOCKED
    );
    assert_eq!(
        call_u32(&mut store, &instance, "write", &[writable, 200, 2]).await,
        packed(COMPLETED, 2)
    );
    assert_eq!(
        call_u32(&mut store, &instance, "write", &[writable, 200, 3]).await,
        BLOCKED
    );
    assert_eq!(
        call_u32(&mut store, &instance, "cancel-write", &[writable]).await,
        packed(CANCELLED, 0)
    );

    call_ok(&mut store, &instance, "drop-writable", &[writable]).await;

    assert_eq!(
        call_u32(&mut store, &instance, "cancel-read", &[readable]).await,
        packed(DROPPED, 2),
        "the read's completion became the dropped result and kept the two it moved"
    );
    let message = call_trap(&mut store, &instance, "read", &[readable, 100, 2]).await;
    assert!(
        message.contains("cannot read after being notified that the writable end dropped"),
        "{message}"
    );
}
