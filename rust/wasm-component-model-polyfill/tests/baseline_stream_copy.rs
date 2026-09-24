//! Baseline tests for `stream.read` and `stream.write`.
//!
//! A read and a write on the two ends of one stream pair up as the
//! reference's stream state pairs them. The first copy becomes the
//! pending side. A copy that finds the other end pending moves the
//! smaller of the two remaining counts at once and completes, while
//! the pending copy stays pending, keeps taking values, and reports
//! the total when its event is delivered. The event is the triple of
//! the stream code, the end's index, and the packed result: the copy
//! result in the low four bits and the count above them.
//!
//! Most tests drive one component whose synchronous exports each call
//! one built-in on a `stream<u8>`, so a test starts copies, joins
//! ends to a set, and waits, one call at a time, and reads what
//! arrived in the component's memory. The copy budget is proved on
//! two composed components, because the copy of a payload that is
//! not a number runs between two instances. A synchronous read that
//! blocks is proved on two more, whose asynchronous exports the host
//! calls together, so that the write which releases the read is work
//! the suspend seam's nested turn can run. A second such pair has a
//! writer that parks in its event loop after the write, and a read no
//! writer serves fails with the deadlock cause.

#![cfg(test)]

use core::future::{Future, poll_fn};
use core::task::Poll;

