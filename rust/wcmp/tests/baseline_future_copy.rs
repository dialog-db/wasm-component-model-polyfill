//! Baseline tests for `future.read` and `future.write`.
//!
//! A future's copy is a stream's copy with a count of one and no
//! partial step. The first copy becomes the pending side, and the copy
//! that meets it moves the one value and completes both. A completed
//! copy and a dropped result both leave the end done, so a future is
//! written at most once and read at most once, and a copy on a done
//! end traps with Wasmtime's message for how it became done. The
//! event is the triple of the future code, the end's index, and the
//! packed result, whose count is always zero.
//!
//! Most tests drive one component whose synchronous exports each call
//! one built-in on a `future<u32>`, so a test starts copies, joins
//! ends to a set, and waits, one call at a time, and reads what
//! arrived in the component's memory. The copy budget is proved on
//! two composed components, because the copy of a payload that is
//! not a number runs between two instances.

#![cfg(test)]

use wcmp::{Component, Engine, EngineConfig, Error, Instance, Linker, Store, Val};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// The word a copy returns when it has not finished.
const BLOCKED: u32 = 0xffff_ffff;

/// The packed result of a copy that completed. A future's count is
/// always zero, so the word is the result code alone.
const COMPLETED: u32 = 0;

/// The packed result of a copy whose other end dropped.
const DROPPED: u32 = 1;

/// The code of the event a read on a future end delivers.
const FUTURE_READ: u32 = 4;

/// The code of the event a write on a future end delivers.
const FUTURE_WRITE: u32 = 5;

/// A component whose synchronous exports are the copy built-ins of a
/// `future<u32>`, the drops, and the waitable set built-ins, over one
/// memory the tests read and write through `peek` and `poke`.
///
/// `read` and `write` are declared `async`; `read-sync` is not.
/// `wait` writes the event it delivers at address 0. `hand-back`
/// returns the readable end at the index it is given to the host,
/// which lifts it as a crossing does.
const FUTURE_COPIES: &[u8] = component!(
    r#"
    (component
      (core module $libc
        (memory (export "memory") 1)
        (func (export "peek") (param i32) (result i32) (i32.load (local.get 0)))
        (func (export "poke") (param i32 i32) (i32.store (local.get 0) (local.get 1))))
      (core instance $libc (instantiate $libc))

      (type $f (future u32))
      (core func $future-new (canon future.new $f))
      (core func $read (canon future.read $f async (memory (core memory $libc "memory"))))
      (core func $read-sync (canon future.read $f (memory (core memory $libc "memory"))))
      (core func $write (canon future.write $f async (memory (core memory $libc "memory"))))
      (core func $drop-readable (canon future.drop-readable $f))
      (core func $drop-writable (canon future.drop-writable $f))
      (core func $set-new (canon waitable-set.new))
      (core func $wait (canon waitable-set.wait (memory (core memory $libc "memory"))))
      (core func $join (canon waitable.join))

      (core module $m
        (import "" "future.new" (func $future-new (result i64)))
        (import "" "future.read" (func $read (param i32 i32) (result i32)))
        (import "" "future.read-sync" (func $read-sync (param i32 i32) (result i32)))
        (import "" "future.write" (func $write (param i32 i32) (result i32)))
        (import "" "future.drop-readable" (func $drop-readable (param i32)))
        (import "" "future.drop-writable" (func $drop-writable (param i32)))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (import "" "waitable-set.wait" (func $wait (param i32 i32) (result i32)))
        (import "" "waitable.join" (func $join (param i32 i32)))
        (func (export "new-future") (result i64) (call $future-new))
        (func (export "read") (param i32 i32) (result i32)
          (call $read (local.get 0) (local.get 1)))
        (func (export "read-sync") (param i32 i32) (result i32)
          (call $read-sync (local.get 0) (local.get 1)))
        (func (export "write") (param i32 i32) (result i32)
          (call $write (local.get 0) (local.get 1)))
        (func (export "drop-readable") (param i32) (call $drop-readable (local.get 0)))
        (func (export "drop-writable") (param i32) (call $drop-writable (local.get 0)))
        (func (export "new-set") (result i32) (call $set-new))
        (func (export "wait") (param i32) (result i32) (call $wait (local.get 0) (i32.const 0)))
        (func (export "join") (param i32 i32) (call $join (local.get 0) (local.get 1)))
        (func (export "hand-back") (param i32) (result i32) (local.get 0)))
      (core instance $m (instantiate $m (with "" (instance
        (export "future.new" (func $future-new))
        (export "future.read" (func $read))
        (export "future.read-sync" (func $read-sync))
        (export "future.write" (func $write))
        (export "future.drop-readable" (func $drop-readable))
        (export "future.drop-writable" (func $drop-writable))
        (export "waitable-set.new" (func $set-new))
        (export "waitable-set.wait" (func $wait))
        (export "waitable.join" (func $join))))))

      (func (export "new-future") (result u64) (canon lift (core func $m "new-future")))
      (func (export "read") (param "e" u32) (param "p" u32) (result u32)
        (canon lift (core func $m "read")))
      (func (export "read-sync") (param "e" u32) (param "p" u32) (result u32)
        (canon lift (core func $m "read-sync")))
      (func (export "write") (param "e" u32) (param "p" u32) (result u32)
        (canon lift (core func $m "write")))
      (func (export "drop-readable") (param "e" u32) (canon lift (core func $m "drop-readable")))
      (func (export "drop-writable") (param "e" u32) (canon lift (core func $m "drop-writable")))
      (func (export "new-set") (result u32) (canon lift (core func $m "new-set")))
      (func (export "wait") (param "s" u32) (result u32) (canon lift (core func $m "wait")))
      (func (export "join") (param "w" u32) (param "s" u32) (canon lift (core func $m "join")))
      (func (export "hand-back") (param "e" u32) (result $f)
        (canon lift (core func $m "hand-back")))
      (func (export "peek") (param "p" u32) (result u32) (canon lift (core func $libc "peek")))
      (func (export "poke") (param "p" u32) (param "v" u32)
        (canon lift (core func $libc "poke"))))
    "#
);

