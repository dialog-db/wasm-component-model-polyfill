// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The guest of the world `test:dual/dual`, which every run runs.

use wcmp_macros::component;

/// The guest of `wit-dual/`.
///
/// Its `test:dual/guest` exports `accumulator`, whose rep is the address
/// of its total; `shape-of`, which returns `circle(x + y)`; and a
/// `call-*` function for each import, which calls it and returns what
/// it returned. `call-answer`, `call-lost`, and `call-pairs` are lifted
/// `async` with a callback, and wait for a call or a read that does not
/// finish where it starts. `call-pairs` marks every byte of a buffer of
/// four records' room with `0x55`, reads the stream `pairs` returns
/// into it, and returns the buffer's eight words, so a run records what
/// the read wrote and what it left alone. The interface is a component of its own that imports the
/// functions and the types and exports them again, so that each
/// function's type names the types the interface exports.
pub const GUEST: &[u8] = component!(
    r#"
    (component
      (import "test:dual/types" (instance $types
        (type $point' (record (field "x" u32) (field "y" u32)))
        (export "point" (type $point (eq $point')))
        (type $shape' (variant (case "circle" u32) (case "square" $point) (case "none")))
        (export "shape" (type $shape (eq $shape')))
        (type $perms' (flags "read" "write"))
        (export "perms" (type $perms (eq $perms')))
        (type $pair' (record (field "a" u8) (field "b" u32)))
        (export "pair" (type $pair (eq $pair')))))
      (alias export $types "point" (type $point))
      (alias export $types "shape" (type $shape))
      (alias export $types "perms" (type $perms))
      (alias export $types "pair" (type $pair))
      (import "test:dual/host" (instance $host
        (alias outer 1 $point (type $point'))
        (export "point" (type $hpoint (eq $point')))
        (alias outer 1 $shape (type $shape'))
        (export "shape" (type $hshape (eq $shape')))
        (alias outer 1 $perms (type $perms'))
        (export "perms" (type $hperms (eq $perms')))
        (alias outer 1 $pair (type $pair'))
        (export "pair" (type $hpair (eq $pair')))
        (export "counter" (type $counter (sub resource)))
        (export "[constructor]counter" (func (param "start" u32) (result (own $counter))))
        (export "[method]counter.get" (func (param "self" (borrow $counter)) (result u32)))
        (type $fs-error' (enum "not-found" "denied"))
        (export "fs-error" (type $fs-error (eq $fs-error')))
        (export "describe" (func
          (param "shape" $hshape) (param "perms" $hperms) (result $hpoint)))
        (export "checked" (func (param "x" u32) (result u32)))
        (export "read" (func (param "code" u32) (result (result u32 (error $fs-error)))))
        (export "answer" (func async (param "x" u32) (result u32)))
        (type $future (future u32))
        (export "lost" (func (result $future)))
        (type $stream (stream $hpair))
        (export "pairs" (func (result $stream)))))
      (alias export $host "counter" (type $counter))
      (alias export $host "fs-error" (type $fs-error))
      (alias export $host "[constructor]counter" (func $counter-new))
      (alias export $host "[method]counter.get" (func $counter-get))
      (alias export $host "describe" (func $describe))
      (alias export $host "checked" (func $checked))
      (alias export $host "read" (func $read))
      (alias export $host "answer" (func $answer))
      (alias export $host "lost" (func $lost))
      (alias export $host "pairs" (func $pairs))

      (core module $libc (memory (export "memory") 1))
      (core instance $libc (instantiate $libc))
      (type $future (future u32))
      (type $pair-stream (stream $pair))
      (type $accumulator (resource (rep i32)))
      (core func $counter-new-lower (canon lower (func $counter-new)))
      (core func $counter-get-lower (canon lower (func $counter-get)))
      (core func $counter-drop (canon resource.drop $counter))
      (core func $describe-lower
        (canon lower (func $describe) (memory (core memory $libc "memory"))))
      (core func $checked-lower (canon lower (func $checked)))
      (core func $read-lower (canon lower (func $read) (memory (core memory $libc "memory"))))
      (core func $answer-lower
        (canon lower (func $answer) async (memory (core memory $libc "memory"))))
      (core func $lost-lower (canon lower (func $lost)))
      (core func $future-read
        (canon future.read $future async (memory (core memory $libc "memory"))))
      (core func $future-drop (canon future.drop-readable $future))
      (core func $pairs-lower (canon lower (func $pairs)))
      (core func $stream-read
        (canon stream.read $pair-stream async (memory (core memory $libc "memory"))))
      (core func $stream-drop (canon stream.drop-readable $pair-stream))
      (core func $task-return-words
        (canon task.return (result (list u32)) (memory (core memory $libc "memory"))))
      (core func $accumulator-new (canon resource.new $accumulator))
      (core func $task-return (canon task.return (result u32)))
      (core func $set-new (canon waitable-set.new))
      (core func $join (canon waitable.join))

      (core module $m
        (import "libc" "memory" (memory 1))
        (import "" "counter-new" (func $counter-new (param i32) (result i32)))
        (import "" "counter-get" (func $counter-get (param i32) (result i32)))
        (import "" "counter-drop" (func $counter-drop (param i32)))
        (import "" "describe" (func $describe (param i32 i32 i32 i32 i32)))
        (import "" "checked" (func $checked (param i32) (result i32)))
        (import "" "read" (func $read (param i32 i32)))
        (import "" "answer" (func $answer (param i32 i32) (result i32)))
        (import "" "lost" (func $lost (result i32)))
        (import "" "future-read" (func $future-read (param i32 i32) (result i32)))
        (import "" "future-drop" (func $future-drop (param i32)))
        (import "" "pairs" (func $pairs (result i32)))
        (import "" "stream-read" (func $stream-read (param i32 i32 i32) (result i32)))
        (import "" "stream-drop" (func $stream-drop (param i32)))
        (import "" "task.return-words" (func $task-return-words (param i32 i32)))
        (import "" "accumulator-new" (func $accumulator-new (param i32) (result i32)))
        (import "" "task.return" (func $task-return (param i32)))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (import "" "waitable.join" (func $join (param i32 i32)))
        (global $bump (mut i32) (i32.const 1024))
        (global $end (mut i32) (i32.const 0))
        (global $pairs (mut i32) (i32.const 0))

        ;; Wait on the waitable `w` in a set of its own: the callback's
        ;; code to return.
        (func $wait (param $w i32) (result i32)
          (local $set i32)
          (local.set $set (call $set-new))
          (call $join (local.get $w) (local.get $set))
          (i32.or (i32.shl (local.get $set) (i32.const 4)) (i32.const 2)))

        (func (export "accumulator") (param $start i32) (result i32)
          (local $at i32)
          (local.set $at (global.get $bump))
          (global.set $bump (i32.add (global.get $bump) (i32.const 4)))
          (i32.store (local.get $at) (local.get $start))
          (call $accumulator-new (local.get $at)))
        (func (export "total") (param $at i32) (result i32) (i32.load (local.get $at)))

        (func (export "shape-of") (param $x i32) (param $y i32) (result i32)
          (i32.store8 (i32.const 16) (i32.const 0))
          (i32.store offset=4 (i32.const 16) (i32.add (local.get $x) (local.get $y)))
          (i32.const 16))
        (func (export "call-describe") (param $r i32) (result i32)
          (call $describe (i32.const 0) (local.get $r) (i32.const 0) (i32.const 3) (i32.const 64))
          (i32.const 64))
        (func (export "call-checked") (param i32) (result i32) (call $checked (local.get 0)))
        (func (export "call-read") (param $code i32) (result i32)
          (call $read (local.get $code) (i32.const 80))
          (i32.const 80))
        (func (export "call-counter") (param $start i32) (result i32)
          (local $h i32) (local $v i32)
          (local.set $h (call $counter-new (local.get $start)))
          (local.set $v (call $counter-get (local.get $h)))
          (call $counter-drop (local.get $h))
          (local.get $v))

        (func (export "call-answer") (param $x i32) (result i32)
          (local $status i32)
          (local.set $status (call $answer (local.get $x) (i32.const 96)))
          (if (i32.eq (i32.and (local.get $status) (i32.const 0xf)) (i32.const 2))
            (then
              (call $task-return (i32.load (i32.const 96)))
              (return (i32.const 0))))
          (call $wait (i32.shr_u (local.get $status) (i32.const 4))))
        (func (export "answer-callback") (param i32 i32 i32) (result i32)
          (call $task-return (i32.load (i32.const 96)))
          (i32.const 0))

        (func (export "call-lost") (result i32)
          (local $read i32)
          (global.set $end (call $lost))
          (local.set $read (call $future-read (global.get $end) (i32.const 112)))
          (if (i32.ne (local.get $read) (i32.const -1))
            (then
              (call $future-drop (global.get $end))
              (call $task-return (i32.load (i32.const 112)))
              (return (i32.const 0))))
          (call $wait (global.get $end)))
        (func (export "lost-callback") (param i32 i32 i32) (result i32)
          (call $future-drop (global.get $end))
          (call $task-return (i32.load (i32.const 112)))
          (i32.const 0))

        ;; The buffer of `call-pairs`: four records of eight bytes at 256.
        (func (export "call-pairs") (result i32)
          (memory.fill (i32.const 256) (i32.const 0x55) (i32.const 32))
          (global.set $pairs (call $pairs))
          (if (i32.ne
                (call $stream-read (global.get $pairs) (i32.const 256) (i32.const 4))
                (i32.const -1))
            (then (call $return-pairs) (return (i32.const 0))))
          (call $wait (global.get $pairs)))
        (func (export "pairs-callback") (param i32 i32 i32) (result i32)
          (call $return-pairs)
          (i32.const 0))
        (func $return-pairs
          (call $stream-drop (global.get $pairs))
          (call $task-return-words (i32.const 256) (i32.const 8))))
      (core instance $i (instantiate $m
        (with "libc" (instance $libc))
        (with "" (instance
          (export "counter-new" (func $counter-new-lower))
          (export "counter-get" (func $counter-get-lower))
          (export "counter-drop" (func $counter-drop))
          (export "describe" (func $describe-lower))
          (export "checked" (func $checked-lower))
          (export "read" (func $read-lower))
          (export "answer" (func $answer-lower))
          (export "lost" (func $lost-lower))
          (export "future-read" (func $future-read))
          (export "future-drop" (func $future-drop))
          (export "pairs" (func $pairs-lower))
          (export "stream-read" (func $stream-read))
          (export "stream-drop" (func $stream-drop))
          (export "task.return-words" (func $task-return-words))
          (export "accumulator-new" (func $accumulator-new))
          (export "task.return" (func $task-return))
          (export "waitable-set.new" (func $set-new))
          (export "waitable.join" (func $join))))))

      (func $accumulator-constructor (param "start" u32) (result (own $accumulator))
        (canon lift (core func $i "accumulator")))
      (func $accumulator-total (param "self" (borrow $accumulator)) (result u32)
        (canon lift (core func $i "total")))
      (func $shape-of (param "point" $point) (result $shape)
        (canon lift (core func $i "shape-of") (memory (core memory $libc "memory"))))
      (func $call-describe (param "r" u32) (result $point)
        (canon lift (core func $i "call-describe") (memory (core memory $libc "memory"))))
      (func $call-checked (param "x" u32) (result u32)
        (canon lift (core func $i "call-checked")))
      (func $call-read (param "code" u32) (result (result u32 (error $fs-error)))
        (canon lift (core func $i "call-read") (memory (core memory $libc "memory"))))
      (func $call-counter (param "start" u32) (result u32)
        (canon lift (core func $i "call-counter")))
      (func $call-answer async (param "x" u32) (result u32)
        (canon lift (core func $i "call-answer") async
          (callback (core func $i "answer-callback"))))
      (func $call-lost async (result u32)
        (canon lift (core func $i "call-lost") async
          (callback (core func $i "lost-callback"))))
      (func $call-pairs async (result (list u32))
        (canon lift (core func $i "call-pairs") async (memory (core memory $libc "memory"))
          (callback (core func $i "pairs-callback"))))

      (component $interface
        (type $point0 (record (field "x" u32) (field "y" u32)))
        (import "point" (type $p (eq $point0)))
        (type $shape0 (variant (case "circle" u32) (case "square" $p) (case "none")))
        (import "shape" (type $s (eq $shape0)))
        (type $fs-error0 (enum "not-found" "denied"))
        (import "fs-error" (type $f (eq $fs-error0)))
        (import "accumulator" (type $a (sub resource)))
        (import "constructor" (func $constructor (param "start" u32) (result (own $a))))
        (import "total" (func $total (param "self" (borrow $a)) (result u32)))
        (import "shape-of" (func $shape-of (param "point" $p) (result $s)))
        (import "call-describe" (func $call-describe (param "r" u32) (result $p)))
        (import "call-checked" (func $call-checked (param "x" u32) (result u32)))
        (import "call-read" (func $call-read (param "code" u32) (result (result u32 (error $f)))))
        (import "call-counter" (func $call-counter (param "start" u32) (result u32)))
        (import "call-answer" (func $call-answer async (param "x" u32) (result u32)))
        (import "call-lost" (func $call-lost async (result u32)))
        (import "call-pairs" (func $call-pairs async (result (list u32))))
        (export $pe "point" (type $p))
        (export $se "shape" (type $s))
        (export $fe "fs-error" (type $f))
        (export $ae "accumulator" (type $a))
        (export "[constructor]accumulator" (func $constructor)
          (func (param "start" u32) (result (own $ae))))
        (export "[method]accumulator.total" (func $total)
          (func (param "self" (borrow $ae)) (result u32)))
        (export "shape-of" (func $shape-of) (func (param "point" $pe) (result $se)))
        (export "call-describe" (func $call-describe) (func (param "r" u32) (result $pe)))
        (export "call-checked" (func $call-checked))
        (export "call-read" (func $call-read)
          (func (param "code" u32) (result (result u32 (error $fe)))))
        (export "call-counter" (func $call-counter))
        (export "call-answer" (func $call-answer))
        (export "call-lost" (func $call-lost))
        (export "call-pairs" (func $call-pairs)))
      (instance $guest (instantiate $interface
        (with "point" (type $point))
        (with "shape" (type $shape))
        (with "fs-error" (type $fs-error))
        (with "accumulator" (type $accumulator))
        (with "constructor" (func $accumulator-constructor))
        (with "total" (func $accumulator-total))
        (with "shape-of" (func $shape-of))
        (with "call-describe" (func $call-describe))
        (with "call-checked" (func $call-checked))
        (with "call-read" (func $call-read))
        (with "call-counter" (func $call-counter))
        (with "call-answer" (func $call-answer))
        (with "call-lost" (func $call-lost))
        (with "call-pairs" (func $call-pairs))))
      (export "test:dual/guest" (instance $guest)))
    "#
);
