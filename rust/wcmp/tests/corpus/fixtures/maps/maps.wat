;; Copyright 2026 The Dialog DB Project
;;
;; This Source Code Form is subject to the terms of the Mozilla Public
;; License, v. 2.0. If a copy of the MPL was not distributed with this
;; file, You can obtain one at https://mozilla.org/MPL/2.0/.

;; The core module behind the `maps` world. A `map<string, u32>`
;; arrives as a pointer and a length over entries of twelve bytes: the
;; string's pointer and length, then the `u32`. A map result spills:
;; the function returns the address of an eight-byte area holding the
;; entries' pointer and length.
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

  (func (export "sum") (param $ptr i32) (param $len i32) (result i32)
    (local $i i32)
    (local $acc i32)
    (block $done
      (loop $next
        (br_if $done (i32.ge_u (local.get $i) (local.get $len)))
        (local.set $acc
          (i32.add
            (local.get $acc)
            (i32.load offset=8
              (i32.add (local.get $ptr) (i32.mul (local.get $i) (i32.const 12))))))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $next)))
    (local.get $acc))

  (func (export "keys") (param $ptr i32) (param $len i32) (result i32)
    (local $i i32)
    (local $out i32)
    (local $ret i32)
    (local.set $out
      (call $realloc (i32.const 0) (i32.const 0) (i32.const 4)
        (i32.mul (local.get $len) (i32.const 8))))
    (block $done
      (loop $next
        (br_if $done (i32.ge_u (local.get $i) (local.get $len)))
        (i64.store
          (i32.add (local.get $out) (i32.mul (local.get $i) (i32.const 8)))
          (i64.load
            (i32.add (local.get $ptr) (i32.mul (local.get $i) (i32.const 12)))))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $next)))
    (local.set $ret (call $realloc (i32.const 0) (i32.const 0) (i32.const 4) (i32.const 8)))
    (i32.store (local.get $ret) (local.get $out))
    (i32.store offset=4 (local.get $ret) (local.get $len))
    (local.get $ret))

  ;; `make` and `identity` return the entries they received: a list of
  ;; `tuple<string, u32>` and a `map<string, u32>` have the same layout.
  (func $passthrough (param $ptr i32) (param $len i32) (result i32)
    (local $ret i32)
    (local.set $ret (call $realloc (i32.const 0) (i32.const 0) (i32.const 4) (i32.const 8)))
    (i32.store (local.get $ret) (local.get $ptr))
    (i32.store offset=4 (local.get $ret) (local.get $len))
    (local.get $ret))
  (export "make" (func $passthrough))
  (export "identity" (func $passthrough)))
