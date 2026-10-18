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
//! copied for them, and some boundary time. The thread's counters add
//! up every store's work. Each store's own counters count only its
//! work, with a breakdown by host import whose parts add up to the
//! store's total.

#![cfg(all(test, feature = "boundary-counters"))]

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};

use wcmp::{
    Accessor, BoundaryCounters, Component, Engine, HostCall, Instance, Linker, Result, Store,
    StoreBoundaryCounters, Val,
};
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

/// A guest whose `run` calls the host `async` function `answer` through
/// an asynchronous lower, waits on the subtask, and hands the result back
/// from its callback. The result crosses into the guest's return area, 4
/// bytes, in the turn after the host's future completes.
const AN_ASYNC_IMPORT: &[u8] = component!(
    r#"
    (component
      (import "answer" (func $answer async (param "x" u32) (result u32)))
      (core module $libc (memory (export "memory") 1))
      (core instance $libc (instantiate $libc))
      (core func $lowered
        (canon lower (func $answer) async (memory (core memory $libc "memory"))))
      (core func $task-return (canon task.return (result u32)))
      (core func $set-new (canon waitable-set.new))
      (core func $join (canon waitable.join))
      (core module $m
        (import "libc" "memory" (memory 1))
        (import "" "answer" (func $answer (param i32 i32) (result i32)))
        (import "" "task.return" (func $task-return (param i32)))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (import "" "waitable.join" (func $join (param i32 i32)))
        (func (export "run") (param i32) (result i32)
          (local $status i32)
          (local $set i32)
          (local.set $status (call $answer (local.get 0) (i32.const 0)))
          (if (i32.eq (i32.and (local.get $status) (i32.const 0xf)) (i32.const 2))
            (then
              (call $task-return (i32.load (i32.const 0)))
              (return (i32.const 0))))
          (local.set $set (call $set-new))
          (call $join
            (i32.shr_u (local.get $status) (i32.const 4))
            (local.get $set))
          (i32.or (i32.shl (local.get $set) (i32.const 4)) (i32.const 2)))
        (func (export "run-callback") (param i32 i32 i32) (result i32)
          (call $task-return (i32.load (i32.const 0)))
          (i32.const 0)))
      (core instance $m (instantiate $m
        (with "libc" (instance $libc))
        (with "" (instance
          (export "answer" (func $lowered))
          (export "task.return" (func $task-return))
          (export "waitable-set.new" (func $set-new))
          (export "waitable.join" (func $join))))))
      (func (export "run") async (param "x" u32) (result u32)
        (canon lift (core func $m "run") async
          (callback (core func $m "run-callback")))))
    "#
);

/// A guest whose export `length` takes a list of bytes, which the host's
/// call lowers into the guest's memory, and answers its length.
const AN_EXPORT_OF_A_LIST: &[u8] = component!(
    r#"
    (component
      (core module $m
        (memory (export "memory") 1)
        (global $bump (mut i32) (i32.const 1024))
        (func (export "cabi_realloc")
              (param $old i32) (param $old-size i32) (param $align i32) (param $size i32)
              (result i32)
          (local $ptr i32)
          global.get $bump local.set $ptr
          local.get $ptr local.get $size i32.add global.set $bump
          local.get $ptr)
        (func (export "length") (param i32 i32) (result i32)
          local.get 1))
      (core instance $i (instantiate $m))
      (func (export "length") (param "bytes" (list u8)) (result u32)
        (canon lift (core func $i "length")
                    (memory (core memory $i "memory"))
                    (realloc (core func $i "cabi_realloc")))))
    "#
);

/// A future that is pending once, waking its waker, and then answers
/// with `value`, so the call's result crosses in a later turn.
struct PendingOnce {
    polled: bool,
    value: u32,
}

impl Future for PendingOnce {
    type Output = Result<(u32,)>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if this.polled {
            return Poll::Ready(Ok((this.value,)));
        }
        this.polled = true;
        context.waker().wake_by_ref();
        Poll::Pending
    }
}

