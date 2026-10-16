// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Baseline tests for the typed value traits: `ComponentType`, `Lift`,
//! `Lower`, and `ComponentNamedList`.
//!
//! A typed call and a typed host function take their parameters as one
//! tuple and return their results as one tuple, and their values cross
//! the boundary directly, building no `Val` at any depth. The file
//! installs a counting global allocator, so the tests can assert that a
//! crossing costs the host no allocation per element beyond the values
//! themselves. The count is kept per thread and only while a test asks
//! for it.
//!
//! The type check runs when the host acquires a typed function and when
//! the linker links a typed host function, and a mismatch names the path
//! to the first part that differs.

#![cfg(test)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use wcmp::{
    Component, Engine, Error, HostCall, Instance, InterfaceIdentifier, Linker, PrimitiveType,
    Store, TypeMismatchPosition, TypePathStep, TypeRendering, TypedFunc, Val, ValueType,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

thread_local! {
    /// Whether this thread's allocations are being counted.
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    /// How many allocations this thread has made while counting.
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

/// A global allocator that counts the allocations of the thread under
/// measurement and passes everything through to the system's.
struct CountingAllocator;

/// Count one allocation, when this thread is counting.
fn tally() {
    if COUNTING.try_with(Cell::get).unwrap_or(false) {
        let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
    }
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        tally();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        tally();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        tally();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Count the allocations `future` makes on this thread until it
/// completes, and answer them with what it produced.
async fn counted<O>(future: impl core::future::Future<Output = O>) -> (usize, O) {
    ALLOCATIONS.with(|count| count.set(0));
    COUNTING.with(|counting| counting.set(true));
    let output = future.await;
    COUNTING.with(|counting| counting.set(false));
    (ALLOCATIONS.with(Cell::get), output)
}

/// The interface the guest of [`TYPED`] imports.
const HOST: &str = "wcmp-tests:typed/host@0.1.0";

/// A guest whose exports and imports take typed values.
///
/// Its allocator bumps and grows the memory as a payload needs, and
/// `reset` puts the bump back at the start, so a later call reuses the
/// memory an earlier one grew and the measured call grows nothing.
///
/// - `echo-strings` returns the list of strings it receives.
/// - `count-strings` takes a list of strings and returns its length.
/// - `forward` hands the list it receives to the host's `echo` and
///   returns what the host answers.
/// - `echo-async` is lifted `async` and returns the list it receives
///   through `task.return`.
/// - `sum17` takes seventeen `u32`s, past the sixteen that travel as
///   core parameters, so its caller spills them to memory, and returns
///   their sum.
/// - `call-sum17` calls the host's `sum17` with 1 to 17 and returns
///   what the host answers.
/// - `bytes-u16` declares a list of `u16` and traps if it is ever
///   called.
/// - `echo-pairs`, `forward-pairs`, and `echo-pairs-async` do what
///   `echo-strings`, `forward`, and `echo-async` do, over a list of
///   `tuple<u32, u32>`, whose elements point nowhere. The host's
///   `echo-pairs` returns the list it receives.
/// - `echo-bools` and each `echo-…` export after it return the list of
///   its element type it receives.
/// - `peek` reads a word of memory.
const TYPED: &[u8] = component!(
    r#"
    (component
      (core module $libc
        (memory (export "memory") 1)
        (global $bump (mut i32) (i32.const 16))
        (func (export "cabi_realloc")
              (param $old i32) (param $old-size i32) (param $align i32) (param $size i32)
              (result i32)
          (local $ptr i32) (local $end i32) (local $have i32)
          global.get $bump local.get $align i32.add i32.const 1 i32.sub
          local.get $align i32.const 1 i32.sub i32.const -1 i32.xor i32.and
          local.set $ptr
          local.get $ptr local.get $size i32.add local.set $end
          memory.size i32.const 16 i32.shl local.set $have
          (if (i32.gt_u (local.get $end) (local.get $have))
            (then
              (if (i32.eq
                    (memory.grow
                      (i32.shr_u
                        (i32.add (i32.sub (local.get $end) (local.get $have)) (i32.const 65535))
                        (i32.const 16)))
                    (i32.const -1))
                (then unreachable))))
          local.get $end global.set $bump
          local.get $ptr)
        (func (export "reset") i32.const 16 global.set $bump)
        (func (export "peek") (param i32) (result i32) (i32.load (local.get 0))))
      (core instance $libc (instantiate $libc))
      (core func $task-return
        (canon task.return (result (list string)) (memory (core memory $libc "memory"))))
      (core func $task-return-pairs
        (canon task.return (result (list (tuple u32 u32))) (memory (core memory $libc "memory"))))
      (import "wcmp-tests:typed/host@0.1.0" (instance $host
        (export "echo" (func (param "xs" (list string)) (result (list string))))
        (export "sum17" (func
          (param "a0" u32) (param "a1" u32) (param "a2" u32) (param "a3" u32)
          (param "a4" u32) (param "a5" u32) (param "a6" u32) (param "a7" u32)
          (param "a8" u32) (param "a9" u32) (param "a10" u32) (param "a11" u32)
          (param "a12" u32) (param "a13" u32) (param "a14" u32) (param "a15" u32)
          (param "a16" u32) (result u32)))
        (export "echo-pairs" (func (param "xs" (list (tuple u32 u32)))
          (result (list (tuple u32 u32)))))))
      (alias export $host "echo" (func $echo))
      (alias export $host "echo-pairs" (func $echo-pairs))
      (alias export $host "sum17" (func $sum17))
      (core func $echo-lower (canon lower (func $echo)
        (memory (core memory $libc "memory"))
        (realloc (core func $libc "cabi_realloc"))))
      (core func $sum17-lower (canon lower (func $sum17)
        (memory (core memory $libc "memory"))))
      (core func $echo-pairs-lower (canon lower (func $echo-pairs)
        (memory (core memory $libc "memory"))
        (realloc (core func $libc "cabi_realloc"))))
      (core module $m
        (import "libc" "memory" (memory 1))
        (import "libc" "cabi_realloc" (func $realloc (param i32 i32 i32 i32) (result i32)))
        (import "" "task.return" (func $task-return (param i32 i32)))
        (import "host" "echo" (func $echo (param i32 i32 i32)))
        (import "host" "sum17" (func $sum17 (param i32) (result i32)))
        (import "" "task.return-pairs" (func $task-return-pairs (param i32 i32)))
        (import "host" "echo-pairs" (func $echo-pairs (param i32 i32 i32)))
        (func $pair (param $ptr i32) (param $len i32) (result i32)
          (local $ret i32)
          (local.set $ret (call $realloc (i32.const 0) (i32.const 0) (i32.const 4) (i32.const 8)))
          (i32.store (local.get $ret) (local.get $ptr))
          (i32.store offset=4 (local.get $ret) (local.get $len))
          (local.get $ret))
        (func (export "echo") (param $ptr i32) (param $len i32) (result i32)
          (call $pair (local.get $ptr) (local.get $len)))
        (func (export "count") (param $ptr i32) (param $len i32) (result i32)
          (local.get $len))
        (func (export "forward") (param $ptr i32) (param $len i32) (result i32)
          (local $ret i32)
          (local.set $ret (call $realloc (i32.const 0) (i32.const 0) (i32.const 4) (i32.const 8)))
          (call $echo (local.get $ptr) (local.get $len) (local.get $ret))
          (local.get $ret))
        (func (export "echo-async") (param $ptr i32) (param $len i32) (result i32)
          (call $task-return (local.get $ptr) (local.get $len))
          (i32.const 0))
        (func (export "echo-async-callback") (param i32 i32 i32) (result i32)
          unreachable)
        (func (export "forward-pairs") (param $ptr i32) (param $len i32) (result i32)
          (local $ret i32)
          (local.set $ret (call $realloc (i32.const 0) (i32.const 0) (i32.const 4) (i32.const 8)))
          (call $echo-pairs (local.get $ptr) (local.get $len) (local.get $ret))
          (local.get $ret))
        (func (export "echo-pairs-async") (param $ptr i32) (param $len i32) (result i32)
          (call $task-return-pairs (local.get $ptr) (local.get $len))
          (i32.const 0))
        (func (export "sum17") (param $p i32) (result i32)
          (local $i i32) (local $acc i32)
          (block $done
            (loop $next
              (br_if $done (i32.ge_u (local.get $i) (i32.const 17)))
              (local.set $acc
                (i32.add (local.get $acc)
                  (i32.load (i32.add (local.get $p) (i32.shl (local.get $i) (i32.const 2))))))
              (local.set $i (i32.add (local.get $i) (i32.const 1)))
              (br $next)))
          (local.get $acc))
        (func (export "call-sum17") (result i32)
          (local $p i32) (local $i i32)
          (local.set $p (call $realloc (i32.const 0) (i32.const 0) (i32.const 4) (i32.const 68)))
          (block $done
            (loop $next
              (br_if $done (i32.ge_u (local.get $i) (i32.const 17)))
              (i32.store
                (i32.add (local.get $p) (i32.shl (local.get $i) (i32.const 2)))
                (i32.add (local.get $i) (i32.const 1)))
              (local.set $i (i32.add (local.get $i) (i32.const 1)))
              (br $next)))
          (call $sum17 (local.get $p)))
        (func (export "never") (result i32) unreachable))
      (core instance $i (instantiate $m
        (with "libc" (instance $libc))
        (with "" (instance
          (export "task.return" (func $task-return))
          (export "task.return-pairs" (func $task-return-pairs))))
        (with "host" (instance
          (export "echo" (func $echo-lower))
          (export "sum17" (func $sum17-lower))
          (export "echo-pairs" (func $echo-pairs-lower))))))
      (func (export "echo-strings") (param "xs" (list string)) (result (list string))
        (canon lift (core func $i "echo")
          (memory (core memory $libc "memory"))
          (realloc (core func $libc "cabi_realloc"))))
      (func (export "count-strings") (param "xs" (list string)) (result u32)
        (canon lift (core func $i "count")
          (memory (core memory $libc "memory"))
          (realloc (core func $libc "cabi_realloc"))))
      (func (export "forward") (param "xs" (list string)) (result (list string))
        (canon lift (core func $i "forward")
          (memory (core memory $libc "memory"))
          (realloc (core func $libc "cabi_realloc"))))
      (func (export "echo-async") async (param "xs" (list string)) (result (list string))
        (canon lift (core func $i "echo-async") async
          (memory (core memory $libc "memory"))
          (realloc (core func $libc "cabi_realloc"))
          (callback (core func $i "echo-async-callback"))))
      (func (export "sum17")
        (param "a0" u32) (param "a1" u32) (param "a2" u32) (param "a3" u32)
        (param "a4" u32) (param "a5" u32) (param "a6" u32) (param "a7" u32)
        (param "a8" u32) (param "a9" u32) (param "a10" u32) (param "a11" u32)
        (param "a12" u32) (param "a13" u32) (param "a14" u32) (param "a15" u32)
        (param "a16" u32) (result u32)
        (canon lift (core func $i "sum17")
          (memory (core memory $libc "memory"))
          (realloc (core func $libc "cabi_realloc"))))
      (func (export "call-sum17") (result u32) (canon lift (core func $i "call-sum17")))
      (func (export "bytes-u16") (result (list u16))
        (canon lift (core func $i "never") (memory (core memory $libc "memory"))))
      (func (export "echo-pairs") (param "xs" (list (tuple u32 u32)))
        (result (list (tuple u32 u32)))
        (canon lift (core func $i "echo")
          (memory (core memory $libc "memory"))
          (realloc (core func $libc "cabi_realloc"))))
      (func (export "forward-pairs") (param "xs" (list (tuple u32 u32)))
        (result (list (tuple u32 u32)))
        (canon lift (core func $i "forward-pairs")
          (memory (core memory $libc "memory"))
          (realloc (core func $libc "cabi_realloc"))))
      (func (export "echo-pairs-async") async (param "xs" (list (tuple u32 u32)))
        (result (list (tuple u32 u32)))
        (canon lift (core func $i "echo-pairs-async") async
          (memory (core memory $libc "memory"))
          (realloc (core func $libc "cabi_realloc"))
          (callback (core func $i "echo-async-callback"))))
      (func (export "echo-bools") (param "xs" (list bool)) (result (list bool))
        (canon lift (core func $i "echo")
          (memory (core memory $libc "memory"))
          (realloc (core func $libc "cabi_realloc"))))
      (func (export "echo-s8s") (param "xs" (list s8)) (result (list s8))
        (canon lift (core func $i "echo")
          (memory (core memory $libc "memory"))
          (realloc (core func $libc "cabi_realloc"))))
      (func (export "echo-s16s") (param "xs" (list s16)) (result (list s16))
        (canon lift (core func $i "echo")
          (memory (core memory $libc "memory"))
          (realloc (core func $libc "cabi_realloc"))))
      (func (export "echo-s32s") (param "xs" (list s32)) (result (list s32))
        (canon lift (core func $i "echo")
          (memory (core memory $libc "memory"))
          (realloc (core func $libc "cabi_realloc"))))
      (func (export "echo-s64s") (param "xs" (list s64)) (result (list s64))
        (canon lift (core func $i "echo")
          (memory (core memory $libc "memory"))
          (realloc (core func $libc "cabi_realloc"))))
      (func (export "echo-u16s") (param "xs" (list u16)) (result (list u16))
        (canon lift (core func $i "echo")
          (memory (core memory $libc "memory"))
          (realloc (core func $libc "cabi_realloc"))))
      (func (export "echo-u64s") (param "xs" (list u64)) (result (list u64))
        (canon lift (core func $i "echo")
          (memory (core memory $libc "memory"))
          (realloc (core func $libc "cabi_realloc"))))
      (func (export "echo-f32s") (param "xs" (list f32)) (result (list f32))
        (canon lift (core func $i "echo")
          (memory (core memory $libc "memory"))
          (realloc (core func $libc "cabi_realloc"))))
      (func (export "echo-f64s") (param "xs" (list f64)) (result (list f64))
        (canon lift (core func $i "echo")
          (memory (core memory $libc "memory"))
          (realloc (core func $libc "cabi_realloc"))))
      (func (export "echo-chars") (param "xs" (list char)) (result (list char))
        (canon lift (core func $i "echo")
          (memory (core memory $libc "memory"))
          (realloc (core func $libc "cabi_realloc"))))
      (func (export "peek") (param "p" u32) (result u32) (canon lift (core func $libc "peek")))
      (func (export "reset") (canon lift (core func $libc "reset"))))
    "#
);

/// The seventeen `u32`s `sum17` takes.
type Seventeen = (
    u32,
    u32,
    u32,
    u32,
    u32,
    u32,
    u32,
    u32,
    u32,
    u32,
    u32,
    u32,
    u32,
    u32,
    u32,
    u32,
    u32,
);

/// Instantiate [`TYPED`] with typed host functions for its imports.
async fn typed_instance() -> (Store<()>, Instance) {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, TYPED)
        .await
        .expect("the component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let host: InterfaceIdentifier = HOST.parse().expect("identifier");
    linker
        .instance(&host)
        .func_wrap("echo", |_call: HostCall<'_, ()>, (xs,): (Vec<String>,)| {
            Ok((xs,))
        })
        .expect("the registration of `echo`");
    linker
        .instance(&host)
        .func_wrap(
            "sum17",
            |_call: HostCall<'_, ()>,
             (a, b, c, d, e, f, g, h, i, j, k, l, m, n, o, p, q): Seventeen| {
                Ok((a + b + c + d + e + f + g + h + i + j + k + l + m + n + o + p + q,))
            },
        )
        .expect("the registration of `sum17`");
    linker
        .instance(&host)
        .func_wrap(
            "echo-pairs",
            |_call: HostCall<'_, ()>, (xs,): (Vec<(u32, u32)>,)| Ok((xs,)),
        )
        .expect("the registration of `echo-pairs`");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the component links to the typed host functions");
    (store, instance)
}

/// The typed handle of the export `name`.
fn typed<P, R>(instance: &Instance, name: &str) -> TypedFunc<P, R>
where
    P: wcmp::ComponentNamedList + wcmp::Lower,
    R: wcmp::ComponentNamedList + wcmp::Lift,
{
    instance
        .get_func(name)
        .expect("the export")
        .typed::<P, R>()
        .expect("the export has the typed signature")
}

/// Put the guest's bump back at the start.
async fn reset(store: &mut Store<()>, instance: &Instance) {
    typed::<(), ()>(instance, "reset")
        .call(&mut *store, ())
        .await
        .expect("the reset runs");
}

/// `count` short strings.
fn strings(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("s{index}")).collect()
}