/// A reader and a writer, composed, over a `future` of four nested
/// layers of lists. The writer builds the value the Wasmtime corpus
/// writes in `streams-massive-send.wast`: every layer is two pages of
/// lists that all alias the layer below, and the bottom layer is a
/// byte list of the whole memory, so the one value is far larger than
/// any host can build.
///
/// The writer's `run` creates the future, hands the readable end to
/// the reader's `start-read`, which starts a read and returns, and
/// then writes the aliased layers. The read is pending, so the write
/// pairs with it at once and lifts the value.
const MASSIVE_WRITE: &[u8] = component!(
    r#"
    (component
      (type $t (list (list (list (list u8)))))
      (type $f (future $t))

      (component $reader
        (core module $libc
          (memory (export "memory") 1)
          (func (export "realloc") (param i32 i32 i32 i32) (result i32) unreachable))
        (core instance $libc (instantiate $libc))
        (core func $read (canon future.read $f async
          (memory (core memory $libc "memory"))
          (realloc (core func $libc "realloc"))))
        (core module $m
          (import "" "future.read" (func $read (param i32 i32) (result i32)))
          (func (export "start-read") (param i32) (result i32)
            (call $read (local.get 0) (i32.const 0))))
        (core instance $i (instantiate $m (with "" (instance
          (export "future.read" (func $read))))))
        (func (export "start-read") (param "f" $f) (result u32)
          (canon lift (core func $i "start-read"))))

      (component $writer
        (import "start-read" (func $start-read (param "f" $f) (result u32)))
        (core module $libc (memory (export "memory") 1))
        (core instance $libc (instantiate $libc))
        (core func $future-new (canon future.new $f))
        (core func $write (canon future.write $f async (memory (core memory $libc "memory"))))
        (core func $start-read (canon lower (func $start-read)))
        (core module $m
          (import "libc" "memory" (memory 1))
          (import "" "future.new" (func $future-new (result i64)))
          (import "" "future.write" (func $write (param i32 i32) (result i32)))
          (import "" "start-read" (func $start-read (param i32) (result i32)))
          (func (export "run") (result i32)
            (local $f i64) (local $len i32) (local $base i32)
            (local.set $f (call $future-new))
            (if (i32.ne (call $start-read (i32.wrap_i64 (local.get $f))) (i32.const -1))
              (then unreachable))
            (call $prepare (i32.const 4) (i32.const 2))
            (local.set $len)
            (local.set $base)
            (i32.store offset=0 (i32.const 100) (local.get $base))
            (i32.store offset=4 (i32.const 100) (local.get $len))
            (call $write (i32.wrap_i64 (i64.shr_u (local.get $f) (i64.const 32))) (i32.const 100)))
          ;; `$depth` layers of lists above a byte list of the whole
          ;; memory. Each layer is `$pages` of entries that all alias
          ;; the layer below.
          (func $prepare (param $depth i32) (param $pages i32) (result i32 i32)
            (local $base i32) (local $len i32) (local $c_base i32) (local $c_len i32)
            (local $i i32)
            (if (local.get $depth)
              (then
                (local.set $base (call $grow (local.get $pages)))
                (local.set $len
                  (i32.div_u (i32.mul (local.get $pages) (i32.const 65536)) (i32.const 8)))
                (call $prepare (i32.sub (local.get $depth) (i32.const 1)) (local.get $pages))
                (local.set $c_len)
                (local.set $c_base)
                (loop $l
                  (i32.store offset=0
                    (i32.add (local.get $base) (i32.mul (local.get $i) (i32.const 8)))
                    (local.get $c_base))
                  (i32.store offset=4
                    (i32.add (local.get $base) (i32.mul (local.get $i) (i32.const 8)))
                    (local.get $c_len))
                  (local.set $i (i32.add (local.get $i) (i32.const 1)))
                  (br_if $l (i32.lt_u (local.get $i) (local.get $len)))))
              (else
                (local.set $base (i32.const 0))
                (local.set $len (i32.mul (memory.size) (i32.const 65536)))))
            (local.get $base)
            (local.get $len))
          (func $grow (param i32) (result i32)
            (local $r i32)
            (local.set $r (memory.grow (local.get 0)))
            (if (i32.eq (local.get $r) (i32.const -1)) (then unreachable))
            (i32.mul (local.get $r) (i32.const 65536))))
        (core instance $i (instantiate $m
          (with "libc" (instance $libc))
          (with "" (instance
            (export "future.new" (func $future-new))
            (export "future.write" (func $write))
            (export "start-read" (func $start-read))))))
        (func (export "run") (result u32) (canon lift (core func $i "run"))))

      (instance $r (instantiate $reader))
      (instance $w (instantiate $writer (with "start-read" (func $r "start-read"))))
      (export "run" (func $w "run")))
    "#
);