/// Instantiate [`COMPONENT`] in a fresh store of `engine`, with `sum`
/// doing enough work that its host time is more than nothing on a
/// coarse clock.
async fn known_sequence(engine: &Engine) -> (Store<()>, Instance) {
    let component = Component::new(engine, COMPONENT)
        .await
        .expect("component parses");
    let mut store: Store<()> = Store::new(engine, ()).expect("store");
    let mut linker: Linker<()> = Linker::new(engine);
    linker
        .root()
        .func_wrap(
            "sum",
            |_: HostCall<'_, ()>, (bytes,): (Vec<u8>,)| -> Result<(u32,)> {
                // Enough work that the host time is more than nothing on
                // a coarse clock.
                let mut sum = 0u32;
                for _ in 0..2_000 {
                    sum = bytes.iter().fold(std::hint::black_box(sum), |sum, byte| {
                        sum.wrapping_add(u32::from(*byte))
                    });
                }
                Ok((sum,))
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
    (store, instance)
}

/// Run [`COMPONENT`]'s `run` `runs` times.
async fn run(store: &mut Store<()>, instance: &Instance, runs: u64) {
    let run = instance.get_func("run").expect("run export");
    for _ in 0..runs {
        run.call(&mut *store, &[]).await.expect("run");
    }
}

/// The bytes one call of `sum` copies: it lifts its 64 bytes.
const SUM_BYTES: u64 = 64;

/// The bytes one call of `fill` copies: it writes its 32 bytes and then
/// the pointer and the length of the list, 8 bytes, into the return
/// area.
const FILL_BYTES: u64 = 32 + 8;

/// Whether two sums of the same times agree, up to the rounding of
/// adding them in another order.
fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

/// Assert that `counted`'s imports and `other` add up to its total.
fn assert_adds_up(counted: &StoreBoundaryCounters) {
    let mut sum = counted.other;
    for counts in counted.imports.values() {
        sum.calls += counts.calls;
        sum.bytes += counts.bytes;
        sum.boundary_ms += counts.boundary_ms;
        sum.host_ms += counts.host_ms;
    }
    assert_eq!(sum.calls, counted.total.calls, "{counted:?}");
    assert_eq!(sum.bytes, counted.total.bytes, "{counted:?}");
    assert!(
        close(sum.boundary_ms, counted.total.boundary_ms),
        "{counted:?}"
    );
    assert!(close(sum.host_ms, counted.total.host_ms), "{counted:?}");
}

#[wcmp_macros::test]
async fn it_counts_the_calls_the_bytes_and_the_time_of_a_known_sequence() {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let (mut store, instance) = known_sequence(&engine).await;

    // The sequence runs many times, so that the boundary time adds up
    // to more than nothing even where the clock is coarse, as a
    // browser's `performance.now()` can be.
    const RUNS: u64 = 50;
    BoundaryCounters::take();
    run(&mut store, &instance, RUNS).await;
    let counted = BoundaryCounters::take();

    assert_eq!(
        counted.calls,
        RUNS * 4,
        "three calls of sum and one of fill"
    );
    assert_eq!(counted.bytes, RUNS * (3 * SUM_BYTES + FILL_BYTES));
    assert!(
        counted.boundary_ms > 0.0,
        "the polyfill spent time at the boundary: {counted:?}"
    );
    assert!(
        counted.host_ms > 0.0,
        "the host functions spent time: {counted:?}"
    );
    assert_eq!(
        BoundaryCounters::read(),
        BoundaryCounters::default(),
        "taking the counters resets them"
    );
}

#[wcmp_macros::test]
async fn it_counts_each_store_on_a_thread_apart() {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let (mut first, first_instance) = known_sequence(&engine).await;
    let (mut second, second_instance) = known_sequence(&engine).await;
    first.take_boundary_counters();
    second.take_boundary_counters();
    BoundaryCounters::take();

    // The two stores' calls interleave on the one thread.
    for _ in 0..5 {
        run(&mut first, &first_instance, 2).await;
        run(&mut second, &second_instance, 1).await;
    }
    let thread = BoundaryCounters::take();
    let first_counted = first.take_boundary_counters().total;
    let second_counted = second.take_boundary_counters().total;

    assert_eq!(first_counted.calls, 10 * 4, "{first_counted:?}");
    assert_eq!(second_counted.calls, 5 * 4, "{second_counted:?}");
    assert_eq!(
        first_counted.bytes,
        10 * (3 * SUM_BYTES + FILL_BYTES),
        "{first_counted:?}"
    );
    assert_eq!(
        second_counted.bytes,
        5 * (3 * SUM_BYTES + FILL_BYTES),
        "{second_counted:?}"
    );
    assert_eq!(
        thread.calls,
        first_counted.calls + second_counted.calls,
        "the thread's counters are the two stores' together"
    );
    assert_eq!(thread.bytes, first_counted.bytes + second_counted.bytes);
    assert!(
        close(
            thread.boundary_ms,
            first_counted.boundary_ms + second_counted.boundary_ms
        ),
        "every moment of boundary time went to one store: {thread:?}, \
         {first_counted:?}, {second_counted:?}"
    );
    assert!(
        close(
            thread.host_ms,
            first_counted.host_ms + second_counted.host_ms
        ),
        "every moment of host time went to one store: {thread:?}, \
         {first_counted:?}, {second_counted:?}"
    );
    assert_eq!(
        first.boundary_counters(),
        StoreBoundaryCounters::default(),
        "taking a store's counters resets them"
    );
}

#[wcmp_macros::test]
async fn it_breaks_a_stores_counts_down_by_host_import() {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let (mut store, instance) = known_sequence(&engine).await;
    store.take_boundary_counters();

    const RUNS: u64 = 20;
    run(&mut store, &instance, RUNS).await;
    let counted = store.take_boundary_counters();

    assert_eq!(
        counted.imports.keys().collect::<Vec<_>>(),
        ["fill", "sum"],
        "{counted:?}"
    );
    let sum = counted.imports["sum"];
    let fill = counted.imports["fill"];
    assert_eq!(sum.calls, 3 * RUNS);
    assert_eq!(sum.bytes, 3 * RUNS * SUM_BYTES);
    assert_eq!(fill.calls, RUNS);
    assert_eq!(fill.bytes, RUNS * FILL_BYTES);
    assert!(
        sum.host_ms > 0.0,
        "`sum`'s own work is its host time: {counted:?}"
    );
    assert_eq!(
        counted.other.calls, 0,
        "every call was an import's: {counted:?}"
    );
    assert_adds_up(&counted);
}

#[wcmp_macros::test]
async fn it_counts_an_async_imports_later_polls_and_its_result_under_it() {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, AN_ASYNC_IMPORT)
        .await
        .expect("component parses");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap_concurrent("answer", |_: &Accessor<()>, (x,): (u32,)| PendingOnce {
            polled: false,
            value: x * 2,
        })
        .expect("answer registers");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let run = instance.get_func("run").expect("run export");
    store.take_boundary_counters();

    const RUNS: u64 = 10;
    for _ in 0..RUNS {
        let result = run.call(&mut store, &[Val::U32(21)]).await.expect("run");
        assert_eq!(result.first(), Some(&Val::U32(42)));
    }
    let counted = store.take_boundary_counters();

    let answer = counted.imports["answer"];
    assert_eq!(answer.calls, RUNS, "{counted:?}");
    assert_eq!(
        answer.bytes,
        RUNS * 4,
        "the result crossed in a later turn, and counted under the import: \
         {counted:?}"
    );
    assert_eq!(counted.other.bytes, 0, "{counted:?}");
    assert_adds_up(&counted);
}