/// How many allocations one call of `name`, a list of strings in and
/// whatever `R` is out, makes for 16 strings and for 4,096, after a
/// warm-up call that grows the guest's memory for the larger list.
async fn allocations_for<R>(name: &str) -> (usize, usize)
where
    R: wcmp::ComponentNamedList + wcmp::Lift + 'static,
{
    let (mut store, instance) = typed_instance().await;
    let call = typed::<(Vec<String>,), R>(&instance, name);
    let run = async |store: &mut Store<()>, count: usize| {
        let (allocations, output) = counted(call.call(&mut *store, (strings(count),))).await;
        output.expect("the call returns");
        allocations
    };
    run(&mut store, 4096).await;
    reset(&mut store, &instance).await;
    let small = run(&mut store, 16).await;
    reset(&mut store, &instance).await;
    let large = run(&mut store, 4096).await;
    (small, large)
}

/// How many more allocations a call makes for 4,096 strings than for
/// 16, for a call of `name` with a list of strings in and `R` out.
async fn growth<R>(name: &str) -> usize
where
    R: wcmp::ComponentNamedList + wcmp::Lift + 'static,
{
    let (small, large) = allocations_for::<R>(name).await;
    large - small
}

/// The strings the larger call carries beyond the smaller.
const MORE: usize = 4096 - 16;

