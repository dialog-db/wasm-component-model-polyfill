;; Copyright 2026 The Dialog DB Project
;;
;; This Source Code Form is subject to the terms of the Mozilla Public
;; License, v. 2.0. If a copy of the MPL was not distributed with this
;; file, You can obtain one at https://mozilla.org/MPL/2.0/.

;; The core module behind the `fixed-lists` world. A `list<u32, 4>`
;; and a `list<u8, 16>` both fit the flat form, so they arrive as
;; four and sixteen `i32` parameters. A `list<u8, 16>` result does not
;; fit the single flat result slot, so the function returns the
;; address of sixteen bytes it wrote.
(module
  (memory (export "memory") 1)
  (global $bump (mut i32) (i32.const 16))
  (func $realloc (export "cabi_realloc")
        (param $old i32) (param $old-size i32) (param $align i32) (param $size i32)
        (result i32)
    (local $ptr i32)
    (local.set $ptr
      (i32.and
        (i32.add (global.get $bump) (i32.sub (local.get $align) (i32.const 1)))
        (i32.sub (i32.const 0) (local.get $align))))
    (global.set $bump (i32.add (local.get $ptr) (local.get $size)))
    (local.get $ptr))

  (func (export "sum") (param i32 i32 i32 i32) (result i32)
    (i32.add (i32.add (local.get 0) (local.get 1)) (i32.add (local.get 2) (local.get 3))))

  (func (export "double") (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32) (result i32)
    (local $ret i32)
    (local.set $ret (call $realloc (i32.const 0) (i32.const 0) (i32.const 1) (i32.const 16)))
    (i32.store8 offset=0 (local.get $ret) (i32.mul (local.get 0) (i32.const 2)))
    (i32.store8 offset=1 (local.get $ret) (i32.mul (local.get 1) (i32.const 2)))
    (i32.store8 offset=2 (local.get $ret) (i32.mul (local.get 2) (i32.const 2)))
    (i32.store8 offset=3 (local.get $ret) (i32.mul (local.get 3) (i32.const 2)))
    (i32.store8 offset=4 (local.get $ret) (i32.mul (local.get 4) (i32.const 2)))
    (i32.store8 offset=5 (local.get $ret) (i32.mul (local.get 5) (i32.const 2)))
    (i32.store8 offset=6 (local.get $ret) (i32.mul (local.get 6) (i32.const 2)))
    (i32.store8 offset=7 (local.get $ret) (i32.mul (local.get 7) (i32.const 2)))
    (i32.store8 offset=8 (local.get $ret) (i32.mul (local.get 8) (i32.const 2)))
    (i32.store8 offset=9 (local.get $ret) (i32.mul (local.get 9) (i32.const 2)))
    (i32.store8 offset=10 (local.get $ret) (i32.mul (local.get 10) (i32.const 2)))
    (i32.store8 offset=11 (local.get $ret) (i32.mul (local.get 11) (i32.const 2)))
    (i32.store8 offset=12 (local.get $ret) (i32.mul (local.get 12) (i32.const 2)))
    (i32.store8 offset=13 (local.get $ret) (i32.mul (local.get 13) (i32.const 2)))
    (i32.store8 offset=14 (local.get $ret) (i32.mul (local.get 14) (i32.const 2)))
    (i32.store8 offset=15 (local.get $ret) (i32.mul (local.get 15) (i32.const 2)))
    (local.get $ret))

  (func (export "identity") (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32) (result i32)
    (local $ret i32)
    (local.set $ret (call $realloc (i32.const 0) (i32.const 0) (i32.const 1) (i32.const 16)))
    (i32.store8 offset=0 (local.get $ret) (local.get 0))
    (i32.store8 offset=1 (local.get $ret) (local.get 1))
    (i32.store8 offset=2 (local.get $ret) (local.get 2))
    (i32.store8 offset=3 (local.get $ret) (local.get 3))
    (i32.store8 offset=4 (local.get $ret) (local.get 4))
    (i32.store8 offset=5 (local.get $ret) (local.get 5))
    (i32.store8 offset=6 (local.get $ret) (local.get 6))
    (i32.store8 offset=7 (local.get $ret) (local.get 7))
    (i32.store8 offset=8 (local.get $ret) (local.get 8))
    (i32.store8 offset=9 (local.get $ret) (local.get 9))
    (i32.store8 offset=10 (local.get $ret) (local.get 10))
    (i32.store8 offset=11 (local.get $ret) (local.get 11))
    (i32.store8 offset=12 (local.get $ret) (local.get 12))
    (i32.store8 offset=13 (local.get $ret) (local.get 13))
    (i32.store8 offset=14 (local.get $ret) (local.get 14))
    (i32.store8 offset=15 (local.get $ret) (local.get 15))
    (local.get $ret)))