#[wcmp_macros::test]
async fn it_counts_the_values_of_the_hosts_calls_into_exports_as_other() {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, AN_EXPORT_OF_A_LIST)
        .await
        .expect("component parses");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let linker: Linker<()> = Linker::new(&engine);
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let length = instance.get_func("length").expect("length export");
    let bytes = Val::List((0..16).map(Val::U8).collect());
    store.take_boundary_counters();
    BoundaryCounters::take();

    const RUNS: u64 = 10;
    for _ in 0..RUNS {
        let result = length
            .call(&mut store, std::slice::from_ref(&bytes))
            .await
            .expect("length");
        assert_eq!(result.first(), Some(&Val::U32(16)));
    }
    let thread = BoundaryCounters::take();
    let counted = store.take_boundary_counters();

    assert!(counted.imports.is_empty(), "{counted:?}");
    assert_eq!(
        counted.other.bytes,
        RUNS * 16,
        "the list crossed into the guest's memory, under the store's other \
         work: {counted:?}"
    );
    assert_eq!(
        (thread.calls, thread.bytes),
        (counted.total.calls, counted.total.bytes),
        "nothing the thread counted escaped the store: {thread:?}, {counted:?}"
    );
    assert_adds_up(&counted);
}

#[path = "support/backend.rs"]
mod test_backend;