/// Instantiate `bytes` into a store of its own, with no imports. The
/// engine accepts the built-ins the Component Model gates behind its
/// "more async built-ins" feature, of which a synchronous
/// `future.read` is one.
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

/// Create a future and split the `i64` `future.new` returned into the
/// readable end's index and the writable end's.
async fn new_future(store: &mut Store<()>, instance: &Instance) -> (u32, u32) {
    match call(store, instance, "new-future", &[]).await {
        Ok(Some(Val::U64(packed))) => (packed as u32, (packed >> 32) as u32),
        other => panic!("new-future answered {other:?}"),
    }
}

/// Join `end` to a fresh set, wait on the set, and answer the event
/// it delivered as the triple the guest reads: the code, and the two
/// payloads written at address 0.
async fn wait_on(store: &mut Store<()>, instance: &Instance, end: u32) -> (u32, u32, u32) {
    let set = call_u32(store, instance, "new-set", &[]).await;
    call_ok(store, instance, "join", &[end, set]).await;
    let code = call_u32(store, instance, "wait", &[set]).await;
    let index = call_u32(store, instance, "peek", &[0]).await;
    let packed = call_u32(store, instance, "peek", &[4]).await;
    call_ok(store, instance, "join", &[end, 0]).await;
    (code, index, packed)
}

/// A fresh component with a future whose value moved: a write from
/// address 200 waited for the read into address 100, which completed
/// at once. Answers the readable end and the writable end, both done
/// once the write's event is delivered.
async fn written_and_read() -> (Store<()>, Instance, u32, u32) {
    let (mut store, instance) = instantiate(FUTURE_COPIES).await;
    let (readable, writable) = new_future(&mut store, &instance).await;
    call_ok(&mut store, &instance, "poke", &[200, 42]).await;
    assert_eq!(
        call_u32(&mut store, &instance, "write", &[writable, 200]).await,
        BLOCKED
    );
    assert_eq!(
        call_u32(&mut store, &instance, "read", &[readable, 100]).await,
        COMPLETED
    );
    (store, instance, readable, writable)
}

#[wcmp_macros::test]
async fn it_delivers_an_asynchronous_read_as_the_future_code_the_index_and_a_count_of_zero() {
    let (mut store, instance) = instantiate(FUTURE_COPIES).await;
    let (readable, writable) = new_future(&mut store, &instance).await;
    call_ok(&mut store, &instance, "poke", &[200, 42]).await;

    assert_eq!(
        call_u32(&mut store, &instance, "read", &[readable, 100]).await,
        BLOCKED,
        "an asynchronous read with no writer returns the blocked sentinel"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "write", &[writable, 200]).await,
        COMPLETED,
        "the write finds the read pending and completes at once"
    );

    assert_eq!(
        wait_on(&mut store, &instance, readable).await,
        (FUTURE_READ, readable, COMPLETED),
        "the read's event counts no values, though one moved"
    );
    assert_eq!(call_u32(&mut store, &instance, "peek", &[100]).await, 42);
}

#[wcmp_macros::test]
async fn it_completes_a_pending_write_when_the_read_arrives() {
    let (mut store, instance, _, writable) = written_and_read().await;

    assert_eq!(call_u32(&mut store, &instance, "peek", &[100]).await, 42);
    assert_eq!(
        wait_on(&mut store, &instance, writable).await,
        (FUTURE_WRITE, writable, COMPLETED)
    );
    call_ok(&mut store, &instance, "drop-writable", &[writable]).await;
}

