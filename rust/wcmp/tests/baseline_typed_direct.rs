// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Baseline tests for the typed call's direct crossing of strings and
//! vectors of numbers.
//!
//! A `TypedFunc` lowers a `Vec<u8>` or a `String` straight into guest
//! memory and lifts one straight out, without a `Val` per element
//! between them. The untyped path holds a `Val` for every element of
//! a list, several times the element's own size, so a payload of a
//! hundred megabytes would cost the host gigabytes there.
//!
//! The file installs a counting global allocator, so the tests can
//! assert what the crossing costs the host rather than infer it from
//! a timing. The count is kept per thread and only while a test asks
//! for it: an async test runs its call on the thread it started on,
//! and nothing else in the process is counted.

#![cfg(test)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use wcmp::{Component, Engine, Instance, Linker, Store, TypedFunc, Val, ValueType};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

thread_local! {
    /// Whether this thread's allocations are being counted.
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    /// The bytes this thread has allocated while counting.
    static BYTES: Cell<usize> = const { Cell::new(0) };
    /// The allocations at or above [`LARGE`] bytes this thread has
    /// made while counting.
    static LARGE_COUNT: Cell<usize> = const { Cell::new(0) };
    /// The size at or above which an allocation counts as large.
    static LARGE: Cell<usize> = const { Cell::new(usize::MAX) };
}

/// Count an allocation of `size` bytes, when this thread is counting.
fn tally(size: usize) {
    let counting = COUNTING.try_with(Cell::get).unwrap_or(false);
    if !counting {
        return;
    }
    let _ = BYTES.try_with(|bytes| bytes.set(bytes.get() + size));
    let large = LARGE.try_with(Cell::get).unwrap_or(usize::MAX);
    if size >= large {
        let _ = LARGE_COUNT.try_with(|count| count.set(count.get() + 1));
    }
}

