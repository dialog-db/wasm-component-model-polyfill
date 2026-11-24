// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Baseline tests for how the items of a stream cross between a guest
//! and the host.
//!
//! The reference reads a write's items as one list
//! (`load_list_from_valid_range`) and stores a read's items one after
//! another into the reader's buffer (`store_list_into_valid_range`),
//! and so does Wasmtime's host end. The polyfill follows both, and
//! keeps the accesses of guest memory few:
//!
//! - A host delivery of numbers reaches the guest in one write.
//! - A host delivery of any other type is written item by item where
//!   it goes, so the padding of the guest's buffer stays as the guest
//!   left it.
//! - A host read of numbers takes the range in one read and charges
//!   the copy budget for the host values it builds, as Wasmtime's
//!   `WasmList` does.
//! - A host read of any other type lifts each item out of one read of
//!   the range, so the items lifted before one that fails stay with
//!   the consumer, as they do in Wasmtime.
//!
//! The tests count the runtime-layer reads and writes of guest memory
//! through the eager strategy's count of them.

#![cfg(test)]

use core::pin::Pin;
use core::task::{Context, Poll};
use std::sync::{Arc, Mutex};

use crate::abi::strategy::memory_accesses;
use crate::{
    Component, Engine, Error, Instance, Linker, Source, Store, StoreContext, StreamConsumer,
    StreamReader, StreamResult, Val,
};
use wcmp_macros::component;

/// The address of the buffer each copy goes through.
const BUFFER: u32 = 4096;

/// The number of bytes the byte copies move.
const BYTES: u32 = 4096;

/// A component that serves and makes streams for the host: a
/// `stream<u8>` and a `stream<pair>` the host writes, where `pair` is a
/// record of a `u8` and a `u32` with three bytes of padding between
/// them, and a `stream<u8>` and a `stream<option<u8>>` the host reads.
///
/// `take-<kind>` answers the index the readable end the host passes
/// arrived under, and `read-<kind>` reads from it. `make-<kind>`
/// creates a stream, keeps its writable end, and hands the host the
/// readable one, which `write-<kind>` writes to. `fill` sets a range
/// of memory to one byte, and `poke` and `peek` reach a word.
const COPIES: &[u8] = component!(
    r#"
    (component
      (type $pair-def (record (field "a" u8) (field "b" u32)))
      (export $pair "pair" (type $pair-def))
      (type $bytes (stream u8))
      (type $pairs (stream $pair))
      (type $maybes (stream (option u8)))
      (core module $libc (memory (export "memory") 3))
      (core instance $libc (instantiate $libc))
      (core func $read-bytes
        (canon stream.read $bytes async (memory (core memory $libc "memory"))))
      (core func $read-pairs
        (canon stream.read $pairs async (memory (core memory $libc "memory"))))
      (core func $new-bytes (canon stream.new $bytes))
      (core func $new-maybes (canon stream.new $maybes))
      (core func $write-bytes
        (canon stream.write $bytes async (memory (core memory $libc "memory"))))
      (core func $write-maybes
        (canon stream.write $maybes async (memory (core memory $libc "memory"))))
      (core module $m
        (import "libc" "memory" (memory 3))
        (import "" "read-bytes" (func $read-bytes (param i32 i32 i32) (result i32)))
        (import "" "read-pairs" (func $read-pairs (param i32 i32 i32) (result i32)))
        (import "" "new-bytes" (func $new-bytes (result i64)))
        (import "" "new-maybes" (func $new-maybes (result i64)))
        (import "" "write-bytes" (func $write-bytes (param i32 i32 i32) (result i32)))
        (import "" "write-maybes" (func $write-maybes (param i32 i32 i32) (result i32)))
        (global $writable (mut i32) (i32.const 0))
        (func $keep (param $ends i64) (result i32)
          (global.set $writable (i32.wrap_i64 (i64.shr_u (local.get $ends) (i64.const 32))))
          (i32.wrap_i64 (local.get $ends)))
        (func (export "take") (param i32) (result i32) (local.get 0))
        (func (export "read-bytes") (param i32 i32 i32) (result i32)
          (call $read-bytes (local.get 0) (local.get 1) (local.get 2)))
        (func (export "read-pairs") (param i32 i32 i32) (result i32)
          (call $read-pairs (local.get 0) (local.get 1) (local.get 2)))
        (func (export "make-bytes") (result i32) (call $keep (call $new-bytes)))
        (func (export "make-maybes") (result i32) (call $keep (call $new-maybes)))
        (func (export "write-bytes") (param i32 i32) (result i32)
          (call $write-bytes (global.get $writable) (local.get 0) (local.get 1)))
        (func (export "write-maybes") (param i32 i32) (result i32)
          (call $write-maybes (global.get $writable) (local.get 0) (local.get 1)))
        (func (export "fill") (param i32 i32 i32)
          (memory.fill (local.get 0) (local.get 1) (local.get 2)))
        (func (export "poke") (param i32 i32) (i32.store (local.get 0) (local.get 1)))
        (func (export "peek") (param i32) (result i32) (i32.load (local.get 0))))
      (core instance $i (instantiate $m
        (with "libc" (instance $libc))
        (with "" (instance
          (export "read-bytes" (func $read-bytes))
          (export "read-pairs" (func $read-pairs))
          (export "new-bytes" (func $new-bytes))
          (export "new-maybes" (func $new-maybes))
          (export "write-bytes" (func $write-bytes))
          (export "write-maybes" (func $write-maybes))))))
      (func (export "take-bytes") (param "s" $bytes) (result u32)
        (canon lift (core func $i "take")))
      (func (export "take-pairs") (param "s" $pairs) (result u32)
        (canon lift (core func $i "take")))
      (func (export "read-bytes") (param "e" u32) (param "p" u32) (param "n" u32) (result u32)
        (canon lift (core func $i "read-bytes")))
      (func (export "read-pairs") (param "e" u32) (param "p" u32) (param "n" u32) (result u32)
        (canon lift (core func $i "read-pairs")))
      (func (export "make-bytes") (result $bytes) (canon lift (core func $i "make-bytes")))
      (func (export "make-maybes") (result $maybes) (canon lift (core func $i "make-maybes")))
      (func (export "write-bytes") (param "p" u32) (param "n" u32) (result u32)
        (canon lift (core func $i "write-bytes")))
      (func (export "write-maybes") (param "p" u32) (param "n" u32) (result u32)
        (canon lift (core func $i "write-maybes")))
      (func (export "fill") (param "p" u32) (param "v" u32) (param "n" u32)
        (canon lift (core func $i "fill")))
      (func (export "poke") (param "p" u32) (param "v" u32) (canon lift (core func $i "poke")))
      (func (export "peek") (param "p" u32) (result u32) (canon lift (core func $i "peek"))))
    "#
);