use wasm_component_model_polyfill::{
    Component, Engine, EngineConfig, Error, Instance, Linker, Store, Val,
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

/// The code of the event a write on a stream end delivers.
const STREAM_WRITE: u32 = 3;

/// A component whose synchronous exports are the copy built-ins of a
/// `stream<u8>` and a `stream<u32>`, the drops, and the waitable set
/// built-ins, over one memory the tests read and write through `peek`
/// and `poke`.
///
/// `read` and `write` are declared `async`; `read-sync` is not.
/// `wait` and `poll` write the event they deliver at address 0; a
/// poll of a set with no event delivers the none code. `hand-back`
/// returns the readable end at the index it is given to the host,
/// which lifts it as a crossing does.
const STREAM_COPIES: &[u8] = component!(
    r#"
    (component
      (core module $libc
        (memory (export "memory") 1)
        (func (export "peek") (param i32) (result i32) (i32.load (local.get 0)))
        (func (export "poke") (param i32 i32) (i32.store (local.get 0) (local.get 1))))
      (core instance $libc (instantiate $libc))

      (type $s (stream u8))
      (type $wide (stream u32))
      (core func $stream-new (canon stream.new $s))
      (core func $read (canon stream.read $s async (memory (core memory $libc "memory"))))
      (core func $read-sync (canon stream.read $s (memory (core memory $libc "memory"))))
      (core func $write (canon stream.write $s async (memory (core memory $libc "memory"))))
      (core func $drop-readable (canon stream.drop-readable $s))
      (core func $drop-writable (canon stream.drop-writable $s))
      (core func $wide-new (canon stream.new $wide))
      (core func $wide-read (canon stream.read $wide async (memory (core memory $libc "memory"))))
      (core func $wide-write
        (canon stream.write $wide async (memory (core memory $libc "memory"))))
      (core func $set-new (canon waitable-set.new))
      (core func $wait (canon waitable-set.wait (memory (core memory $libc "memory"))))
      (core func $poll (canon waitable-set.poll (memory (core memory $libc "memory"))))
      (core func $join (canon waitable.join))

      (core module $m
        (import "" "stream.new" (func $stream-new (result i64)))
        (import "" "stream.read" (func $read (param i32 i32 i32) (result i32)))
        (import "" "stream.read-sync" (func $read-sync (param i32 i32 i32) (result i32)))
        (import "" "stream.write" (func $write (param i32 i32 i32) (result i32)))
        (import "" "stream.drop-readable" (func $drop-readable (param i32)))
        (import "" "stream.drop-writable" (func $drop-writable (param i32)))
        (import "" "wide.new" (func $wide-new (result i64)))
        (import "" "wide.read" (func $wide-read (param i32 i32 i32) (result i32)))
        (import "" "wide.write" (func $wide-write (param i32 i32 i32) (result i32)))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (import "" "waitable-set.wait" (func $wait (param i32 i32) (result i32)))
        (import "" "waitable-set.poll" (func $poll (param i32 i32) (result i32)))
        (import "" "waitable.join" (func $join (param i32 i32)))
        (func (export "new-stream") (result i64) (call $stream-new))
        (func (export "read") (param i32 i32 i32) (result i32)
          (call $read (local.get 0) (local.get 1) (local.get 2)))
        (func (export "read-sync") (param i32 i32 i32) (result i32)
          (call $read-sync (local.get 0) (local.get 1) (local.get 2)))
        (func (export "write") (param i32 i32 i32) (result i32)
          (call $write (local.get 0) (local.get 1) (local.get 2)))
        (func (export "drop-readable") (param i32) (call $drop-readable (local.get 0)))
        (func (export "drop-writable") (param i32) (call $drop-writable (local.get 0)))
        (func (export "new-wide") (result i64) (call $wide-new))
        (func (export "wide-read") (param i32 i32 i32) (result i32)
          (call $wide-read (local.get 0) (local.get 1) (local.get 2)))
        (func (export "wide-write") (param i32 i32 i32) (result i32)
          (call $wide-write (local.get 0) (local.get 1) (local.get 2)))
        (func (export "new-set") (result i32) (call $set-new))
        (func (export "wait") (param i32) (result i32) (call $wait (local.get 0) (i32.const 0)))
        (func (export "poll") (param i32) (result i32) (call $poll (local.get 0) (i32.const 0)))
        (func (export "join") (param i32 i32) (call $join (local.get 0) (local.get 1)))
        (func (export "hand-back") (param i32) (result i32) (local.get 0)))
      (core instance $m (instantiate $m (with "" (instance
        (export "stream.new" (func $stream-new))
        (export "stream.read" (func $read))
        (export "stream.read-sync" (func $read-sync))
        (export "stream.write" (func $write))
        (export "stream.drop-readable" (func $drop-readable))
        (export "stream.drop-writable" (func $drop-writable))
        (export "wide.new" (func $wide-new))
        (export "wide.read" (func $wide-read))
        (export "wide.write" (func $wide-write))
        (export "waitable-set.new" (func $set-new))
        (export "waitable-set.wait" (func $wait))
        (export "waitable-set.poll" (func $poll))
        (export "waitable.join" (func $join))))))

      (func (export "new-stream") (result u64) (canon lift (core func $m "new-stream")))
      (func (export "read") (param "e" u32) (param "p" u32) (param "n" u32) (result u32)
        (canon lift (core func $m "read")))
      (func (export "read-sync") (param "e" u32) (param "p" u32) (param "n" u32) (result u32)
        (canon lift (core func $m "read-sync")))
      (func (export "write") (param "e" u32) (param "p" u32) (param "n" u32) (result u32)
        (canon lift (core func $m "write")))
      (func (export "drop-readable") (param "e" u32) (canon lift (core func $m "drop-readable")))
      (func (export "drop-writable") (param "e" u32) (canon lift (core func $m "drop-writable")))
      (func (export "new-wide") (result u64) (canon lift (core func $m "new-wide")))
      (func (export "wide-read") (param "e" u32) (param "p" u32) (param "n" u32) (result u32)
        (canon lift (core func $m "wide-read")))
      (func (export "wide-write") (param "e" u32) (param "p" u32) (param "n" u32) (result u32)
        (canon lift (core func $m "wide-write")))
      (func (export "new-set") (result u32) (canon lift (core func $m "new-set")))
      (func (export "wait") (param "s" u32) (result u32) (canon lift (core func $m "wait")))
      (func (export "poll") (param "s" u32) (result u32) (canon lift (core func $m "poll")))
      (func (export "join") (param "w" u32) (param "s" u32) (canon lift (core func $m "join")))
      (func (export "hand-back") (param "e" u32) (result $s)
        (canon lift (core func $m "hand-back")))
      (func (export "peek") (param "p" u32) (result u32) (canon lift (core func $libc "peek")))
      (func (export "poke") (param "p" u32) (param "v" u32)
        (canon lift (core func $libc "poke"))))
    "#
);

/// A reader and a writer, composed, over a `stream` of four nested
/// layers of lists. The writer builds the value the Wasmtime corpus
/// sends in `streams-massive-send.wast`: every layer is two pages of
/// lists that all alias the layer below, and the bottom layer is a
/// byte list of the whole memory, so one value is far larger than
/// any host can build.
///
/// The writer's `run` creates the stream, hands the readable end to
/// the reader's `start-read`, which starts a read of 100 values and
/// returns, and then writes the aliased layers. The read is pending,
/// so the write pairs with it at once and lifts what it moves.
const MASSIVE_SEND: &[u8] = component!(
    r#"
    (component
      (type $t (list (list (list (list u8)))))
      (type $s (stream $t))

      (component $reader
        (core module $libc
          (memory (export "memory") 1)
          (func (export "realloc") (param i32 i32 i32 i32) (result i32) unreachable))
        (core instance $libc (instantiate $libc))
        (core func $read (canon stream.read $s async
          (memory (core memory $libc "memory"))
          (realloc (core func $libc "realloc"))))
        (core module $m
          (import "" "stream.read" (func $read (param i32 i32 i32) (result i32)))
          (func (export "start-read") (param i32) (result i32)
            (call $read (local.get 0) (i32.const 0) (i32.const 100))))
        (core instance $i (instantiate $m (with "" (instance
          (export "stream.read" (func $read))))))
        (func (export "start-read") (param "s" $s) (result u32)
          (canon lift (core func $i "start-read"))))

      (component $writer
        (import "start-read" (func $start-read (param "s" $s) (result u32)))
        (core module $libc (memory (export "memory") 1))
        (core instance $libc (instantiate $libc))
        (core func $stream-new (canon stream.new $s))
        (core func $write (canon stream.write $s async (memory (core memory $libc "memory"))))
        (core func $start-read (canon lower (func $start-read)))
        (core module $m
          (import "libc" "memory" (memory 1))
          (import "" "stream.new" (func $stream-new (result i64)))
          (import "" "stream.write" (func $write (param i32 i32 i32) (result i32)))
          (import "" "start-read" (func $start-read (param i32) (result i32)))
          (func (export "run") (result i32)
            (local $s i64)
            (local.set $s (call $stream-new))
            (if (i32.ne (call $start-read (i32.wrap_i64 (local.get $s))) (i32.const -1))
              (then unreachable))
            (i32.wrap_i64 (i64.shr_u (local.get $s) (i64.const 32)))
            (call $prepare (i32.const 4) (i32.const 2))
            (call $write))
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
            (export "stream.new" (func $stream-new))
            (export "stream.write" (func $write))
            (export "start-read" (func $start-read))))))
        (func (export "run") (result u32) (canon lift (core func $i "run"))))

      (instance $r (instantiate $reader))
      (instance $w (instantiate $writer (with "start-read" (func $r "start-read"))))
      (export "run" (func $w "run")))
    "#
);

