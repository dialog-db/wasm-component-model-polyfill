// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Guests more than one test file runs.

#![allow(dead_code)]

use wcmp_macros::component;

/// A guest that implements the world `exports` of `wit-exports/`:
/// `shape-of` returns `circle(x + y)`, `echo` returns the string it is
/// given, and `add` adds.
pub const IMPLEMENTS_EXPORTS: &[u8] = component!(
    r#"
    (component
      (core module $m
        (memory (export "memory") 1)
        (global $bump (mut i32) (i32.const 1024))
        (func (export "cabi_realloc")
              (param $old i32) (param $old-size i32) (param $align i32) (param $size i32)
              (result i32)
          (local $ptr i32)
          global.get $bump local.get $align i32.add i32.const 1 i32.sub
          local.get $align i32.const 1 i32.sub i32.const -1 i32.xor i32.and
          local.set $ptr
          local.get $ptr local.get $size i32.add global.set $bump
          local.get $ptr)
        (func (export "shape-of") (param $x i32) (param $y i32) (result i32)
          (i32.store (i32.const 16) (i32.const 0))
          (i32.store offset=4 (i32.const 16) (i32.add (local.get $x) (local.get $y)))
          (i32.const 16))
        (func (export "echo") (param $ptr i32) (param $len i32) (result i32)
          (i32.store (i32.const 32) (local.get $ptr))
          (i32.store offset=4 (i32.const 32) (local.get $len))
          (i32.const 32))
        (func (export "add") (param i32 i32) (result i32)
          (i32.add (local.get 0) (local.get 1))))
      (core instance $i (instantiate $m))
      (type $point (record (field "x" u32) (field "y" u32)))
      (type $shape (variant (case "circle" u32) (case "none")))
      (func $shape-of (param "point" $point) (result $shape)
        (canon lift (core func $i "shape-of") (memory (core memory $i "memory"))))
      (func $echo (param "text" string) (result string)
        (canon lift (core func $i "echo") (memory (core memory $i "memory"))
          (realloc (core func $i "cabi_realloc"))))
      (instance $shapes
        (export "point" (type $point))
        (export "shape" (type $shape))
        (export "shape-of" (func $shape-of))
        (export "echo" (func $echo)))
      (export "test:exports/shapes" (instance $shapes))
      (func (export "add") (param "a" u32) (param "b" u32) (result u32)
        (canon lift (core func $i "add"))))
    "#
);

/// The guest of `wit-resources/`.
///
/// `run` makes a counter from `start`, increments it, reads it through
/// `users`, gets it, drops it, and returns the read times 1000 plus the
/// get. `test:resources/guest-made` defines `accumulator`, whose rep is
/// the address of its total.
pub const RESOURCES: &[u8] = component!(
    r#"
    (component
      (import "test:resources/counters" (instance $counters
        (export "counter" (type $counter (sub resource)))
        (export "[constructor]counter" (func (param "start" u32) (result (own $counter))))
        (export "[method]counter.get" (func (param "self" (borrow $counter)) (result u32)))
        (export "[method]counter.increment" (func (param "self" (borrow $counter))))))
      (alias export $counters "counter" (type $counter))
      (import "test:resources/users" (instance $users
        (alias outer 1 $counter (type $outer))
        (export "counter" (type $c (eq $outer)))
        (export "read" (func (param "c" (borrow $c)) (result u32)))))
      (alias export $counters "[constructor]counter" (func $new))
      (alias export $counters "[method]counter.get" (func $get))
      (alias export $counters "[method]counter.increment" (func $increment))
      (alias export $users "read" (func $read))
      (core func $new-lower (canon lower (func $new)))
      (core func $get-lower (canon lower (func $get)))
      (core func $increment-lower (canon lower (func $increment)))
      (core func $read-lower (canon lower (func $read)))
      (core func $drop (canon resource.drop $counter))

      (type $accumulator (resource (rep i32)))
      (core func $accumulator-new (canon resource.new $accumulator))

      (core module $m
        (import "" "new" (func $new (param i32) (result i32)))
        (import "" "get" (func $get (param i32) (result i32)))
        (import "" "increment" (func $increment (param i32)))
        (import "" "read" (func $read (param i32) (result i32)))
        (import "" "drop" (func $drop (param i32)))
        (import "" "accumulator-new" (func $accumulator-new (param i32) (result i32)))
        (memory 1)
        (global $bump (mut i32) (i32.const 64))
        (func (export "run") (param $start i32) (result i32)
          (local $h i32) (local $read i32) (local $got i32)
          (local.set $h (call $new (local.get $start)))
          (call $increment (local.get $h))
          (local.set $read (call $read (local.get $h)))
          (local.set $got (call $get (local.get $h)))
          (call $drop (local.get $h))
          (i32.add (i32.mul (local.get $read) (i32.const 1000)) (local.get $got)))
        (func (export "accumulator") (param $start i32) (result i32)
          (local $at i32)
          (local.set $at (global.get $bump))
          (global.set $bump (i32.add (global.get $bump) (i32.const 4)))
          (i32.store (local.get $at) (local.get $start))
          (call $accumulator-new (local.get $at)))
        (func (export "total") (param $at i32) (result i32) (i32.load (local.get $at)))
        (func (export "add") (param $at i32) (param $x i32)
          (i32.store (local.get $at) (i32.add (i32.load (local.get $at)) (local.get $x)))))
      (core instance $i (instantiate $m
        (with "" (instance
          (export "new" (func $new-lower))
          (export "get" (func $get-lower))
          (export "increment" (func $increment-lower))
          (export "read" (func $read-lower))
          (export "drop" (func $drop))
          (export "accumulator-new" (func $accumulator-new))))))

      (func (export "run") (param "start" u32) (result u32) (canon lift (core func $i "run")))
      (func $accumulator-constructor (param "start" u32) (result (own $accumulator))
        (canon lift (core func $i "accumulator")))
      (func $accumulator-total (param "self" (borrow $accumulator)) (result u32)
        (canon lift (core func $i "total")))
      (func $accumulator-add (param "self" (borrow $accumulator)) (param "x" u32)
        (canon lift (core func $i "add")))
      ;; The interface is a component of its own that imports the
      ;; resource and its functions and exports them again, so that each
      ;; function's type names the resource it exports.
      (component $interface
        (import "accumulator" (type $a (sub resource)))
        (import "constructor" (func $c (param "start" u32) (result (own $a))))
        (import "total" (func $t (param "self" (borrow $a)) (result u32)))
        (import "add" (func $d (param "self" (borrow $a)) (param "x" u32)))
        (export $e "accumulator" (type $a))
        (export "[constructor]accumulator" (func $c)
          (func (param "start" u32) (result (own $e))))
        (export "[method]accumulator.total" (func $t)
          (func (param "self" (borrow $e)) (result u32)))
        (export "[method]accumulator.add" (func $d)
          (func (param "self" (borrow $e)) (param "x" u32))))
      (instance $guest-made (instantiate $interface
        (with "accumulator" (type $accumulator))
        (with "constructor" (func $accumulator-constructor))
        (with "total" (func $accumulator-total))
        (with "add" (func $accumulator-add))))
      (export "test:resources/guest-made" (instance $guest-made)))
    "#
);
