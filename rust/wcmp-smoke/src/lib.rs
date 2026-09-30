//! The smoke test: one host program that walks through what a
//! developer does with the polyfill, story by story, and reports how
//! each went. A story is a short title, a short goal, and the
//! few values that show it worked, so a reader new to the project, or
//! to a browser, can tell at a glance whether the major features work.
//! `stories` lists them in the order the report tells them.
//!
//! It runs as a native binary (`tests smoke native`) and as a page in
//! the browser (`tests smoke web`) from the same source, so a reader
//! can check the polyfill by reading this file and by running it on
//! both targets.
//!
//! Each story is self-contained: it builds its own store, runs a
//! component, and returns the evidence it observed. A failure in one
//! story does not stop the others, and the report reaches a
//! [`Reporter`] a story at a time, so the page shows each as it
//! completes.

mod clock;
mod host_state;
mod offered;
mod outcome;
mod outside;
mod reporter;
mod step;
mod story;

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use wcmp::{
    Accessor, Component, ComponentValue, CoreExternType, Destination, Engine, EngineConfig, Error,
    FunctionParameter, FunctionType, FutureConsumer, FutureReader, HostCall, Instance,
    InterfaceIdentifier, LinkError, Linker, PrimitiveType, ResourceType, SchedulerCause, Source,
    Store, StoreContext, StreamConsumer, StreamProducer, StreamReader, StreamResult,
    SuspendProviderKind, TaskCause, Val, ValField, ValueType,
};
use wcmp_macros::component;

pub use crate::host_state::HostState;
pub use crate::outcome::Outcome;
pub use crate::outside::Outside;
pub use crate::reporter::Reporter;
pub use crate::step::Step;
pub use crate::story::Story;

/// The `guest` fixture: a component `wasm-tools component new` built
/// from a core module and its WIT. It exports `double`.
const GUEST: &[u8] = include_bytes!("../../wcmp/tests/corpus/fixtures/guest/guest.wasm");

/// The `composition` fixture: two such components joined by `wac plug`.
/// The socket's `run` calls the plug's `double` through an adapter.
const COMPOSITION: &[u8] =
    include_bytes!("../../wcmp/tests/corpus/fixtures/composition/composed.wasm");

/// The `maps` fixture: a `wasm-tools` build whose exports take and
/// return a `map<string, u32>`.
const MAPS: &[u8] = include_bytes!("../../wcmp/tests/corpus/fixtures/maps/maps.wasm");

/// The `fixed-lists` fixture: a `wasm-tools` build whose exports take
/// and return a `list<u32, 4>` and a `list<u8, 16>`.
const FIXED_LISTS: &[u8] =
    include_bytes!("../../wcmp/tests/corpus/fixtures/fixed-lists/fixed-lists.wasm");

/// The `rich` fixture: three components `cargo` and wit-bindgen
/// built against a world of records, variants, enums, flags,
/// options, results, nested lists, strings, and two resources,
/// joined by two `wac plug` steps. Every call it answers has crossed
/// three component boundaries and come back.
const RICH: &[u8] = include_bytes!("../../wcmp/tests/corpus/fixtures/rich/rich.wasm");

/// The `wasi-http` fixture: a WASI 0.3 HTTP handler the same
/// toolchain built. Its export is an `async func` whose request and
/// response carry a `stream<u8>` body and a `future` of trailers,
/// whose types it imports from `wasi:http/types`.
const WASI_HTTP: &[u8] = include_bytes!("../../wcmp/tests/corpus/fixtures/wasi-http/handler.wasm");

/// The `streams` fixture: a component `cargo` and wit-bindgen's
/// async support built. `words` answers with a `stream<string>` and
/// then writes a word at a time into it; `checksum` takes a
/// `stream<u32>`, answers with a `future<u64>`, and resolves it with
/// the sum of each number it read times its one-based position. Both
/// write after the export has returned.
const STREAMS: &[u8] = include_bytes!("../../wcmp/tests/corpus/fixtures/streams/streams.wasm");

/// The `stream-composition` fixture: two components the same
/// toolchain built, joined by `wac plug`. `total` calls the other
/// component's `count-up`, reads the `stream<u32>` it answers with to
/// its end, and returns the sum.
const STREAM_COMPOSITION: &[u8] =
    include_bytes!("../../wcmp/tests/corpus/fixtures/stream-composition/composed.wasm");

/// The `sync-wait` fixture: a component `cargo` and wit-bindgen built
/// whose `async func` import and export are both bound synchronously.
/// `total` calls the host's `host-echo-u32` once per key, through a
/// plain call that returns only once the host has answered, and
/// returns the sum of the answers.
const SYNC_WAIT: &[u8] =
    include_bytes!("../../wcmp/tests/corpus/fixtures/sync-wait/sync-wait.wasm");

/// The `deadline` fixture: an HTTP-style handler `cargo` and
/// wit-bindgen's async support built. `handle` sends the request the
/// host hands it upstream through the host's `fetch`, lending a borrow
/// of it for the call, and races that call against the host's `sleep`.
/// When the timer wins it drops the pending call, which wit-bindgen's
/// runtime cancels with `subtask.cancel`, then drops the request and
/// answers `timeout`.
const DEADLINE: &[u8] = include_bytes!("../../wcmp/tests/corpus/fixtures/deadline/deadline.wasm");

/// The `stats` fixture: a component the same toolchain built, with a
/// bug. `average` divides by the number of values without checking
/// it, so an empty list divides by zero and the guest traps. `sum`
/// has no bug.
const STATS: &[u8] = include_bytes!("../../wcmp/tests/corpus/fixtures/stats/stats.wasm");

/// Two components that pass an `error-context` along: a store whose
/// `put` fails a value over 16 bytes with an error context saying so,
/// and a caller whose `save` writes through the store and, when the
/// store fails the write, reads the error's debug message with
/// `error-context.debug-message` and returns the message and the same
/// error context.
///
/// wit-bindgen's Rust generator lowers an `error-context` that an
/// export returns by borrowing the value's handle, and the value drops
/// its handle with `error-context.drop` before the canonical ABI lifts
/// the export's result, so the lift finds no handle and the guest
/// traps. Both of these components return one, and the flake carries
/// no other toolchain that emits the error-context built-ins, so they
/// are written here by hand. Each keeps its handle; an error context
/// is copied, not moved, when it crosses to another component.
const STORE_AND_CALLER: &[u8] = component!(
    r#"
    (component
      (component $store
        (core module $libc
          (memory (export "memory") 1)
          (global $bump (mut i32) (i32.const 4096))
          (func (export "realloc")
            (param $old i32) (param $old-size i32) (param $align i32) (param $size i32)
            (result i32)
            (local $ptr i32)
            (local.set $ptr
              (i32.and
                (i32.add (global.get $bump) (i32.sub (local.get $align) (i32.const 1)))
                (i32.sub (i32.const 0) (local.get $align))))
            (global.set $bump (i32.add (local.get $ptr) (local.get $size)))
            (local.get $ptr)))
        (core instance $libc (instantiate $libc))
        (core func $new
          (canon error-context.new (memory (core memory $libc "memory"))))
        (core module $m
          (import "libc" "memory" (memory 1))
          (import "" "error-context.new" (func $new (param i32 i32) (result i32)))
          (data (i32.const 0) "cannot write `")
          (data (i32.const 16) "`: the value is over the 16-byte limit")
          ;; The answer lands at 64: the discriminant, then the error
          ;; context's handle at 68. The message is put together at
          ;; 1024 from the two pieces above with the key between.
          (func (export "put")
            (param $key i32) (param $key-len i32) (param $value i32) (param $value-len i32)
            (result i32)
            (if (i32.le_u (local.get $value-len) (i32.const 16))
              (then
                (i32.store8 (i32.const 64) (i32.const 0))
                (return (i32.const 64))))
            (memory.copy (i32.const 1024) (i32.const 0) (i32.const 14))
            (memory.copy (i32.const 1038) (local.get $key) (local.get $key-len))
            (memory.copy
              (i32.add (i32.const 1038) (local.get $key-len)) (i32.const 16) (i32.const 38))
            (i32.store8 (i32.const 64) (i32.const 1))
            (i32.store (i32.const 68)
              (call $new (i32.const 1024) (i32.add (local.get $key-len) (i32.const 52))))
            (i32.const 64)))
        (core instance $m (instantiate $m
          (with "libc" (instance $libc))
          (with "" (instance (export "error-context.new" (func $new))))))
        (func (export "put")
          (param "key" string) (param "value" string) (result (result (error error-context)))
          (canon lift (core func $m "put")
            (memory (core memory $libc "memory"))
            (realloc (core func $libc "realloc")))))
      (instance $store (instantiate $store))

      (component $caller
        (import "put" (func $put
          (param "key" string) (param "value" string) (result (result (error error-context)))))
        (core module $libc
          (memory (export "memory") 1)
          (global $bump (mut i32) (i32.const 4096))
          (func (export "realloc")
            (param $old i32) (param $old-size i32) (param $align i32) (param $size i32)
            (result i32)
            (local $ptr i32)
            (local.set $ptr
              (i32.and
                (i32.add (global.get $bump) (i32.sub (local.get $align) (i32.const 1)))
                (i32.sub (i32.const 0) (local.get $align))))
            (global.set $bump (i32.add (local.get $ptr) (local.get $size)))
            (local.get $ptr)))
        (core instance $libc (instantiate $libc))
        (core func $put (canon lower (func $put) (memory (core memory $libc "memory"))))
        (core func $debug-message
          (canon error-context.debug-message
            (memory (core memory $libc "memory"))
            (realloc (core func $libc "realloc"))))
        (core module $m
          (import "libc" "memory" (memory 1))
          (import "" "put" (func $put (param i32 i32 i32 i32 i32)))
          (import "" "error-context.debug-message" (func $debug-message (param i32 i32)))
          ;; The store's answer lands at 64: the discriminant, then the
          ;; handle at 68. The message's pointer and length land at 72.
          ;; The answer to the host is at 128: the discriminant, then
          ;; the message's pointer at 132, its length at 136, and the
          ;; error context's handle at 140.
          (func (export "save")
            (param $key i32) (param $key-len i32) (param $value i32) (param $value-len i32)
            (result i32)
            (call $put
              (local.get $key) (local.get $key-len)
              (local.get $value) (local.get $value-len)
              (i32.const 64))
            (if (i32.eqz (i32.load8_u (i32.const 64)))
              (then
                (i32.store8 (i32.const 128) (i32.const 0))
                (return (i32.const 128))))
            (call $debug-message (i32.load (i32.const 68)) (i32.const 72))
            (i32.store8 (i32.const 128) (i32.const 1))
            (i32.store (i32.const 132) (i32.load (i32.const 72)))
            (i32.store (i32.const 136) (i32.load (i32.const 76)))
            (i32.store (i32.const 140) (i32.load (i32.const 68)))
            (i32.const 128)))
        (core instance $m (instantiate $m
          (with "libc" (instance $libc))
          (with "" (instance
            (export "put" (func $put))
            (export "error-context.debug-message" (func $debug-message))))))
        (func (export "save")
          (param "key" string) (param "value" string)
          (result (result (error (tuple string error-context))))
          (canon lift (core func $m "save")
            (memory (core memory $libc "memory"))
            (realloc (core func $libc "realloc")))))
      (instance $caller (instantiate $caller (with "put" (func $store "put"))))
      (export "save" (func $caller "save")))
    "#
);

/// The `error-reporter` fixture: a third component, which `cargo` and
/// wit-bindgen built, whose `describe` answers the debug message of
/// the `error-context` it is handed.
const REPORTER: &[u8] =
    include_bytes!("../../wcmp/tests/corpus/fixtures/error-reporter/error-reporter.wasm");

/// A component that exports a core module for the host to take: one
/// global and one function, and nothing the component instantiates
/// itself.
const MODULE_PROVIDER: &[u8] = component!(
    r#"
    (component
      (core module $m
        (global (export "g") i32 i32.const 100)
        (func (export "f") (result i32) i32.const 101))
      (export "m" (core module $m)))
    "#
);

/// A component that imports that core module, instantiates it, and
/// exports the sum of its function's result and its global.
const MODULE_CONSUMER: &[u8] = component!(
    r#"
    (component
      (import "m" (core module $m
        (export "f" (func (result i32)))
        (export "g" (global i32))))
      (core instance $provided (instantiate $m))
      (core module $sum
        (import "m" "f" (func $f (result i32)))
        (import "m" "g" (global $g i32))
        (func (export "sum") (result i32)
          call $f global.get $g i32.add))
      (core instance $i (instantiate $sum (with "m" (instance $provided))))
      (func (export "sum") (result u32) (canon lift (core func $i "sum"))))
    "#
);

/// A component whose functions live under a plain-named instance
/// export, `a`, and under an instance nested inside it, `a.b`.
const NESTED_EXPORTS: &[u8] = component!(
    r#"
    (component
      (core module $m
        (func (export "f") (result i32) i32.const 42)
        (func (export "g") (param i32) (result i32) local.get 0 i32.const 1 i32.add))
      (core instance $i (instantiate $m))
      (func $f (result u32) (canon lift (core func $i "f")))
      (func $g (param "x" u32) (result u32) (canon lift (core func $i "g")))
      (instance $b (export "g" (func $g)))
      (instance $a (export "f" (func $f)) (export "b" (instance $b)))
      (export "a" (instance $a)))
    "#
);