/// A reader and a writer, composed, whose asynchronous exports block
/// and release a synchronous read.
///
/// The writer's `setup` creates a `stream<u8>`, keeps the writable
/// end, and hands the readable end to the reader's `take`, which keeps
/// it. The reader's `drain` reads four values synchronously into its
/// memory at 100 and returns the packed result through `task.return`.
/// The writer's `fill` writes four values asynchronously from its
/// memory at 200 and returns the word the write answered. Both are
/// lifted with a callback, so a host call of either may block.
const SEAM_RELEASE: &[u8] = component!(
    r#"
    (component
      (component $reader
        (type $s (stream u8))
        (core module $libc (memory (export "memory") 1))
        (core instance $libc (instantiate $libc))
        (core func $read-sync (canon stream.read $s (memory (core memory $libc "memory"))))
        (core func $task-return (canon task.return (result u32)))
        (core module $m
          (import "libc" "memory" (memory 1))
          (import "" "stream.read" (func $read (param i32 i32 i32) (result i32)))
          (import "" "task.return" (func $task-return (param i32)))
          (global $end (mut i32) (i32.const 0))
          (func (export "take") (param i32) (global.set $end (local.get 0)))
          (func (export "drain") (result i32)
            (call $task-return (call $read (global.get $end) (i32.const 100) (i32.const 4)))
            (i32.const 0))
          (func (export "cb") (param i32 i32 i32) (result i32) unreachable)
          (func (export "peek") (param i32) (result i32) (i32.load (local.get 0))))
        (core instance $i (instantiate $m
          (with "libc" (instance $libc))
          (with "" (instance
            (export "stream.read" (func $read-sync))
            (export "task.return" (func $task-return))))))
        (func (export "take") (param "s" $s) (canon lift (core func $i "take")))
        (func (export "drain") async (result u32)
          (canon lift (core func $i "drain") async (callback (core func $i "cb"))))
        (func (export "peek") (param "p" u32) (result u32) (canon lift (core func $i "peek"))))

      (component $writer
        (type $s (stream u8))
        (import "take" (func $take (param "s" $s)))
        (core module $libc (memory (export "memory") 1))
        (core instance $libc (instantiate $libc))
        (core func $stream-new (canon stream.new $s))
        (core func $write (canon stream.write $s async (memory (core memory $libc "memory"))))
        (core func $take (canon lower (func $take)))
        (core func $task-return (canon task.return (result u32)))
        (core module $m
          (import "libc" "memory" (memory 1))
          (import "" "stream.new" (func $stream-new (result i64)))
          (import "" "stream.write" (func $write (param i32 i32 i32) (result i32)))
          (import "" "take" (func $take (param i32)))
          (import "" "task.return" (func $task-return (param i32)))
          (global $end (mut i32) (i32.const 0))
          (func (export "setup")
            (local $s i64)
            (local.set $s (call $stream-new))
            (global.set $end (i32.wrap_i64 (i64.shr_u (local.get $s) (i64.const 32))))
            (call $take (i32.wrap_i64 (local.get $s))))
          (func (export "fill") (result i32)
            (call $task-return (call $write (global.get $end) (i32.const 200) (i32.const 4)))
            (i32.const 0))
          (func (export "cb") (param i32 i32 i32) (result i32) unreachable)
          (func (export "poke") (param i32 i32) (i32.store (local.get 0) (local.get 1))))
        (core instance $i (instantiate $m
          (with "libc" (instance $libc))
          (with "" (instance
            (export "stream.new" (func $stream-new))
            (export "stream.write" (func $write))
            (export "take" (func $take))
            (export "task.return" (func $task-return))))))
        (func (export "setup") (canon lift (core func $i "setup")))
        (func (export "fill") async (result u32)
          (canon lift (core func $i "fill") async (callback (core func $i "cb"))))
        (func (export "poke") (param "p" u32) (param "v" u32) (canon lift (core func $i "poke"))))

      (instance $r (instantiate $reader))
      (instance $w (instantiate $writer (with "take" (func $r "take"))))
      (export "setup" (func $w "setup"))
      (export "fill" (func $w "fill"))
      (export "poke" (func $w "poke"))
      (export "drain" (func $r "drain"))
      (export "peek" (func $r "peek")))
    "#
);