/// What lowering [`MORE`] strings costs the host. Each lowered string
/// is written straight from the host's string, into memory the guest's
/// `cabi_realloc` gives, and the Canonical ABI runs each such call as a
/// task of its own, whose records the host allocates whichever path
/// lowers the string. No `Val` is part of it.
async fn lowering() -> usize {
    let lowered = growth::<(u32,)>("count-strings").await;
    assert_eq!(
        lowered % MORE,
        0,
        "lowering costs the same for each string: {lowered} for {MORE}"
    );
    lowered
}

#[wcmp_macros::test]
async fn it_passes_seventeen_values_to_a_typed_export_and_a_typed_host_function() {
    let (mut store, instance) = typed_instance().await;
    let (sum,) = typed::<Seventeen, (u32,)>(&instance, "sum17")
        .call(
            &mut store,
            (1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17),
        )
        .await
        .expect("the typed export takes seventeen values");
    assert_eq!(sum, 153);

    let (sum,) = typed::<(), (u32,)>(&instance, "call-sum17")
        .call(&mut store, ())
        .await
        .expect("the guest calls the typed host function with seventeen values");
    assert_eq!(
        sum, 153,
        "the host function's result tuple reached the guest"
    );
}

#[wcmp_macros::test]
async fn it_builds_no_val_for_a_typed_call_of_a_list_of_strings() {
    // A lifted string is one allocation, the buffer it is read into.
    // Nothing else of the call grows with the list but the lowering
    // of each string.
    let lowered = lowering().await;
    let grew = growth::<(Vec<String>,)>("echo-strings").await;
    assert_eq!(
        grew,
        lowered + MORE,
        "one allocation per string lifted beyond the lowering, and nothing per element besides"
    );
}