/// A 32-bit component that forwards a string to a 64-bit component,
/// which copies it inside its `i64`-addressed memory and hands it
/// back, so the string crosses an adapter in each direction.
const MEMORY64_COMPOSITION: &[u8] = component!(
    r#"
    (component
      (component $c64
        (core module $m
          (memory (export "memory") i64 1)
          (global $next (mut i64) (i64.const 8))
          (func $realloc (export "realloc")
            (param $old i64) (param $old-size i64) (param $align i64) (param $size i64)
            (result i64)
            (local $ret i64)
            (local.set $ret
              (i64.and (i64.add (global.get $next) (i64.const 7)) (i64.const -8)))
            (global.set $next (i64.add (local.get $ret) (local.get $size)))
            (local.get $ret))
          (func (export "roundtrip") (param $ptr i64) (param $len i64) (result i64)
            (local $dst i64)
            (local $ret i64)
            (local.set $dst
              (call $realloc (i64.const 0) (i64.const 0) (i64.const 1) (local.get $len)))
            (memory.copy (local.get $dst) (local.get $ptr) (local.get $len))
            (local.set $ret
              (call $realloc (i64.const 0) (i64.const 0) (i64.const 8) (i64.const 16)))
            (i64.store (local.get $ret) (local.get $dst))
            (i64.store offset=8 (local.get $ret) (local.get $len))
            (local.get $ret)))
        (core instance $m (instantiate $m))
        (func (export "roundtrip") (param "a" string) (result string)
          (canon lift (core func $m "roundtrip")
            (memory (core memory $m "memory"))
            (realloc (core func $m "realloc")))))
      (instance $c64 (instantiate $c64))
      (component $c32
        (import "backend" (instance $i
          (export "roundtrip" (func (param "a" string) (result string)))))
        (core module $libc
          (memory (export "memory") 1)
          (global $next (mut i32) (i32.const 8))
          (func (export "realloc")
            (param $old i32) (param $old-size i32) (param $align i32) (param $size i32)
            (result i32)
            (local $ret i32)
            (local.set $ret
              (i32.and (i32.add (global.get $next) (i32.const 7)) (i32.const -8)))
            (global.set $next (i32.add (local.get $ret) (local.get $size)))
            (local.get $ret)))
        (core instance $libc (instantiate $libc))
        (core func $roundtrip
          (canon lower (func $i "roundtrip")
            (memory (core memory $libc "memory"))
            (realloc (core func $libc "realloc"))))
        (core module $m
          (import "" "memory" (memory 1))
          (import "" "realloc" (func $realloc (param i32 i32 i32 i32) (result i32)))
          (import "" "roundtrip" (func $roundtrip (param i32 i32 i32)))
          (func (export "roundtrip") (param $ptr i32) (param $len i32) (result i32)
            (local $ret i32)
            (local.set $ret
              (call $realloc (i32.const 0) (i32.const 0) (i32.const 1) (i32.const 8)))
            (call $roundtrip (local.get $ptr) (local.get $len) (local.get $ret))
            (local.get $ret)))
        (core instance $m (instantiate $m
          (with "" (instance
            (export "memory" (memory $libc "memory"))
            (export "realloc" (func $libc "realloc"))
            (export "roundtrip" (func $roundtrip))))))
        (func (export "roundtrip") (param "a" string) (result string)
          (canon lift (core func $m "roundtrip")
            (memory (core memory $libc "memory"))
            (realloc (core func $libc "realloc")))))
      (instance $c32 (instantiate $c32 (with "backend" (instance $c64))))
      (export "roundtrip" (func $c32 "roundtrip")))
    "#
);

/// A component whose import carries the gated `implements`
/// annotation, which an engine accepts only when configured to.
const IMPLEMENTS: &[u8] = component!(
    r#"
    (component (import "a" (implements "a:b/c") (instance)))
    "#
);

/// A component that imports a host function and exports functions
/// whose signatures cross the Canonical ABI in both directions:
/// strings and a list lowered into guest memory through
/// `cabi_realloc`, a string lifted back out, and a scalar handed to
/// the host.
const GREETER: &[u8] = component!(
    r#"
    (component
      (type $host (instance
        (export "tally" (func (param "n" u32)))))
      (import "wcmp:smoke/host@0.1.0" (instance $host (type $host)))
      (alias export $host "tally" (func $tally))
      (core func $core-tally (canon lower (func $tally)))
      (core module $m
        (import "host" "tally" (func $tally (param i32)))
        (memory (export "memory") 1)
        (global $bump (mut i32) (i32.const 1024))
        ;; A bump allocator that honors alignment and moves a block on
        ;; reallocation, which is what the string lowering needs.
        (func $realloc (export "cabi_realloc")
              (param $old i32) (param $old-size i32) (param $align i32) (param $size i32)
              (result i32)
          (local $ptr i32)
          global.get $bump local.get $align i32.add i32.const 1 i32.sub
          local.get $align i32.const 1 i32.sub i32.const -1 i32.xor i32.and
          local.set $ptr
          local.get $ptr local.get $size i32.add global.set $bump
          local.get $old
          i32.eqz
          if
            local.get $ptr
            return
          end
          local.get $ptr
          local.get $old
          local.get $old-size local.get $size
          local.get $old-size local.get $size i32.lt_u
          select
          memory.copy
          local.get $ptr)
        ;; `len(s: string) -> s32`: the byte length of a lowered string.
        (func (export "len") (param i32 i32) (result i32)
          local.get 1)
        ;; `echo(s: string) -> string`: return the lowered string as the
        ;; (pointer, length) pair the lift reads.
        (func (export "echo") (param i32 i32) (result i32)
          (local $ret i32)
          i32.const 0 i32.const 0 i32.const 4 i32.const 8 call $realloc local.set $ret
          local.get $ret local.get 0 i32.store
          local.get $ret local.get 1 i32.store offset=4
          local.get $ret)
        ;; `sum(xs: list<u32>) -> u32`: add up a lowered list.
        (func (export "sum") (param $ptr i32) (param $len i32) (result i32)
          (local $i i32) (local $acc i32)
          block $done
            loop $more
              local.get $i local.get $len i32.ge_u br_if $done
              local.get $acc
              local.get $ptr local.get $i i32.const 4 i32.mul i32.add i32.load
              i32.add local.set $acc
              local.get $i i32.const 1 i32.add local.set $i
              br $more
            end
          end
          local.get $acc)
        ;; `notify(n: u32)`: call the host with twice the argument.
        (func (export "notify") (param i32)
          local.get 0 i32.const 2 i32.mul call $tally))
      (core instance $i (instantiate $m
        (with "host" (instance (export "tally" (func $core-tally))))))
      (func (export "len") (param "s" string) (result s32)
        (canon lift (core func $i "len")
          (memory (core memory $i "memory"))
          (realloc (core func $i "cabi_realloc"))))
      (func (export "echo") (param "s" string) (result string)
        (canon lift (core func $i "echo")
          (memory (core memory $i "memory"))
          (realloc (core func $i "cabi_realloc"))))
      (func (export "sum") (param "xs" (list u32)) (result u32)
        (canon lift (core func $i "sum")
          (memory (core memory $i "memory"))
          (realloc (core func $i "cabi_realloc"))))
      (func (export "notify") (param "n" u32)
        (canon lift (core func $i "notify"))))
    "#
);

/// A component that imports a host resource type and exports a
/// function that drops two owned handles, so the host can watch its
/// destructor run once per handle, in order.
const DROPPER: &[u8] = component!(
    r#"
    (component
      (import "wcmp:smoke/resources@0.1.0" (instance $i
        (export "thing" (type (sub resource)))))
      (alias export $i "thing" (type $thing))
      (core func $thing-drop (canon resource.drop $thing))
      (core module $m
        (func (import "host" "drop") (param i32))
        (func (export "drop2") (param i32 i32)
          local.get 0 call 0
          local.get 1 call 0))
      (core instance $core (instantiate $m
        (with "host" (instance (export "drop" (func $thing-drop))))))
      (func (export "drop2") (param "a" (own $thing)) (param "b" (own $thing))
        (canon lift (core func $core "drop2"))))
    "#
);

/// A component whose stackful export `both` starts two calls to the
/// host's `async` function `fetch`, blocks in `waitable-set.wait`
/// until each has answered, and returns the sum of the answers through
/// `task.return`. Each time the wait wakes it for an answer, it tells
/// the host's `tally` which call answered: 1 for the first, 2 for the
/// second.
///
/// A stackful export is an `async` lift with no `callback`: its core
/// function is plain code that blocks where it stands, and the thread
/// it runs on is set aside while it waits. wit-bindgen's core ABI
/// marks this lift as not supported, and no other toolchain the flake
/// carries emits it, so the component is written here by hand.
const BLOCKING_EXPORT: &[u8] = component!(
    r#"
    (component
      (type $host (instance
        (export "tally" (func (param "n" u32)))))
      (import "wcmp:smoke/host@0.1.0" (instance $host (type $host)))
      (alias export $host "tally" (func $tally))
      (import "fetch" (func $fetch async (param "key" u32) (result u32)))
      (core module $libc (memory (export "memory") 1))
      (core instance $libc (instantiate $libc))
      (core func $tally (canon lower (func $tally)))
      (core func $fetch
        (canon lower (func $fetch) async (memory (core memory $libc "memory"))))
      (core func $set-new (canon waitable-set.new))
      (core func $join (canon waitable.join))
      (core func $wait (canon waitable-set.wait (memory (core memory $libc "memory"))))
      (core func $set-drop (canon waitable-set.drop))
      (core func $subtask-drop (canon subtask.drop))
      (core func $task-return (canon task.return (result u32)))
      (core module $m
        (import "" "memory" (memory 1))
        (import "" "tally" (func $tally (param i32)))
        (import "" "fetch" (func $fetch (param i32 i32) (result i32)))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (import "" "waitable.join" (func $join (param i32 i32)))
        (import "" "waitable-set.wait" (func $wait (param i32 i32) (result i32)))
        (import "" "waitable-set.drop" (func $set-drop (param i32)))
        (import "" "subtask.drop" (func $subtask-drop (param i32)))
        (import "" "task.return" (func $task-return (param i32)))
        ;; Start one call, whose answer lands at `slot`. A call that
        ;; answered at once returns 0; any other joins the set and
        ;; returns its subtask.
        (func $start (param $key i32) (param $slot i32) (param $set i32) (result i32)
          (local $status i32)
          (local $subtask i32)
          (local.set $status (call $fetch (local.get $key) (local.get $slot)))
          (if (result i32) (i32.eq (i32.and (local.get $status) (i32.const 15)) (i32.const 2))
            (then (i32.const 0))
            (else
              (local.set $subtask (i32.shr_u (local.get $status) (i32.const 4)))
              (call $join (local.get $subtask) (local.get $set))
              (local.get $subtask))))
        ;; The answers land at 0 and 4, and each event at 16: the
        ;; subtask at 16 and its new state at 20.
        (func (export "both") (param $a i32) (param $b i32)
          (local $set i32)
          (local $first i32)
          (local $second i32)
          (local $pending i32)
          (local $answered i32)
          (local.set $set (call $set-new))
          (local.set $first (call $start (local.get $a) (i32.const 0) (local.get $set)))
          (local.set $second (call $start (local.get $b) (i32.const 4) (local.get $set)))
          (local.set $pending
            (i32.add
              (i32.ne (local.get $first) (i32.const 0))
              (i32.ne (local.get $second) (i32.const 0))))
          (block $all
            (loop $more
              (br_if $all (i32.eqz (local.get $pending)))
              ;; A subtask event whose state is `returned`.
              (if (i32.and
                    (i32.eq (call $wait (local.get $set) (i32.const 16)) (i32.const 1))
                    (i32.eq (i32.load (i32.const 20)) (i32.const 2)))
                (then
                  (local.set $answered (i32.load (i32.const 16)))
                  (call $tally
                    (select (i32.const 1) (i32.const 2)
                      (i32.eq (local.get $answered) (local.get $first))))
                  (call $join (local.get $answered) (i32.const 0))
                  (call $subtask-drop (local.get $answered))
                  (local.set $pending (i32.sub (local.get $pending) (i32.const 1)))))
              (br $more)))
          (call $set-drop (local.get $set))
          (call $task-return (i32.add (i32.load (i32.const 0)) (i32.load (i32.const 4))))))
      (core instance $i (instantiate $m
        (with "" (instance
          (export "memory" (memory $libc "memory"))
          (export "tally" (func $tally))
          (export "fetch" (func $fetch))
          (export "waitable-set.new" (func $set-new))
          (export "waitable.join" (func $join))
          (export "waitable-set.wait" (func $wait))
          (export "waitable-set.drop" (func $set-drop))
          (export "subtask.drop" (func $subtask-drop))
          (export "task.return" (func $task-return))))))
      (func (export "both") async (param "a" u32) (param "b" u32) (result u32)
        (canon lift (core func $i "both") async)))
    "#
);

/// A component whose stackful export `run` starts three threads of
/// its own and parks and wakes them, telling the host's `tally` each
/// step.
///
/// Thread `n` tells `10 + n` as it starts, parks itself with
/// `thread.suspend`, and tells `20 + n` once another thread woke it;
/// the last to finish wakes the main thread. The main thread lets each
/// thread run up to its park, tells `30`, wakes thread 2 and then
/// thread 1 for later with `thread.resume-later`, switches straight to
/// thread 0 with `thread.suspend-then-resume`, and tells `40` once the
/// last thread woke it. It returns the number of threads that
/// finished.
///
/// wit-bindgen's Rust generator exposes no thread built-in. Its C
/// generator does, but the flake carries no C toolchain, and the
/// flake's `wasm-tools` encodes the thread built-ins under the names
/// and opcodes they had before the Component Model renamed them, so a
/// component it assembled would name other built-ins than the ones the
/// polyfill decodes. The component is written here by hand.
const GUEST_THREADS: &[u8] = component!(
    r#"
    (component
      (type $host (instance
        (export "tally" (func (param "n" u32)))))
      (import "wcmp:smoke/host@0.1.0" (instance $host (type $host)))
      (alias export $host "tally" (func $tally))
      (core module $libc (table (export "__indirect_function_table") 1 funcref))
      (core instance $libc (instantiate $libc))
      (core func $tally (canon lower (func $tally)))
      (core func $task-return (canon task.return (result u32)))
      (core func $thread-index (canon thread.index))
      (core type $start-ty (func (param i32)))
      (alias core export $libc "__indirect_function_table" (core table $table))
      (core func $new-indirect (canon thread.new-indirect $start-ty (core table $table)))
      (core func $resume-later (canon thread.resume-later))
      (core func $suspend (canon thread.suspend))
      (core func $yield-then-resume (canon thread.yield-then-resume))
      (core func $suspend-then-resume (canon thread.suspend-then-resume))
      (core module $m
        (import "" "tally" (func $tally (param i32)))
        (import "" "task.return" (func $task-return (param i32)))
        (import "" "thread.index" (func $thread-index (result i32)))
        (import "" "thread.new-indirect" (func $new-indirect (param i32 i32) (result i32)))
        (import "" "thread.resume-later" (func $resume-later (param i32)))
        (import "" "thread.suspend" (func $suspend (result i32)))
        (import "" "thread.yield-then-resume" (func $yield-then-resume (param i32) (result i32)))
        (import "" "thread.suspend-then-resume" (func $suspend-then-resume (param i32) (result i32)))
        (import "libc" "__indirect_function_table" (table 1 funcref))
        (global $main (mut i32) (i32.const 0))
        (global $running (mut i32) (i32.const 0))
        (global $finished (mut i32) (i32.const 0))
        (func $worker (param $n i32)
          (call $tally (i32.add (i32.const 10) (local.get $n)))
          (drop (call $suspend))
          (call $tally (i32.add (i32.const 20) (local.get $n)))
          (global.set $finished (i32.add (global.get $finished) (i32.const 1)))
          (global.set $running (i32.sub (global.get $running) (i32.const 1)))
          (if (i32.eqz (global.get $running))
            (then (call $resume-later (global.get $main)))))
        (elem (table 0) (i32.const 0) func $worker)
        (func (export "run")
          (local $t0 i32)
          (local $t1 i32)
          (local $t2 i32)
          (global.set $main (call $thread-index))
          (global.set $running (i32.const 3))
          (global.set $finished (i32.const 0))
          (local.set $t0 (call $new-indirect (i32.const 0) (i32.const 0)))
          (local.set $t1 (call $new-indirect (i32.const 0) (i32.const 1)))
          (local.set $t2 (call $new-indirect (i32.const 0) (i32.const 2)))
          (drop (call $yield-then-resume (local.get $t0)))
          (drop (call $yield-then-resume (local.get $t1)))
          (drop (call $yield-then-resume (local.get $t2)))
          (call $tally (i32.const 30))
          (call $resume-later (local.get $t2))
          (call $resume-later (local.get $t1))
          (drop (call $suspend-then-resume (local.get $t0)))
          (call $tally (i32.const 40))
          (call $task-return (global.get $finished))))
      (core instance $i (instantiate $m
        (with "" (instance
          (export "tally" (func $tally))
          (export "task.return" (func $task-return))
          (export "thread.index" (func $thread-index))
          (export "thread.new-indirect" (func $new-indirect))
          (export "thread.resume-later" (func $resume-later))
          (export "thread.suspend" (func $suspend))
          (export "thread.yield-then-resume" (func $yield-then-resume))
          (export "thread.suspend-then-resume" (func $suspend-then-resume))))
        (with "libc" (instance $libc))))
      (func (export "run") async (result u32)
        (canon lift (core func $i "run") async)))
    "#
);