/// A reader and a writer, composed, whose writer parks in its event
/// loop after the write that releases a synchronous read.
///
/// The writer's `setup` creates a `stream<u8>`, keeps the writable
/// end, and hands the readable end to the reader's `take`, which keeps
/// it. The reader's `drain` reads four values synchronously into its
/// memory at the address it is given and returns the packed result
/// through `task.return`. The writer's `fill-then-park` writes four
/// values asynchronously from its memory at 200 and traps unless the
/// write completed all four at once. It then writes four more from
/// 204, traps unless that write blocked, joins the end to a set, and
/// returns to its event loop waiting on the set. Its callback traps
/// unless the event is the write's, and returns the event's packed
/// result through `task.return`. Both exports are lifted with a
/// callback, so a host call of either may block.
const PARKED_WRITER: &[u8] = component!(
    r#"
    (component
      (component $reader
        (type $s (stream u8))
        (core module $libc (memory (export "memory") 1))
        (core instance $libc (instantiate $libc))
        (core func $read-sync (canon stream.read $s (memory (core memory $libc "memory"))))
        (core func $task-return (canon task.return (result u32)))
        (core module $m
          (import "libc" "memory" (memory 1))
          (import "" "stream.read" (func $read (param i32 i32 i32) (result i32)))
          (import "" "task.return" (func $task-return (param i32)))
          (global $end (mut i32) (i32.const 0))
          (func (export "take") (param i32) (global.set $end (local.get 0)))
          (func (export "drain") (param $at i32) (result i32)
            (call $task-return (call $read (global.get $end) (local.get $at) (i32.const 4)))
            (i32.const 0))
          (func (export "cb") (param i32 i32 i32) (result i32) unreachable)
          (func (export "peek") (param i32) (result i32) (i32.load (local.get 0))))
        (core instance $i (instantiate $m
          (with "libc" (instance $libc))
          (with "" (instance
            (export "stream.read" (func $read-sync))
            (export "task.return" (func $task-return))))))
        (func (export "take") (param "s" $s) (canon lift (core func $i "take")))
        (func (export "drain") async (param "at" u32) (result u32)
          (canon lift (core func $i "drain") async (callback (core func $i "cb"))))
        (func (export "peek") (param "p" u32) (result u32) (canon lift (core func $i "peek"))))

      (component $writer
        (type $s (stream u8))
        (import "take" (func $take (param "s" $s)))
        (core module $libc (memory (export "memory") 1))
        (core instance $libc (instantiate $libc))
        (core func $stream-new (canon stream.new $s))
        (core func $write (canon stream.write $s async (memory (core memory $libc "memory"))))
        (core func $take (canon lower (func $take)))
        (core func $task-return (canon task.return (result u32)))
        (core func $set-new (canon waitable-set.new))
        (core func $join (canon waitable.join))
        (core module $m
          (import "libc" "memory" (memory 1))
          (import "" "stream.new" (func $stream-new (result i64)))
          (import "" "stream.write" (func $write (param i32 i32 i32) (result i32)))
          (import "" "take" (func $take (param i32)))
          (import "" "task.return" (func $task-return (param i32)))
          (import "" "waitable-set.new" (func $set-new (result i32)))
          (import "" "waitable.join" (func $join (param i32 i32)))
          (global $end (mut i32) (i32.const 0))
          (func (export "setup")
            (local $s i64)
            (local.set $s (call $stream-new))
            (global.set $end (i32.wrap_i64 (i64.shr_u (local.get $s) (i64.const 32))))
            (call $take (i32.wrap_i64 (local.get $s))))
          (func (export "fill-then-park") (result i32)
            (local $set i32)
            ;; COMPLETED | (4 << 4): the read was pending and took all four.
            (if (i32.ne (call $write (global.get $end) (i32.const 200) (i32.const 4))
                        (i32.const 0x40))
              (then unreachable))
            ;; BLOCKED: the read is full, so this write waits for the next.
            (if (i32.ne (call $write (global.get $end) (i32.const 204) (i32.const 4))
                        (i32.const -1))
              (then unreachable))
            (local.set $set (call $set-new))
            (call $join (global.get $end) (local.get $set))
            ;; WAIT on the set, with no frame left on the stack.
            (i32.or (i32.shl (local.get $set) (i32.const 4)) (i32.const 2)))
          (func (export "cb") (param $code i32) (param $index i32) (param $payload i32) (result i32)
            ;; The stream write event.
            (if (i32.ne (local.get $code) (i32.const 3)) (then unreachable))
            (call $task-return (local.get $payload))
            (i32.const 0))
          (func (export "poke") (param i32 i32) (i32.store (local.get 0) (local.get 1))))
        (core instance $i (instantiate $m
          (with "libc" (instance $libc))
          (with "" (instance
            (export "stream.new" (func $stream-new))
            (export "stream.write" (func $write))
            (export "take" (func $take))
            (export "task.return" (func $task-return))
            (export "waitable-set.new" (func $set-new))
            (export "waitable.join" (func $join))))))
        (func (export "setup") (canon lift (core func $i "setup")))
        (func (export "fill-then-park") async (result u32)
          (canon lift (core func $i "fill-then-park") async (callback (core func $i "cb"))))
        (func (export "poke") (param "p" u32) (param "v" u32) (canon lift (core func $i "poke"))))

      (instance $r (instantiate $reader))
      (instance $w (instantiate $writer (with "take" (func $r "take"))))
      (export "setup" (func $w "setup"))
      (export "fill-then-park" (func $w "fill-then-park"))
      (export "poke" (func $w "poke"))
      (export "drain" (func $r "drain"))
      (export "peek" (func $r "peek")))
    "#
);