#[wcmp_macros::test]
async fn it_builds_no_val_for_a_typed_host_function_over_a_list_of_strings() {
    // The strings cross four times: into the guest, out to the host
    // function, back into the guest, and out to the caller. Two of
    // those crossings lift them.
    let lowered = lowering().await;
    let grew = growth::<(Vec<String>,)>("forward").await;
    assert_eq!(
        grew,
        2 * (lowered + MORE),
        "one allocation per string lifted beyond the two lowerings, and nothing per element \
         besides"
    );
}

#[wcmp_macros::test]
async fn it_builds_no_val_for_the_result_of_an_export_lifted_async() {
    // The result crosses through `task.return`, which lifts it straight
    // into the typed call's tuple.
    let lowered = lowering().await;
    let grew = growth::<(Vec<String>,)>("echo-async").await;
    assert_eq!(
        grew,
        lowered + MORE,
        "one allocation per string lifted beyond the lowering, and nothing per element besides"
    );
}

/// `count` pairs of numbers.
fn pairs(count: u32) -> Vec<(u32, u32)> {
    (0..count).map(|x| (x, !x)).collect()
}

/// How many allocations one typed call of `name`, a list of pairs in
/// and the same list out, makes for 16 pairs and for 4,096, after a
/// warm-up call that grows the guest's memory for the larger list.
async fn pair_allocations(name: &str) -> (usize, usize) {
    let (mut store, instance) = typed_instance().await;
    let call = typed::<(Vec<(u32, u32)>,), (Vec<(u32, u32)>,)>(&instance, name);
    let run = async |store: &mut Store<()>, count: u32| {
        let (allocations, output) = counted(call.call(&mut *store, (pairs(count),))).await;
        assert_eq!(
            output.expect("the call returns").0,
            pairs(count),
            "the list came back as it went"
        );
        allocations
    };
    run(&mut store, 4096).await;
    reset(&mut store, &instance).await;
    let small = run(&mut store, 16).await;
    reset(&mut store, &instance).await;
    let large = run(&mut store, 4096).await;
    (small, large)
}