/// Two components, as a C library that uses pthreads and the program
/// that calls it would be.
///
/// The library's stackful export `work` starts a worker thread and
/// returns, leaving the worker to finish the call. The worker yields
/// with the cancellable form of `thread.yield` and tells the host's
/// `tally` what each yield answered: 0 while nothing has asked it to
/// stop, and 1 once its caller has. On 1 it confirms with
/// `task.cancel`. It gives up after `rounds` yields and returns how
/// many it made.
///
/// The program's export `run` is lifted with a callback, as
/// wit-bindgen lifts an `async` export. It calls `work` through an
/// asynchronous lower and gives way `yields` times, answering its
/// callback loop with a yield, so the worker runs. Then it tells the
/// host 50, cancels the call with the asynchronous `subtask.cancel`,
/// waits for the call to resolve if the cancel answered that it has
/// not yet, and returns the state the call resolved to: 4,
/// `CANCELLED_BEFORE_RETURNED`, when the worker confirmed, and 2,
/// `RETURNED`, when it gave up first.
///
/// wit-bindgen's C generator emits the cancellable yield, but the
/// flake carries no C toolchain, and its Rust generator exposes no
/// thread built-in. So the component is written here by hand. The
/// text format no longer spells the `cancellable` immediate, so it is
/// assembled with the yield's immediate byte zero, and
/// [`with_cancellable_yield`] sets it, as a toolchain that emits the
/// immediate encodes it. The worker's yield is the only `thread.yield`
/// in the binary.
const CANCELLED_WORKER: &[u8] = component!(
    r#"
    (component
      (type $host (instance
        (export "tally" (func (param "n" u32)))))
      (import "wcmp:smoke/host@0.1.0" (instance $host (type $host)))
      (alias export $host "tally" (func $tally))

      (component $library
        (import "tally" (func $tally (param "n" u32)))
        (core module $libc (table (export "table") 1 funcref))
        (core instance $libc (instantiate $libc))
        (alias core export $libc "table" (core table $table))
        (core type $start-ty (func (param i32)))
        (core func $tally (canon lower (func $tally)))
        (core func $yield (canon thread.yield))
        (core func $task-cancel (canon task.cancel))
        (core func $task-return (canon task.return (result u32)))
        (core func $new-indirect (canon thread.new-indirect $start-ty (core table $table)))
        (core func $resume-later (canon thread.resume-later))
        (core module $m
          (import "" "tally" (func $tally (param i32)))
          (import "" "thread.yield" (func $yield (result i32)))
          (import "" "task.cancel" (func $task-cancel))
          (import "" "task.return" (func $task-return (param i32)))
          (import "" "thread.new-indirect" (func $new-indirect (param i32 i32) (result i32)))
          (import "" "thread.resume-later" (func $resume-later (param i32)))
          (import "libc" "table" (table 1 funcref))
          (func $worker (param $rounds i32)
            (local $made i32)
            (local $answer i32)
            (block $gave-up
              (loop $more
                (br_if $gave-up (i32.ge_u (local.get $made) (local.get $rounds)))
                (local.set $answer (call $yield))
                (call $tally (local.get $answer))
                (if (local.get $answer)
                  (then
                    (call $task-cancel)
                    (return)))
                (local.set $made (i32.add (local.get $made) (i32.const 1)))
                (br $more)))
            (call $task-return (local.get $made)))
          (elem (table 0) (i32.const 0) func $worker)
          (func (export "work") (param $rounds i32)
            (call $resume-later (call $new-indirect (i32.const 0) (local.get $rounds)))))
        (core instance $m (instantiate $m
          (with "" (instance
            (export "tally" (func $tally))
            (export "thread.yield" (func $yield))
            (export "task.cancel" (func $task-cancel))
            (export "task.return" (func $task-return))
            (export "thread.new-indirect" (func $new-indirect))
            (export "thread.resume-later" (func $resume-later))))
          (with "libc" (instance $libc))))
        (func (export "work") async (param "rounds" u32) (result u32)
          (canon lift (core func $m "work") async)))
      (instance $library (instantiate $library (with "tally" (func $tally))))

      (component $program
        (import "tally" (func $tally (param "n" u32)))
        (import "work" (func $work async (param "rounds" u32) (result u32)))
        (core module $libc (memory (export "memory") 1))
        (core instance $libc (instantiate $libc))
        (core func $tally (canon lower (func $tally)))
        (core func $work
          (canon lower (func $work) async (memory (core memory $libc "memory"))))
        (core func $cancel (canon subtask.cancel async))
        (core func $subtask-drop (canon subtask.drop))
        (core func $set-new (canon waitable-set.new))
        (core func $join (canon waitable.join))
        (core func $set-drop (canon waitable-set.drop))
        (core func $task-return (canon task.return (result u32)))
        (core module $m
          (import "" "tally" (func $tally (param i32)))
          (import "" "work" (func $work (param i32 i32) (result i32)))
          (import "" "subtask.cancel" (func $cancel (param i32) (result i32)))
          (import "" "subtask.drop" (func $subtask-drop (param i32)))
          (import "" "waitable-set.new" (func $set-new (result i32)))
          (import "" "waitable.join" (func $join (param i32 i32)))
          (import "" "waitable-set.drop" (func $set-drop (param i32)))
          (import "" "task.return" (func $task-return (param i32)))
          (global $subtask (mut i32) (i32.const 0))
          (global $yields (mut i32) (i32.const 0))
          (global $set (mut i32) (i32.const 0))
          ;; The call has started once `work` returns: the worker runs
          ;; on, and the answer would land at 0. Give way.
          (func (export "run") (param $yields i32) (result i32)
            (global.set $yields (local.get $yields))
            (global.set $subtask
              (i32.shr_u (call $work (i32.const 1000) (i32.const 0)) (i32.const 4)))
            (i32.const 1 (; YIELD ;)))
          ;; Give way until the yields run out, then cancel. A subtask
          ;; event is the cancelled call resolving.
          (func (export "run-callback") (param $event i32) (param $index i32) (param $state i32)
            (result i32)
            (if (i32.eq (local.get $event) (i32.const 1 (; SUBTASK ;)))
              (then
                (call $join (global.get $subtask) (i32.const 0))
                (call $set-drop (global.get $set))
                (return (call $finish (local.get $state)))))
            (if (global.get $yields)
              (then
                (global.set $yields (i32.sub (global.get $yields) (i32.const 1)))
                (return (i32.const 1 (; YIELD ;)))))
            (call $tally (i32.const 50))
            (local.set $state (call $cancel (global.get $subtask)))
            (if (i32.ne (local.get $state) (i32.const -1 (; BLOCKED ;)))
              (then (return (call $finish (local.get $state)))))
            (global.set $set (call $set-new))
            (call $join (global.get $subtask) (global.get $set))
            (i32.or (i32.const 2 (; WAIT ;)) (i32.shl (global.get $set) (i32.const 4))))
          (func $finish (param $state i32) (result i32)
            (call $subtask-drop (global.get $subtask))
            (call $task-return (local.get $state))
            (i32.const 0 (; EXIT ;))))
        (core instance $m (instantiate $m
          (with "" (instance
            (export "tally" (func $tally))
            (export "work" (func $work))
            (export "subtask.cancel" (func $cancel))
            (export "subtask.drop" (func $subtask-drop))
            (export "waitable-set.new" (func $set-new))
            (export "waitable.join" (func $join))
            (export "waitable-set.drop" (func $set-drop))
            (export "task.return" (func $task-return))))))
        (func (export "run") async (param "yields" u32) (result u32)
          (canon lift (core func $m "run") async (callback (core func $m "run-callback")))))
      (instance $program (instantiate $program
        (with "tally" (func $tally))
        (with "work" (func $library "work"))))
      (export "run" (func $program "run")))
    "#
);

/// A story's body: what it does against the engine, and the evidence
/// it returns or the reason it failed.
type Body<'a> = Pin<Box<dyn Future<Output = Result<String, String>> + 'a>>;

pub static ENGINE_AND_STORE: Story = Story {
    chapter: "Getting started",
    title: "Create an engine and a store",
    goal: "Set up the polyfill and keep your own state in the store.",
};

pub static LOAD_AND_CALL: Story = Story {
    chapter: "Getting started",
    title: "Load a component and call it",
    goal: "Load a component that `wasm-tools` built and call an export with Rust types.",
};

pub static COMPOSITION_STORY: Story = Story {
    chapter: "Getting started",
    title: "Run a `wac` composition",
    goal: "Two components joined by `wac plug` run as one, and a call passes between them.",
};

pub static HOST_FUNCTION: Story = Story {
    chapter: "Host integration",
    title: "Pass strings and lists, and provide a host function",
    goal: "Send strings and lists into a guest, and let it call a Rust function you provide.",
};

pub static HOST_RESOURCE: Story = Story {
    chapter: "Host integration",
    title: "Hand the guest a host resource",
    goal: "The guest drops handles to a resource you own, and your destructor runs for each.",
};

pub static DISPOSAL: Story = Story {
    chapter: "Host integration",
    title: "Clean up handles, instances, and stores",
    goal: "Release a handle yourself, then drop the instance and the store.",
};

pub static CORE_MODULES: Story = Story {
    chapter: "Host integration",
    title: "Share a core module between components",
    goal: "Instantiate a core module that one component exports, and hand it to another.",
};

pub static MAPS_AND_FIXED_LISTS: Story = Story {
    chapter: "Real toolchains, real types",
    title: "Pass maps and fixed-length lists",
    goal: "A `map<string, u32>` and fixed-length lists go into a component and come back.",
};

pub static RICH_WORLD: Story = Story {
    chapter: "Real toolchains, real types",
    title: "Use records, variants, and resources from wit-bindgen",
    goal: "Call three `cargo`-built components that pass every kind of WIT type between \
           them.",
};

pub static MEMORY64: Story = Story {
    chapter: "Real toolchains, real types",
    title: "Cross into a 64-bit memory",
    goal: "Pass a string to a component that uses 64-bit memory, and get it back.",
};

pub static NAVIGATION: Story = Story {
    chapter: "Introspection and configuration",
    title: "Find exports by name and read their signatures",
    goal: "Walk to a nested export by name and read its types before you call it.",
};

pub static ENGINE_CONFIGURATION: Story = Story {
    chapter: "Introspection and configuration",
    title: "Turn on an off-by-default feature",
    goal: "The default engine refuses a gated feature and names it; a configured engine \
           accepts it.",
};

pub static SUSPEND_PROVIDER: Story = Story {
    chapter: "Introspection and configuration",
    title: "Check how this engine pauses a waiting guest",
    goal: "Ask the engine how it pauses a waiting guest: stack switching natively, JSPI in a \
           browser, or not at all.",
};

pub static RUN_CONCURRENT: Story = Story {
    chapter: "Async hosts",
    title: "Await outside the store without blocking it",
    goal: "Your async host code awaits a timer, and the store waits instead of reporting a \
           deadlock.",
};

pub static READ_A_GUEST_STREAM: Story = Story {
    chapter: "Streams and futures",
    title: "Read a stream a guest returns",
    goal: "A guest returns a `stream<string>` and keeps writing to it; you read every word.",
};

pub static STREAM_IN_FUTURE_OUT: Story = Story {
    chapter: "Streams and futures",
    title: "Feed a guest a stream and await its future",
    goal: "Send a guest a `stream<u32>` and await the `future<u64>` it answers with.",
};

pub static STREAM_BETWEEN_COMPONENTS: Story = Story {
    chapter: "Streams and futures",
    title: "Stream numbers from one component to another",
    goal: "One component streams 1 to 1000 to another, which adds them up.",
};

pub static WAIT_FOR_THE_HOST: Story = Story {
    chapter: "Guests that wait",
    title: "Let synchronous guest code wait for an `async` host function",
    goal: "A guest makes a plain blocking call to your `async` host function and pauses until \
           you answer. Without JSPI it fails cleanly instead.",
};

pub static BLOCK_AND_RESUME: Story = Story {
    chapter: "Guests that wait",
    title: "Run an export that blocks until its answers arrive",
    goal: "A guest export starts two host calls and blocks until both answer. Without JSPI it \
           fails cleanly instead.",
};

pub static GUEST_THREADS_STORY: Story = Story {
    chapter: "Guests that wait",
    title: "Park and wake guest threads",
    goal: "A guest starts three threads that park and wake each other, like pthreads. Without \
           JSPI it fails cleanly instead.",
};

pub static CANCEL_A_SLOW_HOST_CALL: Story = Story {
    chapter: "Failure and cancellation",
    title: "Let a guest give up on a slow host call",
    goal: "A guest gives a host call a deadline and cancels it when the deadline passes. Works \
           without JSPI too.",
};

pub static TRAP_LOSES_THE_STORE: Story = Story {
    chapter: "Failure and cancellation",
    title: "Recover from a trap with a new store",
    goal: "A guest bug traps. That store refuses further calls, and a new store works.",
};