/// Instantiate `bytes` into a store of its own, with no imports. The
/// engine accepts the built-ins the Component Model gates behind its
/// "more async built-ins" feature, of which a synchronous
/// `stream.read` is one.
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

/// Join `end` to a fresh set, poll the set, and answer the code of
/// the event the poll delivered: [`EVENT_NONE`] when the end holds
/// none.
async fn poll_on(store: &mut Store<()>, instance: &Instance, end: u32) -> u32 {
    let set = call_u32(store, instance, "new-set", &[]).await;
    call_ok(store, instance, "join", &[end, set]).await;
    let code = call_u32(store, instance, "poll", &[set]).await;
    call_ok(store, instance, "join", &[end, 0]).await;
    code
}

/// The packed result of a copy: `result` in the low four bits and
/// `count` above them.
fn packed(result: u32, count: u32) -> u32 {
    result | (count << 4)
}

#[wcmp_macros::test]
async fn it_keeps_a_pending_read_taking_writes_until_its_event_is_delivered() {
    let (mut store, instance) = instantiate(STREAM_COPIES).await;
    let (readable, writable) = new_ends(&mut store, &instance, "new-stream").await;
    call_ok(&mut store, &instance, "poke", &[200, 0x0403_0201]).await;
    call_ok(&mut store, &instance, "poke", &[204, 0x0807_0605]).await;

    assert_eq!(
        call_u32(&mut store, &instance, "read", &[readable, 100, 8]).await,
        BLOCKED,
        "the first copy on the stream becomes the pending side"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "write", &[writable, 200, 3]).await,
        packed(0, 3),
        "a write that finds the read pending moves its three values at once"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "write", &[writable, 203, 2]).await,
        packed(0, 2),
        "the read is still pending, so a second write moves its two as well"
    );

    assert_eq!(
        wait_on(&mut store, &instance, readable).await,
        (STREAM_READ, readable, packed(0, 5)),
        "the read's event reports the total both writes moved"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "peek", &[100]).await,
        0x0403_0201,
        "the values landed in order"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "peek", &[104]).await & 0xff,
        0x05
    );

    assert_eq!(
        call_u32(&mut store, &instance, "write", &[writable, 205, 1]).await,
        BLOCKED,
        "once its event is delivered the read is no longer pending"
    );
}

#[wcmp_macros::test]
async fn it_delivers_an_asynchronous_copy_as_the_stream_code_the_index_and_the_packed_result() {
    let (mut store, instance) = instantiate(STREAM_COPIES).await;
    let (readable, writable) = new_ends(&mut store, &instance, "new-stream").await;

    assert_eq!(
        call_u32(&mut store, &instance, "write", &[writable, 200, 4]).await,
        BLOCKED,
        "an asynchronous copy with no partner returns the blocked sentinel"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "read", &[readable, 100, 2]).await,
        packed(0, 2),
        "the read takes two of the four at once"
    );

    assert_eq!(
        wait_on(&mut store, &instance, writable).await,
        (STREAM_WRITE, writable, packed(0, 2)),
        "the write's event arrives through the set with what it moved"
    );
}

#[wcmp_macros::test]
async fn it_completes_a_synchronous_read_at_once_against_a_pending_write() {
    let (mut store, instance) = instantiate(STREAM_COPIES).await;
    let (readable, writable) = new_ends(&mut store, &instance, "new-stream").await;
    call_ok(&mut store, &instance, "poke", &[200, 0x0000_2a2b]).await;

    assert_eq!(
        call_u32(&mut store, &instance, "write", &[writable, 200, 2]).await,
        BLOCKED
    );
    assert_eq!(
        call_u32(&mut store, &instance, "read-sync", &[readable, 100, 8]).await,
        packed(0, 2),
        "the write is pending, so the synchronous read never blocks"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "peek", &[100]).await & 0xffff,
        0x2a2b
    );
}

#[wcmp_macros::test]
async fn it_completes_a_pending_read_with_the_dropped_result_and_its_progress() {
    let (mut store, instance) = instantiate(STREAM_COPIES).await;
    let (readable, writable) = new_ends(&mut store, &instance, "new-stream").await;

    assert_eq!(
        call_u32(&mut store, &instance, "read", &[readable, 100, 8]).await,
        BLOCKED
    );
    assert_eq!(
        call_u32(&mut store, &instance, "write", &[writable, 200, 2]).await,
        packed(0, 2)
    );
    call_ok(&mut store, &instance, "drop-writable", &[writable]).await;

    assert_eq!(
        wait_on(&mut store, &instance, readable).await,
        (STREAM_READ, readable, packed(1, 2)),
        "the drop completes the pending read with the dropped result and the two it took"
    );
    let message = call_trap(&mut store, &instance, "read", &[readable, 100, 8]).await;
    assert!(
        message.contains("cannot read after being notified that the writable end dropped"),
        "a readable end that reported the drop can read no more: {message}"
    );
}

#[wcmp_macros::test]
async fn it_gives_an_idle_readable_end_the_dropped_result_when_the_writable_end_drops() {
    let (mut store, instance) = instantiate(STREAM_COPIES).await;
    let (readable, writable) = new_ends(&mut store, &instance, "new-stream").await;

    call_ok(&mut store, &instance, "drop-writable", &[writable]).await;

    assert_eq!(
        wait_on(&mut store, &instance, readable).await,
        (STREAM_READ, readable, packed(1, 0)),
        "the idle readable end holds the dropped result with nothing moved"
    );
    let message = call_trap(&mut store, &instance, "read", &[readable, 100, 1]).await;
    assert!(
        message.contains("cannot read after being notified that the writable end dropped"),
        "{message}"
    );
    call_ok(&mut store, &instance, "drop-readable", &[readable]).await;
}