// A pair points nowhere, so a typed crossing of a list of pairs is one
// read or one write of the whole list, and the result list is one
// allocation at either length: the count is the same for 16 pairs as
// for 4,096. The `Val` form of the same list allocates once for each
// pair, the boxed slice of its `Val::Tuple`, so a `Val` built for the
// list at any crossing adds at least 4,080 allocations to the larger
// call. `it_counts_an_allocation_per_pair_when_a_call_builds_a_val`
// proves that the count sees them.

#[wcmp_macros::test]
async fn it_builds_no_val_for_a_typed_call_of_a_list_of_pairs() {
    let (small, large) = pair_allocations("echo-pairs").await;
    assert_eq!(
        small, large,
        "{small} allocations for 16 pairs, {large} for 4,096: the result list alone, at \
         either length"
    );
}

#[wcmp_macros::test]
async fn it_builds_no_val_for_a_typed_host_function_over_a_list_of_pairs() {
    // The pairs cross four times: into the guest, out to the host
    // function, back into the guest, and out to the caller. The host
    // function's list moves into its result, so the two lifts are the
    // two allocations, at either length.
    let (small, large) = pair_allocations("forward-pairs").await;
    assert_eq!(
        small, large,
        "{small} allocations for 16 pairs, {large} for 4,096: the two lifted lists alone, \
         at either length"
    );
}

