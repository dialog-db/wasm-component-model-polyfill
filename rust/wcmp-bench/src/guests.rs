// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The guests the suite drives.
//!
//! Four are the real-toolchain fixtures under the conformance corpus,
//! the same components the smoke test walks: `wasm-tools` builds from
//! WIT and WAT, and a `wac plug` composition of two of them. Three
//! more are assembled here from WebAssembly text, because no fixture
//! moves the values the canonical-ABI benchmarks need or keeps a host
//! call in flight: a component that echoes a string or a list back, a
//! component with a resource of its own to mint and drop, and a
//! component that starts many host calls and waits for them all. One
//! more keeps a shadow stack pointer in context slot 0, as a Rust
//! component built for `wasm32-wasip3` does, because no fixture reads
//! or writes a context slot.

use wcmp_macros::component;

/// The `guest` fixture: `double: func(x: u32) -> u32`, lifted with no
/// memory and no realloc, so a call through it touches no guest
/// memory at all.
pub const GUEST: &[u8] = include_bytes!("../../wcmp/tests/corpus/fixtures/guest/guest.wasm");

/// The `composition` fixture: a socket whose `run` reaches a plug's
/// `double` through the adapter `wac plug` generated between them.
pub const COMPOSITION: &[u8] =
    include_bytes!("../../wcmp/tests/corpus/fixtures/composition/composed.wasm");

/// The `maps` fixture: exports that take and return a
/// `map<string, u32>`.
pub const MAPS: &[u8] = include_bytes!("../../wcmp/tests/corpus/fixtures/maps/maps.wasm");

/// The `fixed-lists` fixture: exports that take and return a
/// `list<u32, 4>` and a `list<u8, 16>`.
pub const FIXED_LISTS: &[u8] =
    include_bytes!("../../wcmp/tests/corpus/fixtures/fixed-lists/fixed-lists.wasm");

/// A component that echoes a heap value back: the lowered pointer and
/// length are returned unchanged, so one call is one lower into guest
/// memory and one lift back out of it, with no guest work in between.
///
/// Its allocator recycles the arena once the bump passes a mark well
/// above any one payload. A benchmark calls its guest thousands of
/// times, and an allocator that only ever grows would leave the
/// memory instead of the polyfill as the thing measured.
pub const ECHO: &[u8] = component!(
    r#"
    (component
      (core module $m
        (memory (export "memory") 32)
        (global $bump (mut i32) (i32.const 16))
        (func $realloc (export "cabi_realloc")
              (param $old i32) (param $old-size i32) (param $align i32) (param $size i32)
              (result i32)
          (local $ptr i32)
          (if (i32.gt_u (global.get $bump) (i32.const 1048576))
            (then (global.set $bump (i32.const 16))))
          global.get $bump local.get $align i32.add i32.const 1 i32.sub
          local.get $align i32.const 1 i32.sub i32.const -1 i32.xor i32.and
          local.set $ptr
          local.get $ptr local.get $size i32.add global.set $bump
          local.get $ptr)
        (func (export "echo") (param $ptr i32) (param $len i32) (result i32)
          (local $ret i32)
          i32.const 0 i32.const 0 i32.const 4 i32.const 8 call $realloc local.set $ret
          local.get $ret local.get $ptr i32.store
          local.get $ret local.get $len i32.store offset=4
          local.get $ret))
      (core instance $i (instantiate $m))
      (type $point (record (field "x" u32) (field "y" u32)))
      ;; A record is a named type, and a function that takes one is
      ;; valid as an export only once the record itself is exported —
      ;; which is what a WIT interface does for the same shape.
      (export $point-out "point" (type $point))
      (func (export "echo-string") (param "s" string) (result string)
        (canon lift (core func $i "echo")
          (memory (core memory $i "memory"))
          (realloc (core func $i "cabi_realloc"))))
      (func (export "echo-list-u8") (param "xs" (list u8)) (result (list u8))
        (canon lift (core func $i "echo")
          (memory (core memory $i "memory"))
          (realloc (core func $i "cabi_realloc"))))
      (func (export "echo-list-u32") (param "xs" (list u32)) (result (list u32))
        (canon lift (core func $i "echo")
          (memory (core memory $i "memory"))
          (realloc (core func $i "cabi_realloc"))))
      (func (export "echo-list-point") (param "xs" (list $point-out)) (result (list $point-out))
        (canon lift (core func $i "echo")
          (memory (core memory $i "memory"))
          (realloc (core func $i "cabi_realloc")))))
    "#
);