#[wcmp_macros::test]
async fn it_gives_an_idle_writable_end_the_dropped_result_when_the_readable_end_drops() {
    let (mut store, instance) = instantiate(STREAM_COPIES).await;
    let (readable, writable) = new_ends(&mut store, &instance, "new-stream").await;

    call_ok(&mut store, &instance, "drop-readable", &[readable]).await;

    assert_eq!(
        wait_on(&mut store, &instance, writable).await,
        (STREAM_WRITE, writable, packed(1, 0))
    );
    let message = call_trap(&mut store, &instance, "write", &[writable, 200, 1]).await;
    assert!(
        message.contains("cannot write after being notified that the readable end dropped"),
        "{message}"
    );
}

#[wcmp_macros::test]
async fn it_answers_a_copy_on_an_idle_end_the_drop_already_reached_with_the_dropped_result() {
    let (mut store, instance) = instantiate(STREAM_COPIES).await;
    let (readable, writable) = new_ends(&mut store, &instance, "new-stream").await;

    call_ok(&mut store, &instance, "drop-writable", &[writable]).await;

    assert_eq!(
        call_u32(&mut store, &instance, "read", &[readable, 100, 4]).await,
        packed(1, 0),
        "the read returns the dropped result at once rather than the blocked sentinel"
    );
}

#[wcmp_macros::test]
async fn it_completes_a_zero_length_copy_against_a_pending_one_with_nothing_moved() {
    let (mut store, instance) = instantiate(STREAM_COPIES).await;
    let (readable, writable) = new_ends(&mut store, &instance, "new-stream").await;

    assert_eq!(
        call_u32(&mut store, &instance, "write", &[writable, 200, 3]).await,
        BLOCKED
    );
    assert_eq!(
        call_u32(&mut store, &instance, "read", &[readable, 100, 0]).await,
        packed(0, 0),
        "a zero-length read is a readiness probe: the write is pending, so it completes"
    );
    assert_eq!(
        poll_on(&mut store, &instance, writable).await,
        EVENT_NONE,
        "the probe is not a move, so the pending write is told nothing"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "read", &[readable, 100, 8]).await,
        packed(0, 3),
        "the probe moved nothing, and the write still holds its three"
    );
    assert_eq!(
        wait_on(&mut store, &instance, writable).await,
        (STREAM_WRITE, writable, packed(0, 3)),
        "the write's one event reports the read that took its three"
    );
}

#[wcmp_macros::test]
async fn it_turns_an_undelivered_completion_into_the_dropped_result_with_its_progress() {
    // The write is pending, the read takes two of its four, and the
    // write's completed event is already in the set it joined when
    // the readable end drops. Delivery then reports the drop, with
    // the two the write moved.
    let (mut store, instance) = instantiate(STREAM_COPIES).await;
    let (readable, writable) = new_ends(&mut store, &instance, "new-stream").await;
    let set = call_u32(&mut store, &instance, "new-set", &[]).await;
    call_ok(&mut store, &instance, "join", &[writable, set]).await;

    assert_eq!(
        call_u32(&mut store, &instance, "write", &[writable, 200, 4]).await,
        BLOCKED
    );
    assert_eq!(
        call_u32(&mut store, &instance, "read", &[readable, 100, 2]).await,
        packed(0, 2)
    );
    call_ok(&mut store, &instance, "drop-readable", &[readable]).await;

    assert_eq!(
        call_u32(&mut store, &instance, "wait", &[set]).await,
        STREAM_WRITE
    );
    assert_eq!(
        call_u32(&mut store, &instance, "peek", &[0]).await,
        writable
    );
    assert_eq!(
        call_u32(&mut store, &instance, "peek", &[4]).await,
        packed(1, 2),
        "the completion the set held became the dropped result and kept its two"
    );
    call_ok(&mut store, &instance, "join", &[writable, 0]).await;
    let message = call_trap(&mut store, &instance, "write", &[writable, 200, 1]).await;
    assert!(
        message.contains("cannot write after being notified that the readable end dropped"),
        "{message}"
    );
}

#[wcmp_macros::test]
async fn it_traps_a_second_copy_on_an_end_whose_first_has_not_been_reported() {
    let (mut store, instance) = instantiate(STREAM_COPIES).await;
    let (readable, _) = new_ends(&mut store, &instance, "new-stream").await;

    assert_eq!(
        call_u32(&mut store, &instance, "read", &[readable, 100, 1]).await,
        BLOCKED
    );
    let message = call_trap(&mut store, &instance, "read", &[readable, 100, 1]).await;

    assert!(
        message.contains("cannot have concurrent operations active on a future/stream"),
        "{message}"
    );
}