pub static ERROR_BETWEEN_COMPONENTS: Story = Story {
    chapter: "Failure and cancellation",
    title: "Pass an error from one component to another",
    goal: "Turn on `error-context`, and an error's message travels between components intact.",
};

pub static STOP_A_GUEST_THREAD: Story = Story {
    chapter: "Failure and cancellation",
    title: "Stop a guest thread when its caller cancels",
    goal: "Cancel a call, and the worker thread it started sees the cancel and stops. Works \
           without JSPI too.",
};

pub static WASI_HTTP_STORY: Story = Story {
    chapter: "Known limits",
    title: "Get a clear error for a missing `wasi:http` host",
    goal: "Load a `wasi:http` 0.3 handler without a `wasi:http/types` host, and see which \
           import is missing.",
};

pub static SUSPENDING_OFF: Story = Story {
    chapter: "Known limits",
    title: "Turn suspending off and see why a wait fails",
    goal: "With suspending off, calls that must wait fail with a clear error instead of \
           hanging.",
};

/// Every story in the order the report tells them, each with its
/// body. A body is a future and does nothing until `run` awaits it.
fn stories(engine: &Engine) -> Vec<(&'static Story, Body<'_>)> {
    vec![
        (&ENGINE_AND_STORE, Box::pin(engine_and_store(engine))),
        (&LOAD_AND_CALL, Box::pin(load_and_call(engine))),
        (&COMPOSITION_STORY, Box::pin(composition(engine))),
        (&HOST_FUNCTION, Box::pin(host_function(engine))),
        (&HOST_RESOURCE, Box::pin(host_resource(engine))),
        (&DISPOSAL, Box::pin(disposal(engine))),
        (&CORE_MODULES, Box::pin(core_modules(engine))),
        (
            &MAPS_AND_FIXED_LISTS,
            Box::pin(maps_and_fixed_lists(engine)),
        ),
        (&RICH_WORLD, Box::pin(rich_world(engine))),
        (&MEMORY64, Box::pin(memory64(engine))),
        (&NAVIGATION, Box::pin(navigation(engine))),
        (&ENGINE_CONFIGURATION, Box::pin(engine_configuration())),
        (&SUSPEND_PROVIDER, Box::pin(suspend_provider(engine))),
        (&RUN_CONCURRENT, Box::pin(run_concurrent_outside(engine))),
        (&READ_A_GUEST_STREAM, Box::pin(read_a_guest_stream(engine))),
        (
            &STREAM_IN_FUTURE_OUT,
            Box::pin(stream_in_future_out(engine)),
        ),
        (
            &STREAM_BETWEEN_COMPONENTS,
            Box::pin(stream_between_components(engine)),
        ),
        (&WAIT_FOR_THE_HOST, Box::pin(wait_for_the_host_here(engine))),
        (&BLOCK_AND_RESUME, Box::pin(block_and_resume())),
        (&GUEST_THREADS_STORY, Box::pin(guest_threads())),
        (
            &CANCEL_A_SLOW_HOST_CALL,
            Box::pin(cancel_a_slow_host_call(engine)),
        ),
        (
            &TRAP_LOSES_THE_STORE,
            Box::pin(trap_loses_the_store(engine)),
        ),
        (
            &ERROR_BETWEEN_COMPONENTS,
            Box::pin(error_between_components(engine)),
        ),
        (&STOP_A_GUEST_THREAD, Box::pin(stop_a_guest_thread())),
        (&WASI_HTTP_STORY, Box::pin(wasi_http(engine))),
        (&SUSPENDING_OFF, Box::pin(suspending_off())),
    ]
}

/// Run every story in order and return the report, telling `reporter`
/// about each chapter and story as it completes. Compiling,
/// instantiating, and calling are awaited, so the browser can compile
/// through its asynchronous API and paint between stories; natively
/// the futures complete at once.
pub async fn run(reporter: &mut impl Reporter) -> Vec<Step> {
    let engine = match configured_engine(&EngineConfig::new()) {
        Ok(engine) => engine,
        Err(err) => {
            let step = Step {
                story: &ENGINE_AND_STORE,
                outcome: Outcome::Failed(format!("Engine::with_backend failed: {err}")),
                millis: 0.0,
            };
            reporter.begin(1);
            reporter.chapter(step.story.chapter);
            reporter.step(&step);
            return vec![step];
        }
    };
    let stories = stories(&engine);
    reporter.begin(stories.len());
    let mut steps = Vec::with_capacity(stories.len());
    let mut chapter = "";
    for (story, body) in stories {
        if story.chapter != chapter {
            chapter = story.chapter;
            reporter.chapter(chapter);
        }
        let step = Step::run(story, body).await;
        reporter.step(&step);
        steps.push(step);
    }
    steps
}

/// A chapter's line of the transcript.
pub fn chapter_line(chapter: &str) -> String {
    format!("== {chapter}")
}

/// How many stories passed, failed, and were skipped, in that order.
pub fn counts(steps: &[Step]) -> (usize, usize, usize) {
    let count =
        |wanted: fn(&Outcome) -> bool| steps.iter().filter(|step| wanted(&step.outcome)).count();
    (
        count(|outcome| matches!(outcome, Outcome::Passed(_))),
        count(|outcome| matches!(outcome, Outcome::Failed(_))),
        count(|outcome| matches!(outcome, Outcome::Skipped(_))),
    )
}

/// The report's last line. `tests smoke check` compares it between
/// the native run and the page.
pub fn summary(steps: &[Step]) -> String {
    let (passed, failed, skipped) = counts(steps);
    format!("smoke: {passed} passed, {failed} failed, {skipped} skipped")
}

pub fn all_passed(steps: &[Step]) -> bool {
    !steps
        .iter()
        .any(|step| matches!(step.outcome, Outcome::Failed(_)))
}

fn fail(err: impl std::fmt::Display) -> String {
    err.to_string()
}

/// An engine configured with `config`, over the backend the smoke test
/// runs on: Wasmtime natively, and the browser's own engine in a page.
/// The polyfill has no backend of its own, so the host names one.
fn configured_engine(config: &EngineConfig) -> Result<Engine, Error> {
    #[cfg(not(target_arch = "wasm32"))]
    let backend = wcmp_wasm_core_wasmtime::Wasmtime::new().map_err(|error| Error::Internal {
        message: format!("Wasmtime makes no engine: {error}"),
    })?;
    #[cfg(target_arch = "wasm32")]
    let backend = wcmp_wasm_core_web::Web::new();
    Engine::with_backend(backend)?.with_config(config)
}

fn expect<T: PartialEq + std::fmt::Debug>(what: &str, got: T, wanted: T) -> Result<(), String> {
    if got == wanted {
        Ok(())
    } else {
        Err(format!("{what}: expected {wanted:?}, got {got:?}"))
    }
}

/// An engine and a store construct through the public API and the
/// store hands its data back.
async fn engine_and_store(engine: &Engine) -> Result<String, String> {
    let mut store: Store<HostState> = Store::new(engine, HostState::default()).map_err(fail)?;
    store.data_mut().tallies.push(7);
    expect("store data", store.data().tallies.as_slice(), &[7])?;
    Ok("wrote host state into the store and read it back".to_owned())
}

/// A component built by a real toolchain loads, instantiates, and
/// answers a typed call.
async fn load_and_call(engine: &Engine) -> Result<String, String> {
    let component = Component::new(engine, GUEST).await.map_err(fail)?;
    let linker: Linker<HostState> = Linker::new(engine);
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .map_err(fail)?;
    let double = instance
        .get_func("double")
        .ok_or("no `double` export")?
        .typed::<(u32,), u32>()
        .map_err(fail)?;
    let result = double.call(&mut store, (21,)).await.map_err(fail)?;
    expect("double(21)", result, 42)?;
    Ok(format!("double(21) = {result}"))
}

/// Strings and a list lower into guest memory, a string lifts back
/// out, and a typed host function receives what the guest sends.
async fn host_function(engine: &Engine) -> Result<String, String> {
    let component = Component::new(engine, GREETER).await.map_err(fail)?;
    let mut linker: Linker<HostState> = Linker::new(engine);
    let host: InterfaceIdentifier = "wcmp:smoke/host@0.1.0".parse().map_err(fail)?;
    linker
        .instance(&host)
        .func_wrap(
            "tally",
            |mut state: HostCall<'_, HostState>, (n,): (u32,)| -> wcmp::Result<()> {
                state.data_mut().tallies.push(n);
                Ok(())
            },
        )
        .map_err(fail)?;
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .map_err(fail)?;

    let len = instance
        .get_func("len")
        .ok_or("no `len` export")?
        .typed::<(String,), i32>()
        .map_err(fail)?;
    let length = len
        .call(&mut store, ("héllo".to_owned(),))
        .await
        .map_err(fail)?;
    expect("len(\"héllo\") in UTF-8 bytes", length, 6)?;

    let echo = instance
        .get_func("echo")
        .ok_or("no `echo` export")?
        .typed::<(String,), String>()
        .map_err(fail)?;
    let echoed = echo
        .call(&mut store, ("round trip".to_owned(),))
        .await
        .map_err(fail)?;
    expect("echo", echoed.as_str(), "round trip")?;

    let sum = instance
        .get_func("sum")
        .ok_or("no `sum` export")?
        .typed::<(Vec<u32>,), u32>()
        .map_err(fail)?;
    let total = sum
        .call(&mut store, (vec![1, 2, 3, 4, 5],))
        .await
        .map_err(fail)?;
    expect("sum([1..5])", total, 15)?;

    let notify = instance
        .get_func("notify")
        .ok_or("no `notify` export")?
        .typed::<(u32,), ()>()
        .map_err(fail)?;
    notify.call(&mut store, (21,)).await.map_err(fail)?;
    expect("tallies", store.data().tallies.as_slice(), &[42])?;

    Ok(format!(
        "len(\"héllo\") = {length}, echo(\"round trip\") = {echoed:?}, sum([1..5]) = {total}; \
         the guest called the host's tally({})",
        store.data().tallies[0]
    ))
}

/// A host resource's destructor runs exactly once per handle the guest
/// drops, in drop order.
async fn host_resource(engine: &Engine) -> Result<String, String> {
    let component = Component::new(engine, DROPPER).await.map_err(fail)?;
    let mut linker: Linker<HostState> = Linker::new(engine);
    let resources: InterfaceIdentifier = "wcmp:smoke/resources@0.1.0".parse().map_err(fail)?;
    let thing = linker
        .instance(&resources)
        .resource(
            "thing",
            |state: &mut HostState, rep: u32| -> wcmp::Result<()> {
                state.dropped.push(rep);
                Ok(())
            },
        )
        .map_err(fail)?;
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .map_err(fail)?;
    let first = store.resource_new(thing, 11).map_err(fail)?;
    let second = store.resource_new(thing, 22).map_err(fail)?;
    let drop2 = instance.get_func("drop2").ok_or("no `drop2` export")?;
    drop2
        .call(&mut store, &[Val::Own(first), Val::Own(second)])
        .await
        .map_err(fail)?;
    expect(
        "destructor order",
        store.data().dropped.as_slice(),
        &[11, 22],
    )?;
    Ok(format!(
        "your destructor ran for {:?}, in drop order",
        store.data().dropped
    ))
}

/// The host releases a handle it never handed to the guest, watches
/// the destructor run, then drops the instance and the store.
async fn disposal(engine: &Engine) -> Result<String, String> {
    let component = Component::new(engine, DROPPER).await.map_err(fail)?;
    let mut linker: Linker<HostState> = Linker::new(engine);
    let resources: InterfaceIdentifier = "wcmp:smoke/resources@0.1.0".parse().map_err(fail)?;
    let thing = linker
        .instance(&resources)
        .resource(
            "thing",
            |state: &mut HostState, rep: u32| -> wcmp::Result<()> {
                state.dropped.push(rep);
                Ok(())
            },
        )
        .map_err(fail)?;
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .map_err(fail)?;
    let kept = store.resource_new(thing, 33).map_err(fail)?;
    let leaked = store.resource_new(thing, 44).map_err(fail)?;
    store.resource_drop(kept).map_err(fail)?;
    expect(
        "destructor after release",
        store.data().dropped.as_slice(),
        &[33],
    )?;
    let again = store.resource_drop(kept);
    expect("a second release is refused", again.is_err(), true)?;
    // The instance can go before the store; the store keeps the
    // tables and the destructor.
    drop(instance);
    let _ = leaked;
    let dropped = store.data().dropped.clone();
    drop(store);
    expect(
        "a dropped store runs no destructor for the leaked handle",
        dropped.as_slice(),
        &[33],
    )?;
    Ok(
        "release ran the destructor once; a second release was refused; dropping the store \
         ran no destructors"
            .to_owned(),
    )
}

/// A `wac` composition of two real guests runs through the adapter
/// the translator emits between them.
async fn composition(engine: &Engine) -> Result<String, String> {
    let component = Component::new(engine, COMPOSITION).await.map_err(fail)?;
    let linker: Linker<HostState> = Linker::new(engine);
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .map_err(fail)?;
    let run = instance
        .get_func("run")
        .ok_or("no `run` export")?
        .typed::<(u32,), u32>()
        .map_err(fail)?;
    let result = run.call(&mut store, (20,)).await.map_err(fail)?;
    expect("run(20) = double(20) + 1", result, 41)?;
    Ok(format!(
        "run(20) calls double(20) in the other component and returns {result}"
    ))
}