#[wcmp_macros::test]
async fn it_traps_a_second_write_with_the_message_that_names_both_causes() {
    let (mut store, instance, _, writable) = written_and_read().await;
    wait_on(&mut store, &instance, writable).await;

    let message = call_trap(&mut store, &instance, "write", &[writable, 200]).await;

    assert!(
        message.contains(
            "cannot write to future after previous write succeeded or readable end dropped"
        ),
        "{message}"
    );
}

#[wcmp_macros::test]
async fn it_traps_a_second_read_with_the_message_that_names_the_read() {
    let (mut store, instance, readable, _) = written_and_read().await;

    let message = call_trap(&mut store, &instance, "read", &[readable, 100]).await;

    assert!(
        message.contains("cannot read from future after previous read succeeded"),
        "{message}"
    );
}

#[wcmp_macros::test]
async fn it_checks_that_a_read_is_done_before_it_checks_the_waitable_set() {
    // Wasmtime asks whether a synchronous copy's end is in a set only
    // once the copy would block, which is after it asks whether the
    // end is done, so a done end in a set traps as done.
    let (mut store, instance, readable, _) = written_and_read().await;
    let set = call_u32(&mut store, &instance, "new-set", &[]).await;
    call_ok(&mut store, &instance, "join", &[readable, set]).await;

    let message = call_trap(&mut store, &instance, "read-sync", &[readable, 100]).await;

    assert!(
        message.contains("cannot read from future after previous read succeeded"),
        "{message}"
    );
}

#[wcmp_macros::test]
async fn it_traps_the_crossing_of_a_readable_end_that_read_its_value() {
    let (mut store, instance, readable, _) = written_and_read().await;

    let message = call_trap(&mut store, &instance, "hand-back", &[readable]).await;

    assert!(
        message.contains("cannot lift future after previous read succeeded"),
        "{message}"
    );
}

#[wcmp_macros::test]
async fn it_completes_a_write_with_the_dropped_result_once_the_reader_dropped() {
    let (mut store, instance) = instantiate(FUTURE_COPIES).await;
    let (readable, writable) = new_future(&mut store, &instance).await;
    call_ok(&mut store, &instance, "drop-readable", &[readable]).await;

    assert_eq!(
        call_u32(&mut store, &instance, "write", &[writable, 200]).await,
        DROPPED
    );
    let message = call_trap(&mut store, &instance, "write", &[writable, 200]).await;

    // The end was told of the drop, and Wasmtime asks that before it
    // asks whether the future's one write is over.
    assert!(
        message.contains("cannot write after being notified that the readable end dropped"),
        "{message}"
    );
}

#[wcmp_macros::test]
async fn it_completes_a_pending_write_with_the_dropped_result_when_the_reader_drops() {
    let (mut store, instance) = instantiate(FUTURE_COPIES).await;
    let (readable, writable) = new_future(&mut store, &instance).await;
    assert_eq!(
        call_u32(&mut store, &instance, "write", &[writable, 200]).await,
        BLOCKED
    );

    call_ok(&mut store, &instance, "drop-readable", &[readable]).await;

    assert_eq!(
        wait_on(&mut store, &instance, writable).await,
        (FUTURE_WRITE, writable, DROPPED)
    );
    call_ok(&mut store, &instance, "drop-writable", &[writable]).await;
}

#[wcmp_macros::test]
async fn it_keeps_the_completed_read_of_a_future_whose_writer_then_dropped() {
    // The read is the pending side and holds its completed event when
    // the writer, done after its own copy, drops. The value moved, so
    // the read still reports it completed.
    let (mut store, instance) = instantiate(FUTURE_COPIES).await;
    let (readable, writable) = new_future(&mut store, &instance).await;
    call_ok(&mut store, &instance, "poke", &[200, 42]).await;
    assert_eq!(
        call_u32(&mut store, &instance, "read", &[readable, 100]).await,
        BLOCKED
    );
    assert_eq!(
        call_u32(&mut store, &instance, "write", &[writable, 200]).await,
        COMPLETED
    );

    call_ok(&mut store, &instance, "drop-writable", &[writable]).await;

    assert_eq!(
        wait_on(&mut store, &instance, readable).await,
        (FUTURE_READ, readable, COMPLETED)
    );
    assert_eq!(call_u32(&mut store, &instance, "peek", &[100]).await, 42);
    call_ok(&mut store, &instance, "drop-readable", &[readable]).await;
}

#[wcmp_macros::test]
async fn it_fails_a_write_of_a_value_past_the_copy_budget_with_the_budget_cause() {
    let (mut store, instance) = instantiate(MASSIVE_WRITE).await;

    let message = call_trap(&mut store, &instance, "run", &[]).await;

    assert!(
        message.contains("fuel allocated for hostcalls has been exhausted"),
        "{message}"
    );
}