#[wcmp_macros::test]
async fn it_traps_a_synchronous_copy_on_an_end_in_a_waitable_set() {
    let (mut store, instance) = instantiate(STREAM_COPIES).await;
    let (readable, _) = new_ends(&mut store, &instance, "new-stream").await;
    let set = call_u32(&mut store, &instance, "new-set", &[]).await;
    call_ok(&mut store, &instance, "join", &[readable, set]).await;

    let message = call_trap(&mut store, &instance, "read-sync", &[readable, 100, 1]).await;

    assert!(
        message.contains("waitable cannot be used synchronously while added to a waitable set"),
        "{message}"
    );
}

#[wcmp_macros::test]
async fn it_traps_a_count_of_two_to_the_twenty_eighth() {
    let (mut store, instance) = instantiate(STREAM_COPIES).await;
    let (readable, _) = new_ends(&mut store, &instance, "new-stream").await;

    let message = call_trap(&mut store, &instance, "read", &[readable, 0, 1 << 28]).await;

    assert!(
        message.contains("stream read/write count too large"),
        "{message}"
    );
}

#[wcmp_macros::test]
async fn it_traps_a_buffer_that_is_misaligned_or_leaves_the_memory_with_wasmtimes_messages() {
    let (mut store, instance) = instantiate(STREAM_COPIES).await;
    let (readable, writable) = new_ends(&mut store, &instance, "new-wide").await;

    let message = call_trap(&mut store, &instance, "wide-read", &[readable, 2, 1]).await;
    assert!(message.contains("read pointer not aligned"), "{message}");
    let message = call_trap(&mut store, &instance, "wide-write", &[writable, 2, 1]).await;
    assert!(message.contains("write pointer not aligned"), "{message}");

    // Wasmtime's bounds check when a copy starts names the read
    // pointer for a read and a write alike.
    let message = call_trap(&mut store, &instance, "wide-write", &[writable, 65532, 2]).await;
    assert!(
        message.contains("read pointer out of bounds of memory"),
        "{message}"
    );
    let message = call_trap(&mut store, &instance, "wide-read", &[readable, 65532, 2]).await;
    assert!(
        message.contains("read pointer out of bounds of memory"),
        "{message}"
    );

    assert_eq!(
        call_u32(&mut store, &instance, "wide-read", &[readable, 3, 0]).await,
        BLOCKED,
        "a copy of no values checks no pointer"
    );
}

#[wcmp_macros::test]
async fn it_traps_a_drop_of_an_end_whose_copy_is_pending() {
    let (mut store, instance) = instantiate(STREAM_COPIES).await;
    let (readable, _) = new_ends(&mut store, &instance, "new-stream").await;
    assert_eq!(
        call_u32(&mut store, &instance, "read", &[readable, 100, 1]).await,
        BLOCKED
    );

    let message = call_trap(&mut store, &instance, "drop-readable", &[readable]).await;

    assert!(message.contains("cannot remove busy stream"), "{message}");
}

#[wcmp_macros::test]
async fn it_traps_a_crossing_of_a_copying_end_in_a_set_as_busy() {
    // The lift checks whether a copy is in progress before it checks
    // for a set, as the reference and Wasmtime do, so an end that
    // started an asynchronous read and then joined a set fails as
    // busy.
    let (mut store, instance) = instantiate(STREAM_COPIES).await;
    let (readable, _) = new_ends(&mut store, &instance, "new-stream").await;
    assert_eq!(
        call_u32(&mut store, &instance, "read", &[readable, 100, 1]).await,
        BLOCKED
    );
    let set = call_u32(&mut store, &instance, "new-set", &[]).await;
    call_ok(&mut store, &instance, "join", &[readable, set]).await;

    let message = call_trap(&mut store, &instance, "hand-back", &[readable]).await;

    assert!(message.contains("cannot remove busy stream"), "{message}");
    assert!(
        !message.contains("waitable set"),
        "the set is not what the crossing names: {message}"
    );
}

#[wcmp_macros::test]
async fn it_fails_a_write_of_a_value_past_the_copy_budget_with_the_budget_cause() {
    let (mut store, instance) = instantiate(MASSIVE_SEND).await;

    let message = call_trap(&mut store, &instance, "run", &[]).await;

    assert!(
        message.contains("fuel allocated for hostcalls has been exhausted"),
        "{message}"
    );
}