/// Two components built by a real toolchain move a `map<string, u32>`
/// and fixed-length lists in both directions, typed and untyped. The
/// map's keys are sent as `Val::Map` where their order matters, so
/// the evidence is the same on every target.
async fn maps_and_fixed_lists(engine: &Engine) -> Result<String, String> {
    let linker: Linker<HostState> = Linker::new(engine);
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;

    let maps = Component::new(engine, MAPS).await.map_err(fail)?;
    let maps = linker.instantiate(&mut store, &maps).await.map_err(fail)?;
    let sum = maps
        .get_func("sum")
        .ok_or("no `sum` export")?
        .typed::<(HashMap<String, u32>,), u32>()
        .map_err(fail)?;
    let map: HashMap<String, u32> = [("a", 1), ("b", 2), ("c", 39)]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect();
    let total = sum.call(&mut store, (map,)).await.map_err(fail)?;
    expect("sum({a: 1, b: 2, c: 39})", total, 42)?;
    let entries = Val::Map(Box::new([
        (Val::String("x".to_owned()), Val::U32(7)),
        (Val::String("y".to_owned()), Val::U32(8)),
    ]));
    let keys = maps
        .get_func("keys")
        .ok_or("no `keys` export")?
        .call(&mut store, std::slice::from_ref(&entries))
        .await
        .map_err(fail)?;
    expect(
        "keys({x: 7, y: 8})",
        keys.as_ref(),
        &[Val::List(Box::new([
            Val::String("x".to_owned()),
            Val::String("y".to_owned()),
        ]))],
    )?;
    let back = maps
        .get_func("identity")
        .ok_or("no `identity` export")?
        .call(&mut store, std::slice::from_ref(&entries))
        .await
        .map_err(fail)?;
    expect("identity({x: 7, y: 8})", back.as_ref(), &[entries])?;

    let lists = Component::new(engine, FIXED_LISTS).await.map_err(fail)?;
    let lists = linker.instantiate(&mut store, &lists).await.map_err(fail)?;
    let sum4 = lists
        .get_func("sum")
        .ok_or("no `sum` export")?
        .typed::<([u32; 4],), u32>()
        .map_err(fail)?;
    let total4 = sum4
        .call(&mut store, ([1, 2, 3, 36],))
        .await
        .map_err(fail)?;
    expect("sum([1, 2, 3, 36])", total4, 42)?;
    let double = lists
        .get_func("double")
        .ok_or("no `double` export")?
        .typed::<([u8; 16],), [u8; 16]>()
        .map_err(fail)?;
    let input: [u8; 16] = core::array::from_fn(|i| i as u8);
    let doubled = double.call(&mut store, (input,)).await.map_err(fail)?;
    expect(
        "double(0..16)",
        doubled,
        core::array::from_fn(|i| 2 * i as u8),
    )?;
    let bytes = Val::FixedLengthList(input.iter().map(|b| Val::U8(*b)).collect());
    let same = lists
        .get_func("identity")
        .ok_or("no `identity` export")?
        .call(&mut store, std::slice::from_ref(&bytes))
        .await
        .map_err(fail)?;
    expect("identity(0..16)", same.as_ref(), &[bytes])?;

    Ok(format!(
        "sum({{a: 1, b: 2, c: 39}}) = {total}, keys({{x: 7, y: 8}}) = [x, y]; \
         sum([1, 2, 3, 36]) = {total4}, double([0..16]) ends in {}",
        doubled[15]
    ))
}

/// A component exports a core module; the host reads its shape,
/// instantiates it itself, and registers it for a second component
/// that instantiates it in turn.
async fn core_modules(engine: &Engine) -> Result<String, String> {
    let provider = Component::new(engine, MODULE_PROVIDER)
        .await
        .map_err(fail)?;
    let linker: Linker<HostState> = Linker::new(engine);
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    let instance = linker
        .instantiate(&mut store, &provider)
        .await
        .map_err(fail)?;
    let module = instance.get_module("m").ok_or("no `m` module export")?;
    let shape: Vec<String> = module
        .exports()
        .iter()
        .map(|export| {
            let kind = match export.ty {
                CoreExternType::Func { .. } => "func",
                CoreExternType::Global { .. } => "global",
                CoreExternType::Memory { .. } => "memory",
                CoreExternType::Table { .. } => "table",
                CoreExternType::Tag { .. } => "tag",
                _ => "other",
            };
            format!("{} ({kind})", export.name)
        })
        .collect();
    expect("module imports", module.imports().len(), 0)?;
    expect(
        "module exports",
        shape.as_slice(),
        &["g (global)".to_owned(), "f (func)".to_owned()],
    )?;
    let core = module.instantiate(&mut store, &[]).await.map_err(fail)?;
    let g = core.get_export(&store, "g").ok_or("no `g` core export")?;
    expect(
        "the host reads the global's type",
        matches!(g.ty(&store), CoreExternType::Global { .. }),
        true,
    )?;

    let consumer = Component::new(engine, MODULE_CONSUMER)
        .await
        .map_err(fail)?;
    let mut linker: Linker<HostState> = Linker::new(engine);
    linker.root().module("m", &module).map_err(fail)?;
    let instance = linker
        .instantiate(&mut store, &consumer)
        .await
        .map_err(fail)?;
    let sum = instance
        .get_func("sum")
        .ok_or("no `sum` export")?
        .typed::<(), u32>()
        .map_err(fail)?;
    let total = sum.call(&mut store, ()).await.map_err(fail)?;
    expect("f() + g", total, 201)?;
    Ok(format!(
        "module exports {}; a second component used it: f() + g = {total}",
        shape.join(", ")
    ))
}

/// Functions inside a plain-named instance export and inside a nested
/// one are reached by name, and a handle reports its signature.
async fn navigation(engine: &Engine) -> Result<String, String> {
    let component = Component::new(engine, NESTED_EXPORTS).await.map_err(fail)?;
    let linker: Linker<HostState> = Linker::new(engine);
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .map_err(fail)?;
    expect("no root-level `f`", instance.get_func("f").is_none(), true)?;
    let a = instance
        .exports()
        .instance("a")
        .ok_or("no `a` instance export")?;
    let f = a.func("f").ok_or("no `a.f` export")?;
    let forty_two = f.call(&mut store, &[]).await.map_err(fail)?;
    expect("a.f()", forty_two.as_ref(), &[Val::U32(42)])?;
    let g = a
        .instance("b")
        .ok_or("no `a.b` instance export")?
        .func("g")
        .ok_or("no `a.b.g` export")?;
    let signature = g.ty().clone();
    let parameters: Vec<String> = signature
        .parameters
        .iter()
        .map(|parameter| format!("{}: {}", parameter.name, type_name(&parameter.ty)))
        .collect();
    let g = g.typed::<(u32,), u32>().map_err(fail)?;
    let result = g.call(&mut store, (41,)).await.map_err(fail)?;
    expect("a.b.g(41)", result, 42)?;
    Ok(format!(
        "a.f() = 42; a.b.g({}) -> {}; a.b.g(41) = {result}",
        parameters.join(", "),
        signature
            .result
            .as_ref()
            .map_or("nothing".to_owned(), type_name)
    ))
}

/// A value type as WIT spells it, for the primitives the smoke test
/// shows; any other shape falls back to the polyfill's debug form.
fn type_name(ty: &ValueType) -> String {
    match ty {
        ValueType::Primitive(primitive) => format!("{primitive:?}").to_lowercase(),
        other => format!("{other:?}"),
    }
}

/// A string goes from the host into a 32-bit component, through an
/// adapter into a 64-bit component that copies it in an `i64`
/// memory, and back.
async fn memory64(engine: &Engine) -> Result<String, String> {
    let component = Component::new(engine, MEMORY64_COMPOSITION)
        .await
        .map_err(fail)?;
    let linker: Linker<HostState> = Linker::new(engine);
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .map_err(fail)?;
    let roundtrip = instance
        .get_func("roundtrip")
        .ok_or("no `roundtrip` export")?
        .typed::<(String,), String>()
        .map_err(fail)?;
    let text = "héllo from a 64-bit memory";
    let back = roundtrip
        .call(&mut store, (text.to_owned(),))
        .await
        .map_err(fail)?;
    expect("roundtrip", back.as_str(), text)?;
    Ok(format!("{text:?} came back unchanged"))
}

/// The default engine validates with Wasmtime's feature gates, and a
/// host opts into a gated feature through the engine configuration.
async fn engine_configuration() -> Result<String, String> {
    let strict = configured_engine(&EngineConfig::new()).map_err(fail)?;
    let rejection = match Component::new(&strict, IMPLEMENTS).await {
        Ok(_) => return Err("the default engine accepted `implements`".to_owned()),
        Err(Error::InvalidComponentBinary { message, .. }) if message.contains("cm-implements") => {
            "the `cm-implements` feature is not active"
        }
        Err(other) => return Err(format!("unexpected rejection: {other}")),
    };
    let mut config = EngineConfig::new();
    config.wasm_component_model_implements(true);
    let permissive = configured_engine(&config).map_err(fail)?;
    let component = Component::new(&permissive, IMPLEMENTS)
        .await
        .map_err(fail)?;
    expect(
        "the annotated import is described",
        component.imports.len(),
        1,
    )?;
    Ok(format!(
        "default engine: \"{rejection}\"; configured engine: accepted"
    ))
}

/// A suspend provider as the report names it.
fn provider_name(kind: SuspendProviderKind) -> &'static str {
    match kind {
        SuspendProviderKind::StackSwitching => "stack switching",
        SuspendProviderKind::HostSuspension => "JSPI (JavaScript Promise Integration)",
        SuspendProviderKind::None => "none",
        _ => "unknown",
    }
}

/// The engine selected the provider this target offers when it was
/// constructed, and an engine configured with suspending off answers
/// that it has none, whatever the target offers.
async fn suspend_provider(engine: &Engine) -> Result<String, String> {
    let selected = engine.suspend_provider();
    expect(
        "the provider this target offers",
        selected,
        offered::offered(),
    )?;
    let mut config = EngineConfig::new();
    config.suspend_provider(false);
    let off = configured_engine(&config).map_err(fail)?;
    expect(
        "the provider of an engine with suspending off",
        off.suspend_provider(),
        SuspendProviderKind::None,
    )?;
    Ok(format!(
        "this engine: {}; with suspending off: {}",
        provider_name(selected),
        provider_name(off.suspend_provider())
    ))
}

/// How long the `run_concurrent` closure waits outside the store, in
/// milliseconds. The step then waits twice as long with the entry
/// unpolled, so the closure's timer has fired before the entry is
/// driven again.
const WAIT_MILLIS: u32 = 20;

/// A waker that counts the wakes it receives. A wake that lands here
/// is how a step says a wake arrived without comparing waker
/// identities, which two clones of one waker do not always agree on.
#[derive(Default)]
struct Wakes(AtomicUsize);

impl Wakes {
    fn count(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }
}

impl std::task::Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

/// Poll `future` once, as an executor would.
fn poll_once<F: Future>(future: &mut Pin<Box<F>>, waker: &Waker) -> Poll<F::Output> {
    let mut context = Context::from_waker(waker);
    future.as_mut().poll(&mut context)
}

/// The store's `run_concurrent` entry waits on something the store
/// cannot resolve, and comes back with the closure's value.
///
/// The closure reads the host data through the accessor, awaits a
/// timer outside the store, reads the host data again and returns.
/// While it waits the store is idle: every other driver would fail
/// with the deadlock cause there, and this one returns pending
/// instead, because the waker it was polled with is what brings it
/// back. The step polls the entry by hand once to see that, and then
/// waits twice as long with nothing polling it, so the timer's wake
/// lands on the waker the hand poll gave it.
async fn run_concurrent_outside(engine: &Engine) -> Result<String, String> {
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    store.data_mut().tallies.push(1);

    // What the host does while the wait is on: in the browser, a
    // `setTimeout` of zero asked for now, which only a page still
    // delivering callbacks will run.
    let outside = Outside::watching();

    let mut entry = Box::pin(store.run_concurrent(async |accessor| {
        let before = accessor.with(|store| store.data().tallies.len())?;
        // Nothing the store owns can resolve this.
        Outside::pause(WAIT_MILLIS).await;
        let after = accessor.with(|store| {
            store.data_mut().tallies.push(2);
            store.data().tallies.len()
        })?;
        Ok::<String, Error>(format!("{before} then {after}"))
    }));

    let wakes = Arc::new(Wakes::default());
    let waker = Waker::from(wakes.clone());
    match poll_once(&mut entry, &waker) {
        Poll::Pending => (),
        Poll::Ready(Ok(_)) => return Err("the entry completed without waiting".to_owned()),
        Poll::Ready(Err(error)) => {
            return Err(format!("the entry failed instead of waiting: {error}"));
        }
    }

    Outside::pause(2 * WAIT_MILLIS).await;
    let woken = wakes.count();
    expect("the wait woke the entry's waker", woken > 0, true)?;
    let watched = outside.observed()?;

    let value = entry.await.map_err(fail)?.map_err(fail)?;
    expect("the closure's value", value.as_str(), "1 then 2")?;
    expect(
        "what the closure wrote stayed in the store",
        store.data().tallies.as_slice(),
        &[1, 2],
    )?;

    Ok(format!(
        "pending, not deadlocked, during a {WAIT_MILLIS} ms wait; {watched}; returned \
         {value:?} with host state {:?}",
        store.data().tallies
    ))
}

/// The `rich` fixture: a world of records, variants, enums, flags,
/// options, results, nested lists, and strings, and a resource on
/// each side of an import. Every value crosses three component
/// boundaries — driver to guest, guest to support, and back — so one
/// call exercises wit-bindgen's lift and lower code and the
/// allocator's `cabi_realloc` six times.
async fn rich_world(engine: &Engine) -> Result<String, String> {
    let linker: Linker<HostState> = Linker::new(engine);
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    let component = Component::new(engine, RICH).await.map_err(fail)?;
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .map_err(fail)?;

    let call = |name: &'static str| {
        instance
            .get_func(name)
            .ok_or_else(|| format!("no `{name}` export"))
    };

    // An enum, a flags set, an option, and a string back.
    let described = call("describe")?
        .call(
            &mut store,
            &[
                Val::Enum("green".to_owned()),
                Val::Flags(Box::new(["bold".to_owned(), "italic".to_owned()])),
                Val::Option(Some(Box::new(Val::String("hello".to_owned())))),
            ],
        )
        .await
        .map_err(fail)?;
    expect(
        "describe(green, {bold, italic}, some(\"hello\"))",
        described.as_ref(),
        &[Val::String("green[bold,italic] hello".to_owned())],
    )?;

    // A nested list in, a flat one out.
    let folded = call("fold")?
        .call(
            &mut store,
            &[Val::List(Box::new([
                Val::List(Box::new([Val::U32(1), Val::U32(2), Val::U32(3)])),
                Val::List(Box::new([Val::U32(10)])),
            ]))],
        )
        .await
        .map_err(fail)?;
    expect(
        "fold([[1, 2, 3], [10]])",
        folded.as_ref(),
        &[Val::List(Box::new([
            Val::U32(6),
            Val::U32(10),
            Val::U32(16),
        ]))],
    )?;

    // Both arms of a `result`, one of them carrying a variant.
    let point = |x: i32, y: i32| {
        Val::Record(Box::new([
            ValField {
                name: "x".to_owned(),
                value: Val::S32(x),
            },
            ValField {
                name: "y".to_owned(),
                value: Val::S32(y),
            },
        ]))
    };
    let measured = call("measure-all")?
        .call(
            &mut store,
            &[Val::List(Box::new([
                Val::Variant {
                    discriminant: "dot".to_owned(),
                    payload: Some(Box::new(point(3, -4))),
                },
                Val::Variant {
                    discriminant: "empty".to_owned(),
                    payload: None,
                },
            ]))],
        )
        .await
        .map_err(fail)?;
    expect(
        "measure-all([dot(3, -4), empty])",
        measured.as_ref(),
        &[Val::List(Box::new([
            Val::Result(Ok(Some(Box::new(point(3, -4))))),
            Val::Result(Err(Some(Box::new(Val::Variant {
                discriminant: "blank".to_owned(),
                payload: None,
            })))),
        ]))],
    )?;

    // The support component's resource, constructed and dropped by
    // the guest, and the guest's own resource, constructed and
    // dropped by the driver. Each destructor runs in the component
    // that defines it, and each count is read back through it.
    let steps =
        |values: &[u32]| Val::List(values.iter().copied().map(Val::U32).collect::<Box<[_]>>());
    let tallied = call("exercise-tallies")?
        .call(&mut store, &[steps(&[1, 2, 3])])
        .await
        .map_err(fail)?;
    expect(
        "exercise-tallies([1, 2, 3])",
        tallied.as_ref(),
        &[Val::U32(106)],
    )?;
    let tally_drops = call("tally-drops")?
        .call(&mut store, &[])
        .await
        .map_err(fail)?;
    expect("tally-drops", tally_drops.as_ref(), &[Val::U32(2)])?;

    let counted = call("exercise-counters")?
        .call(&mut store, &[steps(&[5, 7, 9])])
        .await
        .map_err(fail)?;
    expect(
        "exercise-counters([5, 7, 9])",
        counted.as_ref(),
        &[Val::U32(28)],
    )?;
    let counter_drops = call("counter-drops")?
        .call(&mut store, &[])
        .await
        .map_err(fail)?;
    expect("counter-drops", counter_drops.as_ref(), &[Val::U32(2)])?;

    Ok(
        "every type crossed three components and back; both resource destructors ran twice"
            .to_owned(),
    )
}