/// The component's `pair`: a `u8`, three bytes of padding, and a `u32`.
#[derive(crate::ComponentType, crate::Lift, crate::Lower, Clone, Copy, Debug, PartialEq)]
#[component(record)]
#[component(wcmp_crate = crate)]
struct Pair {
    a: u8,
    b: u32,
}

/// What a consumer took, and the error its last read failed with.
type Taken<T> = Arc<Mutex<(Vec<T>, Option<String>)>>;

/// A consumer that takes the whole of every write, keeping what it
/// took even when the read fails partway.
struct Collect<T>(Taken<T>);

impl<T: crate::Lift + Send + 'static> StreamConsumer<()> for Collect<T> {
    type Item = T;

    fn poll_consume(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        store: &mut StoreContext<'_, ()>,
        mut source: Source<'_, T>,
        _finish: bool,
    ) -> Poll<Result<StreamResult, Error>> {
        let count = source.remaining();
        let mut items = Vec::new();
        let read = source.read(store, &mut items, count);
        let mut taken = self.0.lock().expect("the record of what was taken");
        taken.0.extend(items);
        Poll::Ready(match read {
            Ok(()) => Ok(StreamResult::Completed),
            Err(error) => {
                taken.1 = Some(chain(&error));
                Err(error)
            }
        })
    }
}

/// Instantiate [`COPIES`] into a store of its own.
async fn instantiate() -> (Store<()>, Instance) {
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, COPIES)
        .await
        .expect("component parses");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let linker: Linker<()> = Linker::new(&engine);
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    (store, instance)
}