#[wcmp_macros::test]
async fn it_blocks_a_synchronous_read_until_a_write_the_nested_turn_runs_releases_it() {
    // The two calls are queued together. The driver's turn starts
    // `drain` first, and its synchronous read finds nothing pending,
    // so it becomes the pending side and blocks through the suspend
    // seam. With no provider the seam runs a nested turn, which
    // starts `fill`. The write finds the read pending and moves its
    // four at once, which releases the read.
    let (mut store, instance) = instantiate(SEAM_RELEASE).await;
    call_ok(&mut store, &instance, "setup", &[]).await;
    call_ok(&mut store, &instance, "poke", &[200, 0x0403_0201]).await;
    let drain = instance.get_func("drain").expect("the drain export");
    let fill = instance.get_func("fill").expect("the fill export");

    let (drained, filled) = store
        .run_concurrent(async |accessor| {
            let mut drained = Box::pin(drain.call_concurrent(accessor, &[]));
            let mut filled = Box::pin(fill.call_concurrent(accessor, &[]));
            let mut drained_done: Option<Result<Box<[Val]>, Error>> = None;
            let mut filled_done: Option<Result<Box<[Val]>, Error>> = None;
            poll_fn(|context| {
                if drained_done.is_none()
                    && let Poll::Ready(value) = drained.as_mut().poll(context)
                {
                    drained_done = Some(value);
                }
                if filled_done.is_none()
                    && let Poll::Ready(value) = filled.as_mut().poll(context)
                {
                    filled_done = Some(value);
                }
                if drained_done.is_some() && filled_done.is_some() {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
            .await;
            (
                drained_done.expect("drain resolved"),
                filled_done.expect("fill resolved"),
            )
        })
        .await
        .expect("the driver returns");

    let filled = filled.unwrap_or_else(|error| panic!("fill failed: {}", chain(&error)));
    assert_eq!(
        filled.as_ref(),
        &[Val::U32(packed(0, 4))],
        "the write found the read pending, so the read had blocked before it"
    );
    let drained = drained.unwrap_or_else(|error| panic!("drain failed: {}", chain(&error)));
    assert_eq!(
        drained.as_ref(),
        &[Val::U32(packed(0, 4))],
        "the read returned once the write the nested turn ran had filled it"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "peek", &[100]).await,
        0x0403_0201
    );
}

#[wcmp_macros::test]
async fn it_releases_a_synchronous_read_through_a_callback_task_of_another_instance_that_writes_and_parks()
 {
    // The driver's turn starts the first `drain`, whose synchronous
    // read finds nothing pending and blocks through the suspend seam.
    // The `drain` task is async-typed, so the nested turn may run
    // ready work of any instance, and it starts `fill-then-park` in
    // the writer. Its first write finds the read pending and fills it,
    // and its second write blocks. The writer then returns to its
    // event loop to wait, leaving no frame of its own on the stack,
    // and the read it filled returns. The second `drain` takes the
    // parked write, whose event resumes the writer.
    let (mut store, instance) = instantiate(PARKED_WRITER).await;
    call_ok(&mut store, &instance, "setup", &[]).await;
    call_ok(&mut store, &instance, "poke", &[200, 0x0403_0201]).await;
    call_ok(&mut store, &instance, "poke", &[204, 0x0807_0605]).await;
    let drain = instance.get_func("drain").expect("the drain export");
    let fill = instance
        .get_func("fill-then-park")
        .expect("the fill-then-park export");
    let into_100 = [Val::U32(100)];
    let into_104 = [Val::U32(104)];

    let (first, parked, second, filled) = store
        .run_concurrent(async |accessor| {
            let mut first = Box::pin(drain.call_concurrent(accessor, &into_100));
            let mut filled = Box::pin(fill.call_concurrent(accessor, &[]));
            let mut filled_done: Option<Result<Box<[Val]>, Error>> = None;
            let first_done = poll_fn(|context| {
                let first_done = first.as_mut().poll(context);
                if filled_done.is_none()
                    && let Poll::Ready(value) = filled.as_mut().poll(context)
                {
                    filled_done = Some(value);
                }
                first_done
            })
            .await;
            let parked = filled_done.is_none();

            let mut second = Box::pin(drain.call_concurrent(accessor, &into_104));
            let mut second_done: Option<Result<Box<[Val]>, Error>> = None;
            poll_fn(|context| {
                if second_done.is_none()
                    && let Poll::Ready(value) = second.as_mut().poll(context)
                {
                    second_done = Some(value);
                }
                if filled_done.is_none()
                    && let Poll::Ready(value) = filled.as_mut().poll(context)
                {
                    filled_done = Some(value);
                }
                if second_done.is_some() && filled_done.is_some() {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
            .await;
            (
                first_done,
                parked,
                second_done.expect("the second drain resolved"),
                filled_done.expect("fill-then-park resolved"),
            )
        })
        .await
        .expect("the driver returns");

    let first = first.unwrap_or_else(|error| panic!("the first drain failed: {}", chain(&error)));
    assert_eq!(
        first.as_ref(),
        &[Val::U32(packed(0, 4))],
        "the write the nested turn ran filled the blocked read"
    );
    assert!(
        parked,
        "the writer was parked in its event loop when the read returned"
    );
    let second =
        second.unwrap_or_else(|error| panic!("the second drain failed: {}", chain(&error)));
    assert_eq!(
        second.as_ref(),
        &[Val::U32(packed(0, 4))],
        "the second read took the parked write at once"
    );
    let filled = filled.unwrap_or_else(|error| panic!("fill-then-park failed: {}", chain(&error)));
    assert_eq!(
        filled.as_ref(),
        &[Val::U32(packed(0, 4))],
        "the parked write's event resumed the writer with its progress"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "peek", &[100]).await,
        0x0403_0201
    );
    assert_eq!(
        call_u32(&mut store, &instance, "peek", &[104]).await,
        0x0807_0605
    );
}

#[wcmp_macros::test]
async fn it_fails_a_synchronous_read_no_writer_ever_serves_with_the_deadlock_cause() {
    // The `drain` task is async-typed and nothing else in the store is
    // ready or pending, so the store is idle and a stack switch would
    // not help.
    let (mut store, instance) = instantiate(PARKED_WRITER).await;
    call_ok(&mut store, &instance, "setup", &[]).await;

    let message = call_trap(&mut store, &instance, "drain", &[100]).await;
    assert!(
        message.contains("deadlock detected: event loop cannot make further progress"),
        "{message}"
    );
}