/// A global allocator that counts what the thread under measurement
/// asks for and passes everything through to the system's.
struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        tally(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        tally(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        tally(new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// What the thread allocated while one measurement ran.
#[derive(Debug)]
struct Allocated {
    /// Every byte asked for, whether or not it was freed again.
    bytes: usize,
    /// The allocations at or above the measurement's large size.
    large: usize,
}

/// Start counting this thread's allocations, calling an allocation
/// of `large` bytes or more a large one.
fn start_counting(large: usize) {
    BYTES.with(|bytes| bytes.set(0));
    LARGE_COUNT.with(|count| count.set(0));
    LARGE.with(|size| size.set(large));
    COUNTING.with(|counting| counting.set(true));
}

/// Stop counting and report what was allocated since the start.
fn stop_counting() -> Allocated {
    COUNTING.with(|counting| counting.set(false));
    Allocated {
        bytes: BYTES.with(Cell::get),
        large: LARGE_COUNT.with(Cell::get),
    }
}

/// A component that echoes a string or a list of bytes back: the
/// lowered pointer and length are returned unchanged, so one call is
/// one lower into guest memory and one lift back out of it. The
/// string echo is lifted under each of the three string encodings. Its
/// allocator bumps and grows the memory as a payload needs, so a
/// payload of any size the memory can reach fits. `reset` puts the
/// bump back at the start, so a later payload reuses memory an earlier
/// one grew.
///
/// It also returns values no lift accepts, each from bytes beneath
/// the bump: a string that is not UTF-8, a list of characters with
/// one past 0x10FFFF or a surrogate, and a list of `u32` at an odd
/// address. `spilled` takes eighteen flat values, two past the most
/// that pass as core parameters, so a caller spills them to memory.
const ECHO: &[u8] = component!(
    r#"
    (component
      (core module $m
        (memory (export "memory") 1)
        (global $bump (mut i32) (i32.const 16))
        (func $realloc (export "cabi_realloc")
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
        (func (export "echo") (param $ptr i32) (param $len i32) (result i32)
          (local $ret i32)
          i32.const 0 i32.const 0 i32.const 4 i32.const 8 call $realloc local.set $ret
          local.get $ret local.get $ptr i32.store
          local.get $ret local.get $len i32.store offset=4
          local.get $ret)
        (func (export "reset") i32.const 16 global.set $bump)
        ;; Beneath the bump: two bytes that are not UTF-8, the
        ;; characters `A` and 0x110000, and a surrogate.
        (data (i32.const 0) "\ff\fe\00\00\41\00\00\00\00\00\11\00\00\d8\00\00")
        (func $pair (param $ptr i32) (param $len i32) (result i32)
          (local $ret i32)
          i32.const 0 i32.const 0 i32.const 4 i32.const 8 call $realloc local.set $ret
          local.get $ret local.get $ptr i32.store
          local.get $ret local.get $len i32.store offset=4
          local.get $ret)
        (func (export "bad-string") (result i32) (call $pair (i32.const 0) (i32.const 2)))
        (func (export "bad-chars-high") (result i32) (call $pair (i32.const 4) (i32.const 2)))
        (func (export "bad-chars-surrogate") (result i32) (call $pair (i32.const 12) (i32.const 1)))
        (func (export "misaligned") (result i32) (call $pair (i32.const 1) (i32.const 1)))
        ;; The spilled tuple `(string, list<u32, 8>, list<u32, 8>)` at
        ;; `$p`: the string's length times 1000, plus each number times
        ;; its place, counting from 1, plus the string's first byte.
        (func (export "spilled") (param $p i32) (result i32)
          (local $i i32) (local $acc i32)
          (local.set $acc (i32.mul (i32.load offset=4 (local.get $p)) (i32.const 1000)))
          (block $done
            (loop $next
              (br_if $done (i32.ge_u (local.get $i) (i32.const 16)))
              (local.set $acc
                (i32.add (local.get $acc)
                  (i32.mul (i32.add (local.get $i) (i32.const 1))
                    (i32.load offset=8
                      (i32.add (local.get $p) (i32.shl (local.get $i) (i32.const 2)))))))
              (local.set $i (i32.add (local.get $i) (i32.const 1)))
              (br $next)))
          (i32.add (local.get $acc) (i32.load8_u (i32.load (local.get $p))))))
      (core instance $i (instantiate $m))
      (func (export "echo-string") (param "s" string) (result string)
        (canon lift (core func $i "echo")
          (memory (core memory $i "memory"))
          (realloc (core func $i "cabi_realloc"))))
      (func (export "echo-bytes") (param "xs" (list u8)) (result (list u8))
        (canon lift (core func $i "echo")
          (memory (core memory $i "memory"))
          (realloc (core func $i "cabi_realloc"))))
      (func (export "echo-numbers") (param "xs" (list u32)) (result (list u32))
        (canon lift (core func $i "echo")
          (memory (core memory $i "memory"))
          (realloc (core func $i "cabi_realloc"))))
      (func (export "reset") (canon lift (core func $i "reset")))
      (func (export "echo-utf16") (param "s" string) (result string)
        (canon lift (core func $i "echo") string-encoding=utf16
          (memory (core memory $i "memory"))
          (realloc (core func $i "cabi_realloc"))))
      (func (export "echo-latin1-utf16") (param "s" string) (result string)
        (canon lift (core func $i "echo") string-encoding=latin1+utf16
          (memory (core memory $i "memory"))
          (realloc (core func $i "cabi_realloc"))))
      (func (export "bad-string") (result string)
        (canon lift (core func $i "bad-string") (memory (core memory $i "memory"))))
      (func (export "bad-chars-high") (result (list char))
        (canon lift (core func $i "bad-chars-high") (memory (core memory $i "memory"))))
      (func (export "bad-chars-surrogate") (result (list char))
        (canon lift (core func $i "bad-chars-surrogate") (memory (core memory $i "memory"))))
      (func (export "misaligned") (result (list u32))
        (canon lift (core func $i "misaligned") (memory (core memory $i "memory"))))
      (func (export "spilled")
        (param "s" string) (param "a" (list u32 8)) (param "b" (list u32 8)) (result u32)
        (canon lift (core func $i "spilled")
          (memory (core memory $i "memory"))
          (realloc (core func $i "cabi_realloc")))))
    "#
);

/// Instantiate [`ECHO`] into a store of its own.
async fn echo() -> (Store<()>, Instance) {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, ECHO)
        .await
        .expect("the echo component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiation succeeds");
    (store, instance)
}

/// The typed handle of the export `name`.
fn typed<P, R>(instance: &Instance, name: &str) -> TypedFunc<P, R>
where
    P: wcmp::ComponentParameters,
    R: wcmp::ComponentResult,
{
    instance
        .get_func(name)
        .expect("the export")
        .typed::<P, R>()
        .expect("the export has the typed signature")
}

/// Grow the echo's memory to hold a payload of `len` bytes, and put
/// its bump back at the start, so a call that follows grows nothing.
/// Wasmi grows a memory by reallocating the host vector that holds
/// it, which the counter would take for a copy of the payload that
/// the crossing made; Wasmtime maps its memory and the counter never
/// sees it.
async fn grown(store: &mut Store<()>, instance: &Instance, len: usize) {
    typed::<(Vec<u8>,), Vec<u8>>(instance, "echo-bytes")
        .call(&mut *store, (vec![0; len],))
        .await
        .expect("the warm-up echoes");
    instance
        .get_func("reset")
        .expect("the export")
        .call(&mut *store, &[])
        .await
        .expect("the bump resets");
}

/// `len` bytes of a pattern that no two neighbouring positions share.
fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|index| (index % 251) as u8).collect()
}

/// The payload of the tests that run on both targets: a MiB, large
/// enough that a `Val` per element would dwarf everything else the
/// call allocates.
const PAYLOAD: usize = 1 << 20;

#[wcmp_macros::test]
async fn it_round_trips_a_byte_vector_without_a_val_per_element() {
    let (mut store, instance) = echo().await;
    let echo_bytes = typed::<(Vec<u8>,), Vec<u8>>(&instance, "echo-bytes");

    let payload = pattern(PAYLOAD);
    grown(&mut store, &instance, PAYLOAD).await;
    let expected = payload.clone();
    start_counting(PAYLOAD / 2);
    let returned = echo_bytes
        .call(&mut store, (payload,))
        .await
        .expect("the bytes echo");
    let allocated = stop_counting();

    assert_eq!(returned, expected);
    // The lift reads the list into one buffer of its size, and that
    // buffer is the vector returned. The lower writes from the
    // caller's own vector, so the way in allocates nothing of the
    // payload's size at all.
    assert_eq!(
        allocated.large, 1,
        "one payload-sized allocation: {allocated:?}"
    );
    assert!(
        allocated.bytes < PAYLOAD + PAYLOAD / 4,
        "the call allocated about the payload once, not a `Val` per element: {allocated:?}"
    );
}

#[wcmp_macros::test]
async fn it_round_trips_a_string_without_a_val_per_element() {
    let (mut store, instance) = echo().await;
    let echo_string = typed::<(String,), String>(&instance, "echo-string");

    let payload: String = "wasm component 🍰 "
        .chars()
        .cycle()
        .take(PAYLOAD / 2)
        .collect();
    grown(&mut store, &instance, payload.len()).await;
    let expected = payload.clone();
    start_counting(payload.len() / 2);
    let returned = echo_string
        .call(&mut store, (payload,))
        .await
        .expect("the string echoes");
    let allocated = stop_counting();

    assert_eq!(returned, expected);
    assert_eq!(
        allocated.large, 1,
        "one payload-sized allocation: {allocated:?}"
    );
    assert!(
        allocated.bytes < expected.len() + expected.len() / 4,
        "the call allocated about the string once: {allocated:?}"
    );
}

#[wcmp_macros::test]
async fn it_counts_the_val_per_element_the_untyped_path_holds() {
    // The same echo through `Func::call` holds a `Val` per element on
    // the way in and again on the way out, which the counter sees as
    // many times the payload. This is what the two tests above would
    // report had the typed call gone through `Val`.
    let (mut store, instance) = echo().await;
    let echo_bytes = instance.get_func("echo-bytes").expect("the export");
    let elements: Vec<Val> = pattern(PAYLOAD).into_iter().map(Val::U8).collect();
    let arguments = [Val::List(elements.into_boxed_slice())];

    start_counting(PAYLOAD / 2);
    let returned = echo_bytes
        .call(&mut store, &arguments)
        .await
        .expect("the bytes echo");
    let allocated = stop_counting();

    assert!(matches!(&returned[0], Val::List(items) if items.len() == PAYLOAD));
    assert!(
        allocated.bytes >= 4 * PAYLOAD,
        "the untyped path holds several bytes per element: {allocated:?}"
    );
}

#[wcmp_macros::test]
async fn it_round_trips_a_vector_of_wider_numbers() {
    let (mut store, instance) = echo().await;
    let echo_numbers = typed::<(Vec<u32>,), Vec<u32>>(&instance, "echo-numbers");
    let payload: Vec<u32> = (0..4096u32).map(|n| n.wrapping_mul(0x9E37_79B9)).collect();
    let returned = echo_numbers
        .call(&mut store, (payload.clone(),))
        .await
        .expect("the numbers echo");
    assert_eq!(returned, payload);
}

#[wcmp_macros::test]
async fn it_round_trips_empty_values() {
    let (mut store, instance) = echo().await;
    let echo_bytes = typed::<(Vec<u8>,), Vec<u8>>(&instance, "echo-bytes");
    let echo_string = typed::<(String,), String>(&instance, "echo-string");
    assert!(
        echo_bytes
            .call(&mut store, (Vec::new(),))
            .await
            .expect("an empty list echoes")
            .is_empty()
    );
    assert!(
        echo_string
            .call(&mut store, (String::new(),))
            .await
            .expect("an empty string echoes")
            .is_empty()
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_typed_handle_of_the_wrong_element_type() {
    let (_store, instance) = echo().await;
    let refused = instance
        .get_func("echo-bytes")
        .expect("the export")
        .typed::<(Vec<u32>,), Vec<u32>>();
    assert!(refused.is_err());
    // The declared types the direct path checks against are the ones
    // the untyped signature reports.
    let func = instance.get_func("echo-bytes").expect("the export");
    assert!(matches!(func.ty().parameters[0].ty, ValueType::List(_)));
}

#[wcmp_macros::test]
async fn it_round_trips_a_typed_string_under_each_utf16_encoding() {
    let (mut store, instance) = echo().await;
    for name in ["echo-utf16", "echo-latin1-utf16"] {
        let echo_typed = typed::<(String,), String>(&instance, name);
        let echo_untyped = instance.get_func(name).expect("the export");
        // ASCII, Latin-1 beyond ASCII, and text only UTF-16 holds, which
        // moves `latin1+utf16` to its UTF-16 form.
        for text in ["plain", "café crème", "日本語 🎉"] {
            let returned = echo_typed
                .call(&mut store, (text.to_owned(),))
                .await
                .expect("the typed string echoes");
            assert_eq!(returned, text, "{name}");
            let untyped = echo_untyped
                .call(&mut store, &[Val::String(text.to_owned())])
                .await
                .expect("the untyped string echoes");
            assert_eq!(untyped.as_ref(), [Val::String(returned)], "{name}");
        }
    }
}

#[wcmp_macros::test]
async fn it_stores_typed_parameters_that_spill_past_the_flat_limit() {
    let (mut store, instance) = echo().await;
    let spilled = typed::<(String, [u32; 8], [u32; 8]), u32>(&instance, "spilled");
    let text = "spill".to_owned();
    let first = [1, 2, 3, 4, 5, 6, 7, 8];
    let second = [10, 20, 30, 40, 50, 60, 70, 80];
    let expected = text.len() as u32 * 1000
        + first
            .iter()
            .chain(&second)
            .enumerate()
            .map(|(index, value)| (index as u32 + 1) * value)
            .sum::<u32>()
        + u32::from(text.as_bytes()[0]);

    let returned = spilled
        .call(&mut store, (text.clone(), first, second))
        .await
        .expect("the typed call spills its parameters");
    assert_eq!(returned, expected);
    let fixed = |values: [u32; 8]| Val::FixedLengthList(values.map(Val::U32).into());
    let untyped = instance
        .get_func("spilled")
        .expect("the export")
        .call(
            &mut store,
            &[Val::String(text), fixed(first), fixed(second)],
        )
        .await
        .expect("the untyped call spills its parameters");
    assert_eq!(untyped.as_ref(), [Val::U32(expected)]);
}

/// The message the typed call of `name` fails with, and the message
/// the untyped call fails with, each in a store of its own, since a
/// failed lift traps and poisons the store.
async fn both_failures<R: wcmp::ComponentResult + std::fmt::Debug>(name: &str) -> (String, String) {
    let (mut store, instance) = echo().await;
    let typed_failure = typed::<(), R>(&instance, name)
        .call(&mut store, ())
        .await
        .expect_err("the typed lift refuses the value");
    let (mut store, instance) = echo().await;
    let untyped_failure = instance
        .get_func(name)
        .expect("the export")
        .call(&mut store, &[])
        .await
        .expect_err("the untyped lift refuses the value");
    (typed_failure.to_string(), untyped_failure.to_string())
}

#[wcmp_macros::test]
async fn it_fails_each_typed_lift_with_the_untyped_paths_message() {
    for (name, cause, (typed_message, untyped_message)) in [
        (
            "bad-string",
            "invalid utf-8",
            both_failures::<String>("bad-string").await,
        ),
        (
            "bad-chars-high",
            "not a valid Unicode scalar",
            both_failures::<Vec<char>>("bad-chars-high").await,
        ),
        (
            "bad-chars-surrogate",
            "not a valid Unicode scalar",
            both_failures::<Vec<char>>("bad-chars-surrogate").await,
        ),
        (
            "misaligned",
            "list pointer is not aligned",
            both_failures::<Vec<u32>>("misaligned").await,
        ),
    ] {
        assert!(untyped_message.contains(cause), "{name}: {untyped_message}");
        assert_eq!(typed_message, untyped_message, "{name}");
    }
}

#[wcmp_macros::test]
async fn it_carries_a_typed_byte_vector_and_string_through_call_concurrent() {
    let (mut store, instance) = echo().await;
    let echo_bytes = typed::<(Vec<u8>,), Vec<u8>>(&instance, "echo-bytes");
    let echo_string = typed::<(String,), String>(&instance, "echo-string");
    let payload = pattern(4096);
    let text = "héllo, wörld 🎉".to_owned();

    let (bytes, string) = store
        .run_concurrent(async |accessor| {
            let bytes = echo_bytes
                .call_concurrent(accessor, (payload.clone(),))
                .await?;
            let string = echo_string
                .call_concurrent(accessor, (text.clone(),))
                .await?;
            Ok::<_, wcmp::Error>((bytes, string))
        })
        .await
        .expect("run the closure")
        .expect("call the exports");
    assert_eq!(bytes, payload);
    assert_eq!(string, text);
    assert_eq!(
        echo_bytes
            .call(&mut store, (payload.clone(),))
            .await
            .expect("the direct call"),
        bytes,
        "the concurrent call answers what the direct one does"
    );
}

/// Over a hundred megabytes, which the untyped path would hold as
/// several gigabytes of `Val`. It stays inside the copy budget of
/// 128 MiB one crossing may build, which a typed vector of bytes
/// charges a byte per element, as Wasmtime charges it.
#[cfg(not(target_arch = "wasm32"))]
const OVER_A_HUNDRED_MEGABYTES: usize = 120 << 20;

#[cfg(not(target_arch = "wasm32"))]
#[wcmp_macros::test]
async fn it_round_trips_over_a_hundred_megabytes_in_one_copy_per_direction() {
    let (mut store, instance) = echo().await;
    let echo_bytes = typed::<(Vec<u8>,), Vec<u8>>(&instance, "echo-bytes");

    let payload = pattern(OVER_A_HUNDRED_MEGABYTES);
    grown(&mut store, &instance, OVER_A_HUNDRED_MEGABYTES).await;
    start_counting(OVER_A_HUNDRED_MEGABYTES / 2);
    let returned = echo_bytes
        .call(&mut store, (payload,))
        .await
        .expect("over a hundred megabytes echo");
    let allocated = stop_counting();

    // The way in is the write from the caller's vector into guest
    // memory, which the substrate makes and the host allocator never
    // sees. The way out is one read into one buffer of the payload's
    // size, which becomes the returned vector.
    assert_eq!(
        allocated.large, 1,
        "one payload-sized allocation for the two directions: {allocated:?}"
    );
    assert!(
        allocated.bytes < OVER_A_HUNDRED_MEGABYTES + (1 << 20),
        "nothing but that one copy is of any size: {allocated:?}"
    );
    assert_eq!(returned.len(), OVER_A_HUNDRED_MEGABYTES);
    assert!(
        returned
            .iter()
            .enumerate()
            .all(|(index, byte)| *byte == (index % 251) as u8),
        "the bytes came back as they went in"
    );
}

#[path = "support/backend.rs"]
mod test_backend;