/// Every message in an error's source chain, joined.
fn chain(error: &Error) -> String {
    let mut out = String::new();
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(link) = current {
        if !out.is_empty() {
            out.push_str(": ");
        }
        out.push_str(&link.to_string());
        current = link.source();
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Call `name` with the numbers `args` and report what it returned.
async fn try_call(
    store: &mut Store<()>,
    instance: &Instance,
    name: &str,
    args: &[u32],
) -> Result<Option<Val>, Error> {
    let args: Vec<Val> = args.iter().map(|&n| Val::U32(n)).collect();
    let values = instance
        .get_func(name)
        .unwrap_or_else(|| panic!("the component exports `{name}`"))
        .call(store, &args)
        .await?;
    Ok(values.first().cloned())
}

/// Call `name` with the numbers `args` and expect one `u32` back, or
/// nothing when `name` returns nothing.
async fn call(store: &mut Store<()>, instance: &Instance, name: &str, args: &[u32]) -> u32 {
    match try_call(store, instance, name, args).await {
        Ok(Some(Val::U32(value))) => value,
        Ok(None) => 0,
        Ok(other) => panic!("{name} answered {other:?}"),
        Err(error) => panic!("{name} failed: {}", chain(&error)),
    }
}

/// Hand `reader` to the guest through `take-<kind>`, and answer the
/// index its readable end arrived under.
async fn take<T: crate::Lower + crate::Lift + 'static>(
    store: &mut Store<()>,
    instance: &Instance,
    kind: &str,
    reader: StreamReader<T>,
) -> u32 {
    instance
        .get_func(&format!("take-{kind}"))
        .expect("the component exports `take-<kind>`")
        .typed::<(StreamReader<T>,), (u32,)>()
        .expect("`take-<kind>` takes a stream")
        .call(store, (reader,))
        .await
        .expect("the readable end crosses to the guest")
        .0
}

/// Have the guest make a stream through `make-<kind>`, and pipe the
/// readable end it hands the host to a consumer that collects what
/// each write offers.
async fn collect<T: crate::Lift + Send + 'static>(
    store: &mut Store<()>,
    instance: &Instance,
    kind: &str,
) -> Taken<T> {
    let reader = instance
        .get_func(&format!("make-{kind}"))
        .expect("the component exports `make-<kind>`")
        .typed::<(), (StreamReader<T>,)>()
        .expect("`make-<kind>` returns a stream")
        .call(&mut *store, ())
        .await
        .expect("the readable end crosses to the host")
        .0;
    let taken = Taken::default();
    reader
        .pipe(&mut store.as_context_mut(), Collect(taken.clone()))
        .expect("the host pipes the stream");
    taken
}

/// The count a copy's packed result carries.
fn count(packed: u32) -> u32 {
    packed >> 4
}

/// The runtime-layer reads and writes of guest memory `run` made.
async fn accesses<R>(run: impl core::future::Future<Output = R>) -> (R, (usize, usize)) {
    let before = memory_accesses();
    let result = run.await;
    let after = memory_accesses();
    (result, (after.0 - before.0, after.1 - before.1))
}

/// The byte at index `i` of the byte copies.
fn byte(i: u32) -> u8 {
    (i * 7 + (i >> 9)) as u8
}

/// The little-endian word the bytes at `at` of `bytes` make.
fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
}

#[wcmp_macros::test]
async fn it_writes_a_host_delivery_of_bytes_into_the_guest_in_one_write() {
    let (mut store, instance) = instantiate().await;
    let bytes: Vec<u8> = (0..BYTES).map(byte).collect();
    let reader = StreamReader::new(&mut store.as_context_mut(), bytes.clone())
        .expect("the host makes a stream");
    let end = take(&mut store, &instance, "bytes", reader).await;
    let (read, (reads, writes)) = accesses(call(
        &mut store,
        &instance,
        "read-bytes",
        &[end, BUFFER, BYTES],
    ))
    .await;
    assert_eq!(count(read), BYTES, "the read takes every byte at once");
    assert_eq!(
        (reads, writes),
        (0, 1),
        "the bytes reach the guest in one write"
    );
    for at in (0..BYTES).step_by(4) {
        assert_eq!(
            call(&mut store, &instance, "peek", &[BUFFER + at]).await,
            word(&bytes, at as usize),
            "the guest holds the bytes the host wrote at {at}"
        );
    }
}