/// The sentence the `words` story hands the guest.
const SENTENCE: &str = "streams carry values between a host and its guests";

/// How many numbers the host streams to `checksum`, and how many the
/// composed counter streams to its reader.
const NUMBERS: u32 = 1000;

/// How many numbers the host's producer delivers per poll.
const BATCH: u32 = 100;

/// How long a stream story waits for the stream or future it reads to
/// finish before it gives up, in milliseconds. A story that meets a
/// stuck stream fails with what it saw rather than hang the report.
const STREAM_DEADLINE_MILLIS: u32 = 10_000;

/// The waker of the `run_concurrent` entry a stream story waits in,
/// which the story's consumer wakes when it takes something.
type Signal = Arc<Mutex<Option<Waker>>>;

/// Wake the entry waiting on `signal`, if one is.
fn wake(signal: &Signal) {
    if let Some(waker) = signal.lock().ok().and_then(|mut slot| slot.take()) {
        waker.wake();
    }
}

/// What a consumer took, shared with the story.
struct Taken<T> {
    items: Vec<T>,
    /// How many polls handed the consumer items.
    copies: usize,
    /// Whether the stream ended, which drops the consumer.
    ended: bool,
}

/// A stream consumer that takes every item it is offered, in order,
/// and records when the pipe drops it because the stream ended.
struct Collects<T> {
    taken: Arc<Mutex<Taken<T>>>,
    signal: Signal,
}

impl<T> Collects<T> {
    /// A consumer, and what it will have taken.
    fn new(signal: &Signal) -> (Self, Arc<Mutex<Taken<T>>>) {
        let taken = Arc::new(Mutex::new(Taken {
            items: Vec::new(),
            copies: 0,
            ended: false,
        }));
        let consumer = Self {
            taken: taken.clone(),
            signal: signal.clone(),
        };
        (consumer, taken)
    }
}

impl<T: ComponentValue + Send + 'static> StreamConsumer<HostState> for Collects<T> {
    type Item = T;

    fn poll_consume(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        store: &mut StoreContext<'_, HostState>,
        mut source: Source<'_, T>,
        _finish: bool,
    ) -> Poll<Result<StreamResult, Error>> {
        let count = source.remaining();
        if count > 0 {
            let mut items = Vec::with_capacity(count);
            source.read(store, &mut items, count)?;
            let mut taken = self.taken.lock().expect("what the consumer took");
            taken.items.extend(items);
            taken.copies += 1;
        }
        wake(&self.signal);
        Poll::Ready(Ok(StreamResult::Completed))
    }
}

impl<T> Drop for Collects<T> {
    fn drop(&mut self) {
        if let Ok(mut taken) = self.taken.lock() {
            taken.ended = true;
        }
        wake(&self.signal);
    }
}

/// A future consumer that keeps the one value it is given.
struct Keeps {
    value: Arc<Mutex<Option<u64>>>,
    signal: Signal,
}

impl FutureConsumer<HostState> for Keeps {
    type Item = u64;

    fn poll_consume(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        store: &mut StoreContext<'_, HostState>,
        mut source: Source<'_, u64>,
        _finish: bool,
    ) -> Poll<Result<(), Error>> {
        let mut value = Vec::with_capacity(1);
        source.read(store, &mut value, 1)?;
        *self.value.lock().expect("the future's value") = value.pop();
        wake(&self.signal);
        Poll::Ready(Ok(()))
    }
}

/// A stream producer that writes the numbers from 1 to `last`,
/// [`BATCH`] per poll, and answers pending once before each batch, so
/// the numbers reach the guest over several turns of the store.
struct Batches {
    next: u32,
    last: u32,
    parked: bool,
    /// How many batches it delivered.
    delivered: Arc<AtomicUsize>,
}

impl StreamProducer<HostState> for Batches {
    type Item = u32;

    fn poll_produce(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        _store: &mut StoreContext<'_, HostState>,
        mut destination: Destination<'_, u32>,
        _finish: bool,
    ) -> Poll<Result<StreamResult, Error>> {
        let this = self.get_mut();
        // A read of nothing asks only whether the stream is ready.
        if destination.remaining() == Some(0) {
            return Poll::Ready(Ok(StreamResult::Completed));
        }
        if !this.parked {
            this.parked = true;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        this.parked = false;
        if this.next > this.last {
            return Poll::Ready(Ok(StreamResult::Dropped));
        }
        let end = this.last.min(this.next + BATCH - 1);
        destination.set_buffer((this.next..=end).collect());
        this.next = end + 1;
        this.delivered.fetch_add(1, Ordering::Relaxed);
        Poll::Ready(Ok(if this.next > this.last {
            StreamResult::Dropped
        } else {
            StreamResult::Completed
        }))
    }
}

/// Run turns of `store` until `done` holds, parking on `signal`
/// between checks, or fail once [`STREAM_DEADLINE_MILLIS`] have
/// passed. The deadline is a timer outside the store, which the
/// `run_concurrent` entry is free to wait on.
async fn run_until(
    store: &mut Store<HostState>,
    signal: &Signal,
    what: &str,
    mut done: impl FnMut() -> bool,
) -> Result<(), String> {
    let finished = store
        .run_concurrent(async |_accessor: &Accessor<HostState>| {
            let mut deadline = Box::pin(Outside::pause(STREAM_DEADLINE_MILLIS));
            core::future::poll_fn(|cx| {
                *signal.lock().expect("the signal") = Some(cx.waker().clone());
                if done() {
                    Poll::Ready(true)
                } else if deadline.as_mut().poll(cx).is_ready() {
                    Poll::Ready(false)
                } else {
                    Poll::Pending
                }
            })
            .await
        })
        .await
        .map_err(fail)?;
    if finished {
        Ok(())
    } else {
        Err(format!(
            "{what} did not finish within {STREAM_DEADLINE_MILLIS} ms"
        ))
    }
}

/// Instantiate `bytes` into a store of its own.
async fn instantiate_alone(
    engine: &Engine,
    bytes: &[u8],
) -> Result<(Store<HostState>, Instance), String> {
    let component = Component::new(engine, bytes).await.map_err(fail)?;
    let linker: Linker<HostState> = Linker::new(engine);
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .map_err(fail)?;
    Ok((store, instance))
}

/// The guest's `words` answers with the readable end of a
/// `stream<string>` and writes into it from a task it spawned, after
/// the call has returned. The host pipes the end to a consumer and
/// runs turns until the guest ends the stream, which drops the
/// consumer. Each word is a write of its own, so each reaches the
/// consumer as a copy of its own, lifted out of the guest's memory.
async fn read_a_guest_stream(engine: &Engine) -> Result<String, String> {
    let (mut store, instance) = instantiate_alone(engine, STREAMS).await?;
    let words = instance
        .get_func("words")
        .ok_or("no `words` export")?
        .typed::<(String,), StreamReader<String>>()
        .map_err(fail)?;
    let reader = store
        .run_concurrent(async |accessor: &Accessor<HostState>| {
            words
                .call_concurrent(accessor, (SENTENCE.to_owned(),))
                .await
        })
        .await
        .map_err(fail)?
        .map_err(fail)?;

    let signal = Signal::default();
    let (consumer, taken) = Collects::<String>::new(&signal);
    reader
        .pipe(&mut store.as_context_mut(), consumer)
        .map_err(fail)?;
    run_until(&mut store, &signal, "the stream of words", || {
        taken.lock().expect("the words").ended
    })
    .await?;

    let taken = taken.lock().expect("the words");
    let wanted: Vec<&str> = SENTENCE.split_whitespace().collect();
    let got: Vec<&str> = taken.items.iter().map(String::as_str).collect();
    expect("the words, in order", got, wanted.clone())?;
    expect("one copy per word", taken.copies, wanted.len())?;
    Ok(format!(
        "all {} words arrived in order, then the stream ended",
        taken.items.len()
    ))
}

/// The host creates a `stream<u32>` over its own producer and hands
/// it to the guest's `checksum`, which answers at once with a
/// `future<u64>` and reads the stream from a task it spawned. The
/// producer delivers a batch per poll and is pending before each, so
/// the numbers cross over several turns. The host pipes the future
/// to a consumer and runs turns until the guest writes the checksum,
/// the sum of each number times its one-based position, which a
/// reordered, lost, or repeated number would change.
async fn stream_in_future_out(engine: &Engine) -> Result<String, String> {
    let (mut store, instance) = instantiate_alone(engine, STREAMS).await?;
    let checksum = instance
        .get_func("checksum")
        .ok_or("no `checksum` export")?
        .typed::<(StreamReader<u32>,), FutureReader<u64>>()
        .map_err(fail)?;
    let delivered = Arc::new(AtomicUsize::new(0));
    let numbers = StreamReader::new(
        &mut store.as_context_mut(),
        Batches {
            next: 1,
            last: NUMBERS,
            parked: false,
            delivered: delivered.clone(),
        },
    )
    .map_err(fail)?;
    let future = store
        .run_concurrent(async |accessor: &Accessor<HostState>| {
            checksum.call_concurrent(accessor, (numbers,)).await
        })
        .await
        .map_err(fail)?
        .map_err(fail)?;

    let signal = Signal::default();
    let value = Arc::new(Mutex::new(None));
    future
        .pipe(
            &mut store.as_context_mut(),
            Keeps {
                value: value.clone(),
                signal: signal.clone(),
            },
        )
        .map_err(fail)?;
    run_until(&mut store, &signal, "the future of the checksum", || {
        value.lock().expect("the checksum").is_some()
    })
    .await?;

    let total = value.lock().expect("the checksum").take();
    // The producer writes n at position n, so the checksum is the sum
    // of the squares of 1..=NUMBERS.
    let n = u64::from(NUMBERS);
    let wanted = n * (n + 1) * (2 * n + 1) / 6;
    expect("the future's value", total, Some(wanted))?;
    let batches = delivered.load(Ordering::Relaxed);
    expect(
        "batches the producer delivered",
        batches,
        NUMBERS.div_ceil(BATCH) as usize,
    )?;
    Ok(format!(
        "sent 1..={NUMBERS} in {batches} batches; the future resolved to {wanted}, the \
         expected checksum"
    ))
}

/// The composed component's `total` calls `count-up` in the other
/// component, which answers with a `stream<u32>` and writes the
/// numbers into it after returning. The stream's readable end crosses
/// the adapter between the two, and each copy moves the numbers'
/// bytes from the writer's memory straight into the reader's, which
/// is how a stream of a number type crosses between guests. The
/// reader traps on a number out of order or missing, so the sum comes
/// back only when every number arrived once.
async fn stream_between_components(engine: &Engine) -> Result<String, String> {
    let (mut store, instance) = instantiate_alone(engine, STREAM_COMPOSITION).await?;
    let total = instance
        .get_func("total")
        .ok_or("no `total` export")?
        .typed::<(u32,), u64>()
        .map_err(fail)?;
    let sum = total.call(&mut store, (NUMBERS,)).await.map_err(fail)?;
    let wanted = u64::from(NUMBERS) * (u64::from(NUMBERS) + 1) / 2;
    expect("total(1000)", sum, wanted)?;
    let empty = total.call(&mut store, (0,)).await.map_err(fail)?;
    expect("total(0)", empty, 0)?;
    Ok(format!(
        "total({NUMBERS}) = {sum}; an empty stream sums to {empty}"
    ))
}

/// The keys the `sync-wait` story hands `total`, which asks the host
/// for each in turn.
const KEYS: [u32; 3] = [1, 2, 39];

/// An engine that accepts stackful exports and the thread built-ins,
/// whose feature gates are off by default, with suspending allowed or
/// turned off.
fn suspending_engine(suspending: bool) -> Result<Engine, String> {
    let mut config = EngineConfig::new();
    config.wasm_component_model_async_stackful(true);
    config.wasm_component_model_threading(true);
    config.suspend_provider(suspending);
    configured_engine(&config).map_err(fail)
}

/// A linker whose `tally` records each value the guest hands it.
fn tallying(engine: &Engine) -> Result<Linker<HostState>, String> {
    let mut linker: Linker<HostState> = Linker::new(engine);
    let host: InterfaceIdentifier = "wcmp:smoke/host@0.1.0".parse().map_err(fail)?;
    linker
        .instance(&host)
        .func_wrap(
            "tally",
            |mut state: HostCall<'_, HostState>, (n,): (u32,)| -> wcmp::Result<()> {
                state.data_mut().tallies.push(n);
                Ok(())
            },
        )
        .map_err(fail)?;
    Ok(linker)
}

/// Whether `error`, or an error it carries, is the stack-switch
/// cause: the guest waited where only setting its stack aside would
/// have let it go on, and the engine has no way to do that.
///
/// A wait that fails inside a built-in or a lowered import the guest
/// called traps the guest, and the trap carries the cause as its
/// message rather than as a value: the error the call returns is the
/// trap, and the cause is the text inside it. So a link of the chain
/// matches either as the cause itself or by carrying the cause's own
/// message, which is how the conformance corpora match it too.
fn needs_stack_switch(error: &Error) -> bool {
    let message = SchedulerCause::StackSwitchNeeded.to_string();
    let mut link: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(current) = link {
        if matches!(
            current.downcast_ref::<SchedulerCause>(),
            Some(SchedulerCause::StackSwitchNeeded)
        ) || current.to_string().contains(&message)
        {
            return true;
        }
        link = current.source();
    }
    false
}