#[wcmp_macros::test]
async fn it_builds_no_val_for_a_list_of_pairs_an_export_lifted_async_returns() {
    let (small, large) = pair_allocations("echo-pairs-async").await;
    assert_eq!(
        small, large,
        "{small} allocations for 16 pairs, {large} for 4,096: the result list alone, at \
         either length"
    );
}

#[wcmp_macros::test]
async fn it_counts_an_allocation_per_pair_when_a_call_builds_a_val() {
    // The control of the three tests above: the untyped call of the
    // same export lifts its result as a `Val`, and the count grows with
    // the list by at least one allocation a pair.
    let (mut store, instance) = typed_instance().await;
    let func = instance.get_func("echo-pairs").expect("the export");
    let args = |count: u32| {
        let pairs = pairs(count)
            .into_iter()
            .map(|(a, b)| Val::Tuple(Box::new([Val::U32(a), Val::U32(b)])))
            .collect();
        [Val::List(pairs)]
    };
    let run = async |store: &mut Store<()>, count: u32| {
        let args = args(count);
        let (allocations, output) = counted(func.call(&mut *store, &args)).await;
        assert_eq!(output.expect("the call returns").as_ref(), &args[..]);
        allocations
    };
    run(&mut store, 4096).await;
    reset(&mut store, &instance).await;
    let small = run(&mut store, 16).await;
    reset(&mut store, &instance).await;
    let large = run(&mut store, 4096).await;
    assert!(
        large - small >= MORE,
        "{small} allocations for 16 pairs, {large} for 4,096: a `Val` costs one a pair"
    );
}