#[wcmp_macros::test]
async fn it_leaves_the_padding_of_the_guests_buffer_as_it_was_when_the_host_writes_records() {
    let (mut store, instance) = instantiate().await;
    // Four records' room, every byte of it marked, for three records.
    call(&mut store, &instance, "fill", &[BUFFER, 0x55, 32]).await;
    let pairs = vec![
        Pair { a: 0xa1, b: 1000 },
        Pair { a: 0xb2, b: 2000 },
        Pair { a: 0xc3, b: 3000 },
    ];
    let reader =
        StreamReader::new(&mut store.as_context_mut(), pairs).expect("the host makes a stream");
    let end = take(&mut store, &instance, "pairs", reader).await;
    let read = call(&mut store, &instance, "read-pairs", &[end, BUFFER, 4]).await;
    assert_eq!(count(read), 3, "the read takes the three records");
    let mut words = Vec::new();
    for at in (0..32).step_by(4) {
        words.push(call(&mut store, &instance, "peek", &[BUFFER + at]).await);
    }
    assert_eq!(
        words,
        [
            0x5555_55a1,
            1000,
            0x5555_55b2,
            2000,
            0x5555_55c3,
            3000,
            0x5555_5555,
            0x5555_5555
        ],
        "each field arrived, the padding after each `a` kept the guest's mark, \
         and the slot no record reached is untouched"
    );
}

#[wcmp_macros::test]
async fn it_reads_a_guests_write_of_bytes_to_the_host_in_one_read() {
    let (mut store, instance) = instantiate().await;
    let taken = collect::<u8>(&mut store, &instance, "bytes").await;
    let bytes: Vec<u8> = (0..BYTES).map(byte).collect();
    for at in (0..BYTES).step_by(4) {
        call(
            &mut store,
            &instance,
            "poke",
            &[BUFFER + at, word(&bytes, at as usize)],
        )
        .await;
    }
    let (written, (reads, writes)) =
        accesses(call(&mut store, &instance, "write-bytes", &[BUFFER, BYTES])).await;
    assert_eq!(count(written), BYTES, "the host takes every byte at once");
    assert_eq!(
        (reads, writes),
        (1, 0),
        "the host takes the bytes in one read"
    );
    let taken = taken.lock().expect("the record of what was taken");
    assert_eq!(taken.0, bytes, "the host holds the bytes the guest wrote");
    assert_eq!(taken.1, None, "and its read did not fail");
}

#[wcmp_macros::test]
async fn it_charges_the_copy_budget_for_the_bytes_a_host_read_builds() {
    let (mut store, instance) = instantiate().await;
    let taken = collect::<u8>(&mut store, &instance, "bytes").await;
    // A budget one byte short of the write refuses the read before it
    // takes anything, as Wasmtime's measure of the list refuses it.
    store.set_hostcall_fuel(BYTES as usize - 1);
    let error = try_call(&mut store, &instance, "write-bytes", &[BUFFER, BYTES])
        .await
        .expect_err("a read that builds more than the budget fails");
    let message = chain(&error);
    assert!(
        message.contains("fuel allocated for hostcalls has been exhausted"),
        "expected the budget cause, got {message}"
    );
    let taken = taken.lock().expect("the record of what was taken");
    assert!(taken.0.is_empty(), "the host took no byte");
}

#[wcmp_macros::test]
async fn it_keeps_the_items_a_host_read_lifted_before_one_that_is_no_value() {
    let (mut store, instance) = instantiate().await;
    let taken = collect::<Option<u8>>(&mut store, &instance, "maybes").await;
    // Four `option<u8>` of two bytes apiece: `some(0x61)`, `some(0x62)`,
    // a discriminant of 2, which is no case of an option, and
    // `some(0x63)`.
    call(&mut store, &instance, "poke", &[BUFFER, 0x6201_6101]).await;
    call(&mut store, &instance, "poke", &[BUFFER + 4, 0x6301_0002]).await;
    let error = try_call(&mut store, &instance, "write-maybes", &[BUFFER, 4])
        .await
        .expect_err("a write with no value in it fails the host's read");
    let taken = taken.lock().expect("the record of what was taken");
    assert_eq!(
        taken.0,
        [Some(0x61), Some(0x62)],
        "the items lifted before the bad one stay with the host, as in Wasmtime"
    );
    let reason = taken.1.clone().expect("the host's read failed");
    assert!(
        reason.contains("discriminant 2 out of range"),
        "the read failed on the bad discriminant, not on {reason}"
    );
    assert_eq!(
        chain(&error),
        reason,
        "and the guest's write failed with the host's error"
    );
}