/// Every message in an error's chain, on one line.
fn chain(error: &Error) -> String {
    let mut messages = Vec::new();
    let mut link: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(current) = link {
        messages.push(current.to_string());
        link = current.source();
    }
    messages
        .join(": ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// What a call that has to wait does on an engine with no suspend
/// provider: it fails with the stack-switch cause, rather than
/// returning or hanging. Returns the evidence when it did.
fn refused_for_a_stack_switch<T: std::fmt::Debug>(
    what: &str,
    outcome: Result<T, Error>,
) -> Result<String, String> {
    match outcome {
        Ok(value) => Err(format!(
            "{what} returned {value:?} on an engine with no way to set the guest aside"
        )),
        Err(error) if needs_stack_switch(&error) => Ok(format!(
            "{what} failed with \"{}\"",
            SchedulerCause::StackSwitchNeeded
        )),
        Err(error) => Err(format!(
            "{what} failed, but not with the stack-switch cause: {}",
            chain(&error)
        )),
    }
}

/// The evidence of a suspending story on an engine with no provider,
/// which is the outcome its goal documents for a browser without
/// JavaScript Promise Integration.
fn without_a_provider(refusal: String) -> String {
    format!("this engine cannot pause a guest, so {refusal}, as expected")
}

/// The `sync-wait` fixture's `total` asks the host's `host-echo-u32`
/// for each key through a synchronous lower, and the host answers
/// each only once a timer outside the store has fired.
///
/// Under a provider the guest's thread is set aside at each call: the
/// lower parks the host's future among the store's host tasks and
/// suspends the thread, and the driver returns pending. The step polls
/// the call by hand once to see that, then waits twice as long with
/// nothing polling it, so the timer's wake lands on the waker the hand
/// poll gave it. Each later key suspends the thread the same way, and
/// the call returns the sum of the host's answers. With no provider
/// the first call fails with the stack-switch cause, because only a
/// suspension can wait for something outside the store.
async fn wait_for_the_host(engine: &Engine) -> Result<String, String> {
    let component = Component::new(engine, SYNC_WAIT).await.map_err(fail)?;
    let mut linker: Linker<HostState> = Linker::new(engine);
    linker
        .root()
        .func_wrap_concurrent(
            "host-echo-u32",
            |accessor: &Accessor<HostState>, (key,): (u32,)| {
                let asked = accessor.with(|store| store.data_mut().tallies.push(key));
                async move {
                    asked?;
                    // Nothing the store owns can resolve this.
                    Outside::pause(WAIT_MILLIS).await;
                    Ok(key)
                }
            },
        )
        .map_err(fail)?;
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .map_err(fail)?;
    let total = instance
        .get_func("total")
        .ok_or("no `total` export")?
        .typed::<(Vec<u32>,), u32>()
        .map_err(fail)?;

    if engine.suspend_provider() == SuspendProviderKind::None {
        let outcome = total.call(&mut store, (KEYS.to_vec(),)).await;
        return refused_for_a_stack_switch("`total`", outcome);
    }

    // What the host does while the guest waits: in the browser, a
    // `setTimeout` of zero asked for now, which only a page still
    // delivering callbacks will run.
    let outside = Outside::watching();
    let wakes = Arc::new(Wakes::default());
    let waker = Waker::from(wakes.clone());
    let (sum, woken) = {
        let mut call = Box::pin(total.call(&mut store, (KEYS.to_vec(),)));
        match poll_once(&mut call, &waker) {
            Poll::Pending => (),
            Poll::Ready(Ok(sum)) => {
                return Err(format!(
                    "`total` returned {sum} without waiting for the timer"
                ));
            }
            Poll::Ready(Err(error)) => {
                return Err(format!("`total` failed instead of waiting: {error}"));
            }
        }
        Outside::pause(2 * WAIT_MILLIS).await;
        let woken = wakes.count();
        (call.await.map_err(fail)?, woken)
    };
    expect("the timer woke the waiting call", woken > 0, true)?;
    let watched = outside.observed()?;
    expect(
        "the keys the host was asked for, in order",
        store.data().tallies.as_slice(),
        &KEYS,
    )?;
    expect("total(1, 2, 39)", sum, KEYS.iter().sum())?;
    Ok(format!(
        "the guest waited on the host for keys {:?}, {WAIT_MILLIS} ms each; {watched}; \
         `total` returned {sum}",
        store.data().tallies
    ))
}

/// The same story on an engine selected for this target, with a
/// provider where the target offers one.
async fn wait_for_the_host_here(engine: &Engine) -> Result<String, String> {
    let evidence = wait_for_the_host(engine).await?;
    Ok(if engine.suspend_provider() == SuspendProviderKind::None {
        without_a_provider(evidence)
    } else {
        evidence
    })
}

/// The stackful export `both` starts two calls to the host's `fetch`
/// and blocks in `waitable-set.wait` until each has answered. The host
/// answers each with twice its key, the first after three times as
/// long as the second, so the second answers first. The export tells
/// `tally` which call each wake was for, so the tallies read `[2, 1]`
/// only when the export woke once per answer, in the order the host
/// answered. With no provider the wait fails with the stack-switch
/// cause, because only a suspension can wait for the host's timers.
async fn block_and_resume_on(engine: &Engine) -> Result<String, String> {
    let component = Component::new(engine, BLOCKING_EXPORT)
        .await
        .map_err(fail)?;
    let mut linker = tallying(engine)?;
    let calls = Arc::new(AtomicUsize::new(0));
    linker
        .root()
        .func_wrap_concurrent(
            "fetch",
            move |_accessor: &Accessor<HostState>, (key,): (u32,)| {
                let first = calls.fetch_add(1, Ordering::Relaxed) == 0;
                async move {
                    Outside::pause(if first { 3 * WAIT_MILLIS } else { WAIT_MILLIS }).await;
                    Ok(key * 2)
                }
            },
        )
        .map_err(fail)?;
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .map_err(fail)?;
    let both = instance
        .get_func("both")
        .ok_or("no `both` export")?
        .typed::<(u32, u32), u32>()
        .map_err(fail)?;

    let outcome = both.call(&mut store, (20, 1)).await;
    if engine.suspend_provider() == SuspendProviderKind::None {
        return refused_for_a_stack_switch("`both`", outcome);
    }
    let sum = outcome.map_err(fail)?;
    expect(
        "the calls the export woke for, in order",
        store.data().tallies.as_slice(),
        &[2, 1],
    )?;
    expect("both(20, 1)", sum, 42)?;
    Ok(format!(
        "the host answered call 2 after {WAIT_MILLIS} ms and call 1 after {} ms; the export \
         woke in that order and returned {sum}",
        3 * WAIT_MILLIS
    ))
}

/// The blocking export on an engine that accepts it, with a provider
/// where the target offers one.
async fn block_and_resume() -> Result<String, String> {
    let engine = suspending_engine(true)?;
    let evidence = block_and_resume_on(&engine).await?;
    Ok(if engine.suspend_provider() == SuspendProviderKind::None {
        without_a_provider(evidence)
    } else {
        evidence
    })
}

/// The order in which the guest's threads and its main thread tell
/// the host what they did: each of threads 0, 1, and 2 starts and
/// parks, the main thread says all three are parked, thread 0 runs
/// first because the main thread switched straight to it, threads 2
/// and 1 run in the order the main thread woke them, and the main
/// thread goes on last.
const THREAD_ORDER: [u32; 8] = [10, 11, 12, 30, 20, 22, 21, 40];

/// The stackful export `run` of [`GUEST_THREADS`] starts three
/// threads, lets each run up to where it parks itself, wakes them in
/// an order of its own, and parks until the last one finishes. Every
/// step tells the host's `tally`, so the tallies are the order in
/// which the threads ran. With no provider the first thread's park
/// fails with the stack-switch cause: the thread runs on the one
/// stack above the main thread, and only the main thread below it can
/// wake it.
async fn park_and_wake(engine: &Engine) -> Result<String, String> {
    let component = Component::new(engine, GUEST_THREADS).await.map_err(fail)?;
    let linker = tallying(engine)?;
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .map_err(fail)?;
    let run = instance
        .get_func("run")
        .ok_or("no `run` export")?
        .typed::<(), u32>()
        .map_err(fail)?;

    let outcome = run.call(&mut store, ()).await;
    if engine.suspend_provider() == SuspendProviderKind::None {
        return refused_for_a_stack_switch("`run`", outcome);
    }
    let finished = outcome.map_err(fail)?;
    expect("threads that finished", finished, 3)?;
    expect(
        "the order the threads ran in",
        store.data().tallies.as_slice(),
        &THREAD_ORDER,
    )?;
    Ok(format!(
        "{finished} threads started and parked, woke in the order 0, 2, 1, and then woke the \
         main thread"
    ))
}

/// The guest threads on an engine that accepts them, with a provider
/// where the target offers one.
async fn guest_threads() -> Result<String, String> {
    let engine = suspending_engine(true)?;
    let evidence = park_and_wake(&engine).await?;
    Ok(if engine.suspend_provider() == SuspendProviderKind::None {
        without_a_provider(evidence)
    } else {
        evidence
    })
}

/// Each suspending story on an engine configured with suspending off:
/// every call that has to wait fails with the stack-switch cause, on
/// every target, rather than returning a wrong answer or hanging.
async fn suspending_off() -> Result<String, String> {
    let mut config = EngineConfig::new();
    config.suspend_provider(false);
    let plain = configured_engine(&config).map_err(fail)?;
    expect(
        "the provider of an engine with suspending off",
        plain.suspend_provider(),
        SuspendProviderKind::None,
    )?;
    wait_for_the_host(&plain).await?;
    let gated = suspending_engine(false)?;
    block_and_resume_on(&gated).await?;
    park_and_wake(&gated).await?;
    Ok(format!(
        "`total`, `both`, and `run` each failed with \"{}\"; none hung",
        SchedulerCause::StackSwitchNeeded
    ))
}

/// How long the host's `fetch` takes to answer, in milliseconds: far
/// past the handler's deadline, so only a cancellation ends the call
/// in time.
const SLOW_FETCH_MILLIS: u32 = 10_000;

/// The representation of the request the host hands the handler.
const REQUEST_REP: u32 = 7;

/// What became of the host's `fetch` call.
#[derive(Debug, Default)]
struct FetchWatch {
    /// The representation of the request the call was lent.
    lent: Option<u32>,
    /// Whether the call answered.
    answered: bool,
    /// Whether the store dropped the call's future.
    dropped: bool,
    /// Whether the drop could reach the store, which it can only
    /// inside a turn of the store.
    dropped_in_a_turn: bool,
}

/// Held by the future of a `fetch` call, so its drop tells the host
/// that the store let the future go, and whether that happened inside
/// a turn.
struct WatchDrop {
    accessor: Accessor<HostState>,
    watch: Arc<Mutex<FetchWatch>>,
}

impl Drop for WatchDrop {
    fn drop(&mut self) {
        let in_a_turn = self.accessor.with(|_store| ()).is_ok();
        if let Ok(mut watch) = self.watch.lock() {
            watch.dropped = true;
            watch.dropped_in_a_turn = in_a_turn;
        }
    }
}

/// The deadline story on `engine`, and again on an engine with
/// suspending turned off. Nothing in it needs a stack switch: the
/// handler is lifted with a callback, and the cancel it makes waits
/// only for the store's next turn to drop the host's future, which a
/// turn nested on the one stack does as well.
async fn cancel_a_slow_host_call(engine: &Engine) -> Result<String, String> {
    let evidence = cancel_a_slow_host_call_on(engine).await?;
    let mut config = EngineConfig::new();
    config.suspend_provider(false);
    let off = configured_engine(&config).map_err(fail)?;
    let again = cancel_a_slow_host_call_on(&off).await?;
    expect(
        "the story with suspending turned off",
        again.as_str(),
        evidence.as_str(),
    )?;
    Ok(format!("{evidence}; same with suspending off"))
}

/// The `deadline` fixture's `handle` races the host's `fetch` against
/// the host's `sleep`. `fetch` answers only after
/// [`SLOW_FETCH_MILLIS`], and the handler's deadline is
/// [`WAIT_MILLIS`], so the timer wins. The handler drops the pending
/// call, and wit-bindgen's runtime cancels it with `subtask.cancel`.
/// The store drops the host's future in its next turn, the call
/// resolves as cancelled before it returned, and the borrow of the
/// request the handler lent comes back. The handler then drops the
/// request, which traps while a borrow is out, so the host's
/// destructor running is the evidence the borrow came back.
async fn cancel_a_slow_host_call_on(engine: &Engine) -> Result<String, String> {
    let component = Component::new(engine, DEADLINE).await.map_err(fail)?;
    let mut linker: Linker<HostState> = Linker::new(engine);
    let upstream_id: InterfaceIdentifier = "wcmp:deadline/upstream@0.1.0".parse().map_err(fail)?;
    let watch = Arc::new(Mutex::new(FetchWatch::default()));
    let request = {
        let mut upstream = linker.instance(&upstream_id);
        let request = upstream
            .resource(
                "request",
                |state: &mut HostState, rep: u32| -> wcmp::Result<()> {
                    state.dropped.push(rep);
                    Ok(())
                },
            )
            .map_err(fail)?;
        let fetch_type = FunctionType {
            parameters: vec![FunctionParameter {
                name: "req".to_owned(),
                ty: ValueType::Borrow(ResourceType::new("request")),
            }],
            result: Some(ValueType::Primitive(PrimitiveType::String)),
            async_: true,
        };
        let watched = watch.clone();
        upstream
            .func_new_concurrent(
                "fetch",
                fetch_type,
                move |accessor: &Accessor<HostState>, args: Vec<Val>| {
                    let lent = match args.first() {
                        Some(Val::Borrow(request)) => Some(request.rep()),
                        _ => None,
                    };
                    let guard = WatchDrop {
                        accessor: accessor.clone(),
                        watch: watched.clone(),
                    };
                    async move {
                        if let Ok(mut watch) = guard.watch.lock() {
                            watch.lent = lent;
                        }
                        // Nothing the store owns can resolve this.
                        Outside::pause(SLOW_FETCH_MILLIS).await;
                        if let Ok(mut watch) = guard.watch.lock() {
                            watch.answered = true;
                        }
                        Ok(vec![Val::String("the upstream body".to_owned())])
                    }
                },
            )
            .map_err(fail)?;
        upstream
            .func_wrap_concurrent(
                "sleep",
                |_accessor: &Accessor<HostState>, (millis,): (u32,)| async move {
                    Outside::pause(millis).await;
                    Ok(())
                },
            )
            .map_err(fail)?;
        request
    };
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .map_err(fail)?;
    let handle = instance
        .exports()
        .instance("wcmp:deadline/handler@0.1.0")
        .ok_or("no `wcmp:deadline/handler` export")?
        .func("handle")
        .ok_or("no `handle` export")?;
    let request = store.resource_new(request, REQUEST_REP).map_err(fail)?;

    let answer = handle
        .call(&mut store, &[Val::Own(request), Val::U32(WAIT_MILLIS)])
        .await
        .map_err(fail)?;
    expect(
        "the handler's answer",
        answer.as_ref(),
        &[Val::String("timeout".to_owned())],
    )?;
    let watch = watch.lock().map_err(fail)?;
    expect("the request lent to `fetch`", watch.lent, Some(REQUEST_REP))?;
    expect("`fetch` answered", watch.answered, false)?;
    expect(
        "the store dropped the future of `fetch`",
        watch.dropped,
        true,
    )?;
    expect(
        "the drop ran inside a turn of the store",
        watch.dropped_in_a_turn,
        true,
    )?;
    expect(
        "the requests the handler let go",
        store.data().dropped.as_slice(),
        &[REQUEST_REP],
    )?;
    Ok(format!(
        "the {WAIT_MILLIS} ms deadline beat the {SLOW_FETCH_MILLIS} ms host call; the call \
         was cancelled, the lent request came back, and `handle` answered \"timeout\""
    ))
}

/// The `stats` fixture's `average` of an empty list divides by zero,
/// which the guest's Rust turns into a panic and its release profile
/// into an `unreachable` trap. The trap is what the call returns, and
/// it poisons the store: a call of `sum`, which has no bug, then fails
/// with the cannot-enter trap before any guest code runs. A new store
/// instantiates the same component, and `sum` answers there.
async fn trap_loses_the_store(engine: &Engine) -> Result<String, String> {
    let component = Component::new(engine, STATS).await.map_err(fail)?;
    let linker: Linker<HostState> = Linker::new(engine);
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .map_err(fail)?;
    let average = instance
        .get_func("average")
        .ok_or("no `average` export")?
        .typed::<(Vec<u32>,), u32>()
        .map_err(fail)?;
    let sum = instance
        .get_func("sum")
        .ok_or("no `sum` export")?
        .typed::<(Vec<u32>,), u32>()
        .map_err(fail)?;
    let mean = average
        .call(&mut store, (vec![2, 4],))
        .await
        .map_err(fail)?;
    expect("average([2, 4])", mean, 3)?;

    let trap = match average.call(&mut store, (Vec::new(),)).await {
        Ok(value) => return Err(format!("average([]) returned {value} instead of trapping")),
        Err(Error::Task(TaskCause::CannotEnter)) => {
            return Err("average([]) was refused before it ran".to_owned());
        }
        Err(trap) => trap,
    };
    match sum.call(&mut store, (KEYS.to_vec(),)).await {
        Err(Error::Task(TaskCause::CannotEnter)) => (),
        Ok(value) => {
            return Err(format!(
                "sum returned {value} in a store a trap had poisoned"
            ));
        }
        Err(other) => {
            return Err(format!(
                "sum failed in the poisoned store, but not with the cannot-enter trap: {}",
                chain(&other)
            ));
        }
    }

    let mut fresh = Store::new(engine, HostState::default()).map_err(fail)?;
    let instance = linker
        .instantiate(&mut fresh, &component)
        .await
        .map_err(fail)?;
    let total = instance
        .get_func("sum")
        .ok_or("no `sum` export")?
        .typed::<(Vec<u32>,), u32>()
        .map_err(fail)?
        .call(&mut fresh, (KEYS.to_vec(),))
        .await
        .map_err(fail)?;
    expect("sum([1, 2, 39]) in a new store", total, KEYS.iter().sum())?;
    Ok(format!(
        "average([2, 4]) = {mean}; average([]) trapped ({}); the next call failed with \
         \"{}\"; a new store answered sum([1, 2, 39]) = {total}",
        root_cause(&trap),
        TaskCause::CannotEnter
    ))
}

/// The innermost message of an error's chain, which for a trap is the
/// runtime's own words for it, without a backtrace the runtime may
/// add around it.
fn root_cause(error: &Error) -> String {
    let mut link: &(dyn std::error::Error + 'static) = error;
    while let Some(source) = link.source() {
        link = source;
    }
    link.to_string()
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// A write [`STORE_AND_CALLER`]'s store accepts: a key and a value
/// within its 16-byte limit.
const FITS: (&str, &str) = ("name", "polyfill");

/// A write the store fails: the value is over its 16-byte limit.
const TOO_LONG: (&str, &str) = ("motto", "errors cross components as values");

/// Every message in an error's chain, as a failed story reports it,
/// so a trap inside a guest names its cause.
fn traced(error: Error) -> String {
    chain(&error)
}

/// The default engine refuses [`STORE_AND_CALLER`] and names the gate.
/// An engine with the gate on instantiates it and the `error-reporter`
/// fixture in one store. `save` of a value that fits succeeds; `save`
/// of one that does not comes back with the message the caller read
/// from the store's error context and that same error context, which
/// the host takes as a `Val::ErrorContext`. The host hands it to the
/// reporter, a component of its own, whose `describe` reads the same
/// message and drops its handle. The host still holds the error, so it
/// hands it over a second time, and the reporter reads it again.
async fn error_between_components(engine: &Engine) -> Result<String, String> {
    match Component::new(engine, STORE_AND_CALLER).await {
        Ok(_) => return Err("the default engine accepted an `error-context`".to_owned()),
        Err(error) if error.to_string().contains("error-context feature") => (),
        Err(other) => return Err(format!("unexpected rejection: {other}")),
    }
    let mut config = EngineConfig::new();
    config.wasm_component_model_error_context(true);
    let engine = configured_engine(&config).map_err(traced)?;
    let saving = Component::new(&engine, STORE_AND_CALLER)
        .await
        .map_err(traced)?;
    let reporting = Component::new(&engine, REPORTER).await.map_err(traced)?;
    let linker: Linker<HostState> = Linker::new(&engine);
    let mut store = Store::new(&engine, HostState::default()).map_err(traced)?;
    let saving = linker
        .instantiate(&mut store, &saving)
        .await
        .map_err(traced)?;
    let reporting = linker
        .instantiate(&mut store, &reporting)
        .await
        .map_err(traced)?;
    let save = saving.get_func("save").ok_or("no `save` export")?;
    let describe = reporting
        .get_func("describe")
        .ok_or("no `describe` export")?;

    let saved = save
        .call(
            &mut store,
            &[
                Val::String(FITS.0.to_owned()),
                Val::String(FITS.1.to_owned()),
            ],
        )
        .await
        .map_err(traced)?;
    expect(
        "a write that fits",
        saved.as_ref(),
        &[Val::Result(Ok(None))],
    )?;

    let failed = save
        .call(
            &mut store,
            &[
                Val::String(TOO_LONG.0.to_owned()),
                Val::String(TOO_LONG.1.to_owned()),
            ],
        )
        .await
        .map_err(traced)?;
    let [Val::Result(Err(Some(failure)))] = failed.as_ref() else {
        return Err(format!("a write over the limit answered {failed:?}"));
    };
    let Val::Tuple(parts) = failure.as_ref() else {
        return Err(format!("the failure is not a tuple: {failure:?}"));
    };
    let [Val::String(logged), Val::ErrorContext(error)] = parts.as_ref() else {
        return Err(format!("the failure carries {parts:?}"));
    };
    let wanted = format!(
        "cannot write `{}`: the value is over the 16-byte limit",
        TOO_LONG.0
    );
    expect(
        "the message the caller read",
        logged.as_str(),
        wanted.as_str(),
    )?;

    let mut described = Vec::new();
    for _ in 0..2 {
        let answer = describe
            .call(&mut store, &[Val::ErrorContext(error.clone())])
            .await
            .map_err(traced)?;
        let [Val::String(message)] = answer.as_ref() else {
            return Err(format!("`describe` answered {answer:?}"));
        };
        described.push(message.clone());
    }
    expect(
        "the message the reporter read, each time",
        described.as_slice(),
        &[wanted.clone(), wanted.clone()],
    )?;
    Ok(format!(
        "refused while off; with it on, {wanted:?} reached the host and a third component \
         intact"
    ))
}

/// How many times the program in [`CANCELLED_WORKER`] gives way before
/// it cancels, which is how many turns the worker gets.
const PROGRAM_YIELDS: u32 = 3;

/// What the host is told in [`CANCELLED_WORKER`]: the worker's yield
/// answers 0 on each of the turns the program gives it, the program
/// says 50 as it cancels, and the worker's next yield answers 1.
const WORKER_TALLIES: [u32; 5] = [0, 0, 0, 50, 1];

/// The state of a subtask whose callee confirmed a cancellation.
const CANCELLED_BEFORE_RETURNED: u32 = 4;

/// `binary` with the `cancellable` immediate of its first
/// `thread.yield` set, as a toolchain that emits the immediate encodes
/// it. The text format no longer spells the immediate, so the
/// component is assembled with the byte zero.
fn with_cancellable_yield(binary: &[u8]) -> Result<Vec<u8>, String> {
    for payload in wasmparser::Parser::new(0).parse_all(binary) {
        let wasmparser::Payload::ComponentCanonicalSection(section) = payload.map_err(fail)? else {
            continue;
        };
        for entry in section.into_iter_with_offsets() {
            let (offset, function) = entry.map_err(fail)?;
            if let wasmparser::CanonicalFunction::ThreadYield { .. } = function {
                let immediate = usize::try_from(offset).map_err(fail)? + 1;
                let mut patched = binary.to_vec();
                expect(
                    "the immediate of the assembled `thread.yield`",
                    patched[immediate],
                    0,
                )?;
                patched[immediate] = 1;
                return Ok(patched);
            }
        }
    }
    Err("the component defines no `thread.yield`".to_owned())
}

/// An engine that accepts [`CANCELLED_WORKER`]: stackful exports and
/// the thread built-ins, as [`suspending_engine`] allows them, and the
/// asynchronous form of `subtask.cancel`. Wasmtime keeps each behind a
/// feature gate that is off by default.
fn cancelling_engine(suspending: bool) -> Result<Engine, String> {
    let mut config = EngineConfig::new();
    config.wasm_component_model_async_stackful(true);
    config.wasm_component_model_threading(true);
    config.wasm_component_model_more_async_builtins(true);
    config.suspend_provider(suspending);
    configured_engine(&config).map_err(fail)
}

/// The program in [`CANCELLED_WORKER`] calls the library's `work`,
/// gives way so the worker it started runs, and cancels the call. The
/// worker's cancellable yield answers 0 on each turn it gets and 1
/// once the program has cancelled, the worker confirms with
/// `task.cancel`, and the program reads `CANCELLED_BEFORE_RETURNED`.
/// Answers what the host was told: each answer of the worker's yields,
/// with the program's 50 where it cancelled.
///
/// Nothing in it needs a stack switch. The program gives way through
/// its callback loop rather than by setting its stack aside, so with
/// no provider the worker's yield runs the program in a turn nested
/// above it on the one stack, and the program's cancel reaches the
/// worker's yield when that turn ends.
async fn cancelled_worker_on(engine: &Engine) -> Result<Vec<u32>, String> {
    let binary = with_cancellable_yield(CANCELLED_WORKER)?;
    let component = Component::new(engine, &binary).await.map_err(fail)?;
    let linker = tallying(engine)?;
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .map_err(fail)?;
    let run = instance
        .get_func("run")
        .ok_or("no `run` export")?
        .typed::<(u32,), u32>()
        .map_err(fail)?;

    let state = run
        .call(&mut store, (PROGRAM_YIELDS,))
        .await
        .map_err(fail)?;
    let tallies = store.data().tallies.clone();
    expect(
        "the state the cancelled call resolved to",
        state,
        CANCELLED_BEFORE_RETURNED,
    )?;
    expect(
        "what the worker's yields answered, and where the program cancelled",
        tallies.as_slice(),
        &WORKER_TALLIES,
    )?;
    Ok(tallies)
}

/// The cancelled worker on an engine with a provider where the target
/// offers one, and on an engine with suspending turned off.
async fn stop_a_guest_thread() -> Result<String, String> {
    let here = cancelled_worker_on(&cancelling_engine(true)?).await?;
    cancelled_worker_on(&cancelling_engine(false)?).await?;
    Ok(format!(
        "the worker yielded {} times, saw the cancel on the next yield, and stopped; the \
         caller saw the call cancelled before it returned; same with suspending off",
        here.len() - 2
    ))
}

/// The interface the `wasi-http` handler imports its request and
/// response types from, which only a host supplies.
const WASI_HTTP_TYPES: &str = "wasi:http/types@0.3.0";

/// The polyfill runs the `wasi-http` handler, whose request and
/// response carry streams and futures and whose binding layer links
/// the cancel built-ins, once a host supplies `wasi:http/types`; the
/// polyfill's repository test `baseline_wasi_http_handler` does that
/// and calls both exports. This step supplies no such host, so it
/// asserts that the polyfill translates the handler and that
/// instantiation stops at link and names that import, as
/// `tests/corpus/expected-failures.txt` does for the fixture's
/// directives. Matching the import rather than any error keeps an
/// unrelated decode or translation bug from passing as the expected
/// failure. Once the smoke host supplies the import, this step takes
/// the call the fixture's assertions describe.
async fn wasi_http(engine: &Engine) -> Result<String, String> {
    let component = Component::new(engine, WASI_HTTP)
        .await
        .map_err(|error| format!("the polyfill no longer translates the handler: {error}"))?;
    let linker: Linker<HostState> = Linker::new(engine);
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    match linker.instantiate(&mut store, &component).await {
        Ok(_) => Err(format!(
            "the handler instantiated without a host for {WASI_HTTP_TYPES}: give this \
             step the call `fixtures/wasi-http/assertions.wast` describes, and take the \
             fixture off the expected-failure list"
        )),
        Err(Error::Link(link))
            if matches!(
                &*link,
                LinkError::UnresolvedImport { import, item: None }
                    if import.to_string() == WASI_HTTP_TYPES
            ) =>
        {
            Ok(format!(
                "the handler loaded; linking stopped and named {WASI_HTTP_TYPES}"
            ))
        }
        Err(other) => Err(format!(
            "instantiating the handler failed, but not at link for {WASI_HTTP_TYPES} \
             as `tests/corpus/expected-failures.txt` records: {other}"
        )),
    }
}