/// Hand `values` to the export `name`, which returns the list it
/// receives, and answer what came back with the first `words` words of
/// guest memory the host lowered the list into. After `reset` the list
/// is the guest's first allocation, at 16.
async fn echo_list<T: 'static>(
    store: &mut Store<()>,
    instance: &Instance,
    name: &str,
    values: Vec<T>,
    words: u32,
) -> (Vec<T>, Vec<u32>)
where
    (Vec<T>,): wcmp::ComponentNamedList + wcmp::Lower + wcmp::Lift,
{
    reset(store, instance).await;
    let (echoed,) = typed::<(Vec<T>,), (Vec<T>,)>(instance, name)
        .call(&mut *store, (values,))
        .await
        .expect("the guest returns the list");
    let peek = typed::<(u32,), (u32,)>(instance, "peek");
    let mut memory = Vec::new();
    for word in 0..words {
        let (value,) = peek
            .call(&mut *store, (16 + 4 * word,))
            .await
            .expect("the peek");
        memory.push(value);
    }
    (echoed, memory)
}

#[wcmp_macros::test]
async fn it_round_trips_typed_lists_of_integers_at_their_bounds_in_little_endian_order() {
    let (mut store, instance) = typed_instance().await;
    let store = &mut store;

    let s8s = vec![i8::MIN, -1, 0, i8::MAX];
    let (echoed, memory) = echo_list(store, &instance, "echo-s8s", s8s.clone(), 1).await;
    assert_eq!(echoed, s8s);
    assert_eq!(memory, [0x7f00_ff80]);

    let s16s = vec![i16::MIN, -1, 0, i16::MAX];
    let (echoed, memory) = echo_list(store, &instance, "echo-s16s", s16s.clone(), 2).await;
    assert_eq!(echoed, s16s);
    assert_eq!(memory, [0xffff_8000, 0x7fff_0000]);

    let s32s = vec![i32::MIN, -1, 0, i32::MAX];
    let (echoed, memory) = echo_list(store, &instance, "echo-s32s", s32s.clone(), 4).await;
    assert_eq!(echoed, s32s);
    assert_eq!(memory, [0x8000_0000, 0xffff_ffff, 0, 0x7fff_ffff]);

    let s64s = vec![i64::MIN, -1, 0, i64::MAX];
    let (echoed, memory) = echo_list(store, &instance, "echo-s64s", s64s.clone(), 8).await;
    assert_eq!(echoed, s64s);
    assert_eq!(
        memory,
        [
            0,
            0x8000_0000,
            0xffff_ffff,
            0xffff_ffff,
            0,
            0,
            0xffff_ffff,
            0x7fff_ffff
        ]
    );

    let u16s = vec![0, 0x0102, 0x8000, u16::MAX];
    let (echoed, memory) = echo_list(store, &instance, "echo-u16s", u16s.clone(), 2).await;
    assert_eq!(echoed, u16s);
    assert_eq!(memory, [0x0102_0000, 0xffff_8000]);

    let u64s = vec![0, 0x0102_0304_0506_0708, u64::MAX];
    let (echoed, memory) = echo_list(store, &instance, "echo-u64s", u64s.clone(), 6).await;
    assert_eq!(echoed, u64s);
    assert_eq!(
        memory,
        [0, 0, 0x0506_0708, 0x0102_0304, 0xffff_ffff, 0xffff_ffff]
    );
}

#[wcmp_macros::test]
async fn it_round_trips_typed_lists_of_floats_at_their_bounds_in_little_endian_order() {
    let (mut store, instance) = typed_instance().await;
    let store = &mut store;

    let f32s = vec![
        f32::MIN,
        -0.0,
        f32::INFINITY,
        f32::MIN_POSITIVE,
        1.5,
        f32::NAN,
    ];
    let (echoed, memory) = echo_list(store, &instance, "echo-f32s", f32s.clone(), 5).await;
    let bits = |values: &[f32]| values.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
    assert_eq!(
        bits(&echoed[..5]),
        bits(&f32s[..5]),
        "the bits of each number"
    );
    assert!(echoed[5].is_nan(), "a NaN comes back a NaN");
    assert_eq!(
        memory,
        [
            0xff7f_ffff,
            0x8000_0000,
            0x7f80_0000,
            0x0080_0000,
            0x3fc0_0000
        ]
    );

    let f64s = vec![f64::MIN, -0.0, f64::INFINITY, 1.5, f64::NAN];
    let (echoed, memory) = echo_list(store, &instance, "echo-f64s", f64s.clone(), 8).await;
    let bits = |values: &[f64]| values.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
    assert_eq!(
        bits(&echoed[..4]),
        bits(&f64s[..4]),
        "the bits of each number"
    );
    assert!(echoed[4].is_nan(), "a NaN comes back a NaN");
    assert_eq!(
        memory,
        [
            0xffff_ffff,
            0xffef_ffff,
            0,
            0x8000_0000,
            0,
            0x7ff0_0000,
            0,
            0x3ff8_0000
        ]
    );
}

