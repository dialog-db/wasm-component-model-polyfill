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
//! component that starts many host calls and waits for them all.

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
