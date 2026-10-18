// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Baseline tests for the boundary counters, which count only with the
//! `boundary-counters` feature.
//!
//! A guest makes a known sequence of calls into the host, and the
//! counters must give the number of calls, the bytes the polyfill
//! copied for them, and some boundary time.

#![cfg(all(test, feature = "boundary-counters"))]

use wcmp::{BoundaryCounters, Component, Engine, HostCall, Linker, Result, Store};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// A guest whose `run` calls `sum` three times with the 64 bytes at 256,
/// and then `fill`, whose 32 bytes come back through a return area.
const COMPONENT: &[u8] = component!(
    r#"
    (component
      (import "sum" (func $sum (param "bytes" (list u8)) (result u32)))
      (import "fill" (func $fill (result (list u8))))
      (core module $shim
        (table (export "$imports") 2 2 funcref)
        (func (export "sum") (param i32 i32) (result i32)
          local.get 0 local.get 1
          i32.const 0
          call_indirect (param i32 i32) (result i32))
        (func (export "fill") (param i32)
          local.get 0
          i32.const 1
          call_indirect (param i32)))
      (core instance $shim (instantiate $shim))
      (core module $m
        (import "" "sum" (func $sum (param i32 i32) (result i32)))
        (import "" "fill" (func $fill (param i32)))
        (memory (export "memory") 1)
        (data (i32.const 256) "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        (global $bump (mut i32) (i32.const 1024))
        (func $realloc (export "cabi_realloc")
              (param $old i32) (param $old-size i32) (param $align i32) (param $size i32)
              (result i32)
          (local $ptr i32)
          global.get $bump local.get $align i32.add i32.const 1 i32.sub
          local.get $align i32.const 1 i32.sub i32.const -1 i32.xor i32.and
          local.set $ptr
          local.get $ptr local.get $size i32.add global.set $bump
          local.get $ptr)
        (func (export "run")
          i32.const 256 i32.const 64 call $sum drop
          i32.const 256 i32.const 64 call $sum drop
          i32.const 256 i32.const 64 call $sum drop
          i32.const 16 call $fill))
      (core instance $i (instantiate $m
        (with "" (instance
          (export "sum" (func $shim "sum"))
          (export "fill" (func $shim "fill"))))))
      (core func $core-sum
        (canon lower (func $sum) (memory (core memory $i "memory"))))
      (core func $core-fill
        (canon lower (func $fill)
                    (memory (core memory $i "memory"))
                    (realloc (core func $i "cabi_realloc"))))
      (core module $fixups
        (import "" "sum" (func (param i32 i32) (result i32)))
        (import "" "fill" (func (param i32)))
        (import "" "$imports" (table 2 2 funcref))
        (elem (i32.const 0) func 0 1))
      (core instance (instantiate $fixups
        (with "" (instance
          (export "sum" (func $core-sum))
          (export "fill" (func $core-fill))
          (export "$imports" (table $shim "$imports"))))))
      (func (export "run")
        (canon lift (core func $i "run"))))
    "#
);

#[wcmp_macros::test]
async fn it_counts_the_calls_the_bytes_and_the_time_of_a_known_sequence() {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap(
            "sum",
            |_: HostCall<'_, ()>, (bytes,): (Vec<u8>,)| -> Result<(u32,)> {
                Ok((bytes.iter().map(|byte| u32::from(*byte)).sum(),))
            },
        )
        .expect("sum registers");
    linker
        .root()
        .func_wrap(
            "fill",
            |_: HostCall<'_, ()>, (): ()| -> Result<(Vec<u8>,)> { Ok((vec![7; 32],)) },
        )
        .expect("fill registers");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let run = instance.get_func("run").expect("run export");

    // The sequence runs many times, so that the boundary time adds up
    // to more than nothing even where the clock is coarse, as a
    // browser's `performance.now()` can be.
    const RUNS: u64 = 50;
    BoundaryCounters::take();
    for _ in 0..RUNS {
        run.call(&mut store, &[]).await.expect("run");
    }
    let counted = BoundaryCounters::take();

    assert_eq!(
        counted.calls,
        RUNS * 4,
        "three calls of sum and one of fill"
    );
    // Each `sum` lifts its 64 bytes, and `fill` writes its 32 bytes
    // and then the pointer and the length of the list, 8 bytes, into
    // the return area.
    assert_eq!(counted.bytes, RUNS * (3 * 64 + 32 + 8));
    assert!(
        counted.boundary_ms > 0.0,
        "the polyfill spent time at the boundary: {counted:?}"
    );
    assert_eq!(
        BoundaryCounters::read(),
        BoundaryCounters::default(),
        "taking the counters resets them"
    );
}

#[path = "support/backend.rs"]
mod test_backend;