/// A component whose `half: func(x: f64) -> f64` halves its argument,
/// lifted with neither a memory nor a realloc: the `u32` floor of the
/// `guest` fixture with a float in place of the integer, so the two
/// differ only in how the value crosses the core call. In the browser
/// an `f64` crosses as a `BigInt` of its bits.
pub const HALF: &[u8] = component!(
    r#"
    (component
      (core module $m
        (func (export "half") (param f64) (result f64)
          (f64.mul (local.get 0) (f64.const 0.5))))
      (core instance $i (instantiate $m))
      (func (export "half") (param "x" f64) (result f64)
        (canon lift (core func $i "half"))))
    "#
);

/// A component with a resource type of its own, a destructor for it,
/// and the two functions a handle's life needs: `make` mints one and
/// hands it out, `dispose` takes it back and drops it. The destructor
/// counts its calls, so the drop is real work and not an elided one.
pub const RESOURCE: &[u8] = component!(
    r#"
    (component
      (core module $d
        (global $dropped (mut i32) (i32.const 0))
        (func (export "dtor") (param i32)
          global.get $dropped i32.const 1 i32.add global.set $dropped)
        (func (export "dropped") (result i32) global.get $dropped))
      (core instance $di (instantiate $d))
      (type $thing (resource (rep i32) (dtor (core func $di "dtor"))))
      (core func $new (canon resource.new $thing))
      (core func $drop (canon resource.drop $thing))
      (core module $m
        (import "" "new" (func $new (param i32) (result i32)))
        (import "" "drop" (func $drop (param i32)))
        (func (export "make") (param i32) (result i32) local.get 0 call $new)
        (func (export "dispose") (param i32) local.get 0 call $drop))
      (core instance $i (instantiate $m
        (with "" (instance (export "new" (func $new)) (export "drop" (func $drop))))))
      (export $thing' "thing" (type $thing))
      (func (export "make") (param "rep" u32) (result (own $thing'))
        (canon lift (core func $i "make")))
      (func (export "dispose") (param "h" (own $thing'))
        (canon lift (core func $i "dispose")))
      (func (export "dropped") (result u32) (canon lift (core func $di "dropped"))))
    "#
);

/// A component that keeps many host calls in flight at once. Its
/// `fan-out: async func(n: u32) -> u32` calls the async-typed import
/// `answer` `n` times through an asynchronous lower, joins every
/// subtask it is given to one waitable set, and waits on the set. Its
/// callback drops each subtask whose `RETURNED` event it receives and
/// returns `n` once the last one has come back, dropping the set, so
/// nothing of a call is left in the store when the host's call
/// returns.
///
/// The guest traps unless every call starts: the benchmark's host
/// future is pending on its first poll, so a call that returned at
/// once would mean the store was not holding the host task at all.
pub const FAN_OUT: &[u8] = component!(
    r#"
    (component
      (import "answer" (func $answer async (param "x" u32) (result u32)))
      (core module $libc (memory (export "mem") 1))
      (core instance $libc (instantiate $libc))
      (core func $lowered
        (canon lower (func $answer) async (memory (core memory $libc "mem"))))
      (core func $task-return (canon task.return (result u32)))
      (core func $set-new (canon waitable-set.new))
      (core func $set-drop (canon waitable-set.drop))
      (core func $join (canon waitable.join))
      (core func $subtask-drop (canon subtask.drop))
      (core module $m
        (import "" "mem" (memory 1))
        (import "" "answer" (func $answer (param i32 i32) (result i32)))
        (import "" "task.return" (func $task-return (param i32)))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (import "" "waitable-set.drop" (func $set-drop (param i32)))
        (import "" "waitable.join" (func $join (param i32 i32)))
        (import "" "subtask.drop" (func $subtask-drop (param i32)))
        (global $set (mut i32) (i32.const 0))
        (global $calls (mut i32) (i32.const 0))
        (global $left (mut i32) (i32.const 0))
        (func (export "fan-out") (param $n i32) (result i32)
          (local $i i32)
          (local $status i32)
          (global.set $calls (local.get $n))
          (global.set $left (local.get $n))
          (global.set $set (call $set-new))
          (block $done
            (loop $next
              (br_if $done (i32.ge_u (local.get $i) (local.get $n)))
              (local.set $status (call $answer (local.get $i) (i32.const 8)))
              (if (i32.ne (i32.and (local.get $status) (i32.const 0xf)) (i32.const 1))
                (then unreachable))
              (call $join (i32.shr_u (local.get $status) (i32.const 4)) (global.get $set))
              (local.set $i (i32.add (local.get $i) (i32.const 1)))
              (br $next)))
          (i32.or (i32.shl (global.get $set) (i32.const 4)) (i32.const 2)))
        (func (export "cb") (param i32 i32 i32) (result i32)
          (if (i32.ne (local.get 0) (i32.const 1)) (then unreachable))
          (if (i32.ne (local.get 2) (i32.const 2)) (then unreachable))
          (call $join (local.get 1) (i32.const 0))
          (call $subtask-drop (local.get 1))
          (global.set $left (i32.sub (global.get $left) (i32.const 1)))
          (if (i32.ne (global.get $left) (i32.const 0))
            (then (return (i32.or (i32.shl (global.get $set) (i32.const 4)) (i32.const 2)))))
          (call $set-drop (global.get $set))
          (call $task-return (global.get $calls))
          (i32.const 0)))
      (core instance $i (instantiate $m (with "" (instance
        (export "mem" (memory $libc "mem"))
        (export "answer" (func $lowered))
        (export "task.return" (func $task-return))
        (export "waitable-set.new" (func $set-new))
        (export "waitable-set.drop" (func $set-drop))
        (export "waitable.join" (func $join))
        (export "subtask.drop" (func $subtask-drop))))))
      (func (export "fan-out") async (param "n" u32) (result u32)
        (canon lift (core func $i "fan-out") async (callback (core func $i "cb")))))
    "#
);

/// A component whose callback export `spin` gives way `n` times
/// before it returns: its core function and each callback answer
/// `YIELD` until the count runs out, and the last returns through
/// `task.return`. Each yield is one round through the store's driver,
/// which in a browser crosses a macrotask boundary.
pub const YIELDS: &[u8] = component!(
    r#"
    (component
      (core func $task-return (canon task.return))
      (core module $m
        (import "" "task.return" (func $task-return))
        (global $left (mut i32) (i32.const 0))
        (func $step (result i32)
          (if (result i32) (i32.eqz (global.get $left))
            (then
              (call $task-return)
              (i32.const 0))
            (else
              (global.set $left (i32.sub (global.get $left) (i32.const 1)))
              (i32.const 1))))
        (func (export "spin") (param $n i32) (result i32)
          (global.set $left (local.get $n))
          (call $step))
        (func (export "spin-callback") (param i32 i32 i32) (result i32)
          (call $step)))
      (core instance $i (instantiate $m (with "" (instance
        (export "task.return" (func $task-return))))))
      (func (export "spin") async (param "n" u32)
        (canon lift (core func $i "spin") async
          (callback (core func $i "spin-callback")))))
    "#
);

/// A component whose exports take the shapes `typed-acquire` checks: a
/// record of sixteen `u32` fields, `take-wide: func(w: wide)`, and a
/// record inside a variant inside a list,
/// `take-shapes: func(xs: list<shape>)` over
/// `variant shape { dot(point), empty }`. Neither is ever called, so
/// their core functions do nothing.
pub const SHAPES: &[u8] = component!(
    r#"
    (component
      (core module $m
        (memory (export "memory") 1)
        (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32) i32.const 16)
        (func (export "take-wide") (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32))
        (func (export "take-shapes") (param i32 i32)))
      (core instance $i (instantiate $m))
      (type $wide (record (field "f0" u32) (field "f1" u32) (field "f2" u32) (field "f3" u32) (field "f4" u32) (field "f5" u32) (field "f6" u32) (field "f7" u32) (field "f8" u32) (field "f9" u32) (field "f10" u32) (field "f11" u32) (field "f12" u32) (field "f13" u32) (field "f14" u32) (field "f15" u32)))
      (export $wide-out "wide" (type $wide))
      (type $point (record (field "x" u32) (field "y" u32)))
      (export $point-out "point" (type $point))
      (type $shape (variant (case "dot" $point-out) (case "empty")))
      (export $shape-out "shape" (type $shape))
      (func (export "take-wide") (param "w" $wide-out)
        (canon lift (core func $i "take-wide")))
      (func (export "take-shapes") (param "xs" (list $shape-out))
        (canon lift (core func $i "take-shapes")
          (memory (core memory $i "memory"))
          (realloc (core func $i "cabi_realloc")))))
    "#
);

/// A component that imports one host function over a record,
/// `f0: func(p: point) -> point`, and does nothing else: instantiating
/// it is linking that one import.
pub const HOSTS_1: &[u8] = component!(
    r#"
    (component
      (import "wcmp-bench:host/points@0.1.0" (instance $host
        (type $point' (record (field "x" u32) (field "y" u32)))
        (export "point" (type $point (eq $point')))
        (export "f0" (func (param "p" $point) (result $point))))))
    "#
);

/// [`HOSTS_1`] with sixteen imports, `f0` to `f15`, each of the same
/// type.
pub const HOSTS_16: &[u8] = component!(
    r#"
    (component
      (import "wcmp-bench:host/points@0.1.0" (instance $host
        (type $point' (record (field "x" u32) (field "y" u32)))
        (export "point" (type $point (eq $point')))
        (export "f0" (func (param "p" $point) (result $point)))
        (export "f1" (func (param "p" $point) (result $point)))
        (export "f2" (func (param "p" $point) (result $point)))
        (export "f3" (func (param "p" $point) (result $point)))
        (export "f4" (func (param "p" $point) (result $point)))
        (export "f5" (func (param "p" $point) (result $point)))
        (export "f6" (func (param "p" $point) (result $point)))
        (export "f7" (func (param "p" $point) (result $point)))
        (export "f8" (func (param "p" $point) (result $point)))
        (export "f9" (func (param "p" $point) (result $point)))
        (export "f10" (func (param "p" $point) (result $point)))
        (export "f11" (func (param "p" $point) (result $point)))
        (export "f12" (func (param "p" $point) (result $point)))
        (export "f13" (func (param "p" $point) (result $point)))
        (export "f14" (func (param "p" $point) (result $point)))
        (export "f15" (func (param "p" $point) (result $point))))))
    "#
);

/// A component whose four exports each run `n` stack frames of 16 bytes
/// and answer `n`. A frame stores to its stack and loads back, and the
/// four differ only in where the stack pointer lives and whether each
/// frame calls the host:
///
/// - `slot-calls: func(n: u32) -> u32` keeps the pointer in context slot
///   0, read and written with `context.get` and `context.set` on entry
///   to and exit from each frame, as a Rust function with a stack frame
///   does when built for `wasm32-wasip3`.
/// - `global-calls: func(n: u32) -> u32` keeps it in a mutable global of
///   the core module, as a build for `wasm32-wasip1` or `wasm32-wasip2`
///   does.
/// - `slot-host-calls` and `global-host-calls` are the same, with one
///   call of the host import `tick: func()` inside each frame.
pub const SHADOW_STACK: &[u8] = component!(
    r#"
    (component
      (import "tick" (func $tick))
      (core func $tick' (canon lower (func $tick)))
      (core func $cget (canon context.get i32 0))
      (core func $cset (canon context.set i32 0))
      (core module $m
        (import "" "context.get" (func $cget (result i32)))
        (import "" "context.set" (func $cset (param i32)))
        (import "" "tick" (func $tick))
        (memory 1)
        (global $sp (mut i32) (i32.const 0))

        (func $slot-frame (param $x i32) (param $host i32) (result i32)
          (local $fp i32)
          (local.set $fp (i32.sub (call $cget) (i32.const 16)))
          (call $cset (local.get $fp))
          (i32.store (local.get $fp) (local.get $x))
          (if (local.get $host) (then (call $tick)))
          (local.set $x (i32.add (i32.load (local.get $fp)) (i32.const 1)))
          (call $cset (i32.add (local.get $fp) (i32.const 16)))
          (local.get $x))
        (func $global-frame (param $x i32) (param $host i32) (result i32)
          (local $fp i32)
          (local.set $fp (i32.sub (global.get $sp) (i32.const 16)))
          (global.set $sp (local.get $fp))
          (i32.store (local.get $fp) (local.get $x))
          (if (local.get $host) (then (call $tick)))
          (local.set $x (i32.add (i32.load (local.get $fp)) (i32.const 1)))
          (global.set $sp (i32.add (local.get $fp) (i32.const 16)))
          (local.get $x))

        (func $slot-run (param $n i32) (param $host i32) (result i32)
          (local $x i32)
          (call $cset (i32.const 65536))
          (block $done
            (loop $next
              (br_if $done (i32.eqz (local.get $n)))
              (local.set $x (call $slot-frame (local.get $x) (local.get $host)))
              (local.set $n (i32.sub (local.get $n) (i32.const 1)))
              (br $next)))
          (local.get $x))
        (func $global-run (param $n i32) (param $host i32) (result i32)
          (local $x i32)
          (global.set $sp (i32.const 65536))
          (block $done
            (loop $next
              (br_if $done (i32.eqz (local.get $n)))
              (local.set $x (call $global-frame (local.get $x) (local.get $host)))
              (local.set $n (i32.sub (local.get $n) (i32.const 1)))
              (br $next)))
          (local.get $x))

        (func (export "slot-calls") (param i32) (result i32)
          (call $slot-run (local.get 0) (i32.const 0)))
        (func (export "global-calls") (param i32) (result i32)
          (call $global-run (local.get 0) (i32.const 0)))
        (func (export "slot-host-calls") (param i32) (result i32)
          (call $slot-run (local.get 0) (i32.const 1)))
        (func (export "global-host-calls") (param i32) (result i32)
          (call $global-run (local.get 0) (i32.const 1))))
      (core instance $i (instantiate $m (with "" (instance
        (export "context.get" (func $cget))
        (export "context.set" (func $cset))
        (export "tick" (func $tick'))))))
      (func (export "slot-calls") (param "n" u32) (result u32)
        (canon lift (core func $i "slot-calls")))
      (func (export "global-calls") (param "n" u32) (result u32)
        (canon lift (core func $i "global-calls")))
      (func (export "slot-host-calls") (param "n" u32) (result u32)
        (canon lift (core func $i "slot-host-calls")))
      (func (export "global-host-calls") (param "n" u32) (result u32)
        (canon lift (core func $i "global-host-calls"))))
    "#
);