#[wcmp_macros::test]
async fn it_round_trips_typed_lists_of_bools_and_chars_at_their_bounds() {
    let (mut store, instance) = typed_instance().await;
    let store = &mut store;

    let bools = vec![false, true, true, false];
    let (echoed, memory) = echo_list(store, &instance, "echo-bools", bools.clone(), 1).await;
    assert_eq!(echoed, bools);
    assert_eq!(memory, [0x0001_0100], "one byte each, 1 for true");

    let chars = vec!['\0', 'a', '\u{d7ff}', '\u{e000}', '\u{10ffff}'];
    let (echoed, memory) = echo_list(store, &instance, "echo-chars", chars.clone(), 5).await;
    assert_eq!(echoed, chars);
    assert_eq!(memory, [0, 0x61, 0xd7ff, 0xe000, 0x10_ffff]);
}

#[wcmp_macros::test]
async fn it_refuses_a_typed_export_whose_types_differ_and_names_the_first_part() {
    let (_store, instance) = typed_instance().await;
    let error = instance
        .get_func("bytes-u16")
        .expect("the export")
        .typed::<(), (Vec<u8>,)>()
        .expect_err("a list of u8 is not a list of u16");
    let Error::TypeMismatch(mismatch) = &error else {
        panic!("expected a type mismatch, got {error:?}");
    };
    assert_eq!(
        mismatch.position,
        TypeMismatchPosition::TypedConversion {
            export: "bytes-u16".to_owned(),
            path: vec![TypePathStep::Result(0), TypePathStep::ListElement],
        },
        "{error}"
    );
    assert_eq!(
        mismatch.expected,
        TypeRendering::Value(ValueType::Primitive(PrimitiveType::U16))
    );
    assert_eq!(mismatch.actual, TypeRendering::Rust("u8".to_owned()));
    // The guest's core function traps if it runs, and the store is
    // not poisoned: no call reached the guest.
}

/// A guest that imports a function over a list of `u16`.
const NARROW: &[u8] = component!(
    r#"
    (component
      (core module $libc (memory (export "memory") 1))
      (core instance $libc (instantiate $libc))
      (import "wcmp-tests:typed/narrow@0.1.0" (instance $narrow
        (export "take" (func (param "xs" (list u16))))))
      (alias export $narrow "take" (func $take))
      (core func $take-lower (canon lower (func $take) (memory (core memory $libc "memory")))))
    "#
);

#[wcmp_macros::test]
async fn it_refuses_to_link_a_typed_host_function_whose_types_differ() {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, NARROW)
        .await
        .expect("the component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let narrow: InterfaceIdentifier = "wcmp-tests:typed/narrow@0.1.0".parse().expect("identifier");
    linker
        .instance(&narrow)
        .func_wrap("take", |_call: HostCall<'_, ()>, (_xs,): (Vec<u8>,)| Ok(()))
        .expect("the registration");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let Err(error) = linker.instantiate(&mut store, &component).await else {
        panic!("a list of u8 is not a list of u16");
    };
    let Error::TypeMismatch(mismatch) = &error else {
        panic!("expected a type mismatch, got {error:?}");
    };
    assert_eq!(
        mismatch.position,
        TypeMismatchPosition::HostFunctionRegistration {
            interface: narrow,
            item: "take".to_owned(),
            path: vec![TypePathStep::Parameter(0), TypePathStep::ListElement],
        },
        "{error}"
    );
}

#[path = "support/backend.rs"]
mod test_backend;
