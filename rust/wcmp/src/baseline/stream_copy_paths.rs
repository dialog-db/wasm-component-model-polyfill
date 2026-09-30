//! Baseline tests for the two paths a stream or future copy takes
//! between two components.
//!
//! A payload of a number type moves as bytes: one runtime-layer read
//! of the writer's memory and one write of the reader's, with no
//! value built, so the copy charges no copy budget, and a NaN keeps
//! its bits. Each side's bytes are found from where its copy has got
//! to. Any other payload moves one value at a time through the two
//! boundary contexts, and the lift of those values charges the
//! budget. The owned handles of a `stream<own<r>>` move between the
//! two tables along that second path, which `stream_copy_handles`
//! proves.
//!
//! The tests compose a reader and a writer of a `stream<u8>`, a
//! `stream<record>`, a `stream<u32>`, and a `future<f64>`. The outer
//! component exports the functions of both, so a test drives each
//! side directly, and counts the runtime-layer accesses a copy makes
//! through the eager strategy's count of them.

#![cfg(test)]

use crate::abi::strategy::memory_accesses;
use crate::{Component, Engine, Error, Func, Instance, Linker, Store, Val};
use wcmp_macros::component;

/// The word a copy returns when it has not finished.
const BLOCKED: u32 = 0xffff_ffff;

/// One mebibyte, the length of the byte copy.
const MEBIBYTE: u32 = 1 << 20;

/// The address of the buffer each side copies through, past the first
/// page, where the event a wait delivers lands.
const BUFFER: u32 = 1 << 16;

/// A reader and a writer, composed, of a `stream<u8>`, a
/// `stream<pair>`, where `pair` is a record of a `u8` and a `u32`, a
/// `stream<u32>`, and a `future<f64>`.
///
/// Each side has a memory of eighteen pages, room for a mebibyte past
/// the first page. The writer's `fill` writes the pattern the
/// reader's `verify` checks: the byte at index `i` of a range is the
/// low byte of `7 * i + (i >> 9)`, and `verify` answers how many bytes
/// of a range differ from it. The writer's `give-<kind>` passes the
/// readable end at the index it is given to the reader's `take`,
/// which answers the index it arrived under. `read-<kind>` and
/// `write-<kind>` are the asynchronous copy built-ins, for `bytes`,
/// `pairs`, `words`, and `nan`, `wait` delivers the event of the end
/// it is given and answers its packed result, and `poke` and `peek`
/// reach the writer's and the reader's memory.
const PATHS: &[u8] = component!(
    r#"
    (component
      (component $reader
        (type $pair-def (record (field "a" u8) (field "b" u32)))
        (export $pair "pair" (type $pair-def))
        (type $bytes (stream u8))
        (type $pairs (stream $pair))
        (type $words (stream u32))
        (type $nan (future f64))
        (core module $libc (memory (export "memory") 18))
        (core instance $libc (instantiate $libc))
        (core func $read-bytes
          (canon stream.read $bytes async (memory (core memory $libc "memory"))))
        (core func $read-pairs
          (canon stream.read $pairs async (memory (core memory $libc "memory"))))
        (core func $read-words
          (canon stream.read $words async (memory (core memory $libc "memory"))))
        (core func $read-nan
          (canon future.read $nan async (memory (core memory $libc "memory"))))
        (core func $set-new (canon waitable-set.new))
        (core func $wait (canon waitable-set.wait (memory (core memory $libc "memory"))))
        (core func $join (canon waitable.join))
        (core module $m
          (import "libc" "memory" (memory 18))
          (import "" "read-bytes" (func $read-bytes (param i32 i32 i32) (result i32)))
          (import "" "read-pairs" (func $read-pairs (param i32 i32 i32) (result i32)))
          (import "" "read-words" (func $read-words (param i32 i32 i32) (result i32)))
          (import "" "read-nan" (func $read-nan (param i32 i32) (result i32)))
          (import "" "waitable-set.new" (func $set-new (result i32)))
          (import "" "waitable-set.wait" (func $wait (param i32 i32) (result i32)))
          (import "" "waitable.join" (func $join (param i32 i32)))
          (func (export "take") (param i32) (result i32) (local.get 0))
          (func (export "read-bytes") (param i32 i32 i32) (result i32)
            (call $read-bytes (local.get 0) (local.get 1) (local.get 2)))
          (func (export "read-pairs") (param i32 i32 i32) (result i32)
            (call $read-pairs (local.get 0) (local.get 1) (local.get 2)))
          (func (export "read-words") (param i32 i32 i32) (result i32)
            (call $read-words (local.get 0) (local.get 1) (local.get 2)))
          (func (export "read-nan") (param i32 i32) (result i32)
            (call $read-nan (local.get 0) (local.get 1)))
          (func (export "wait") (param $end i32) (result i32)
            (local $set i32)
            (local.set $set (call $set-new))
            (call $join (local.get $end) (local.get $set))
            (drop (call $wait (local.get $set) (i32.const 0)))
            (call $join (local.get $end) (i32.const 0))
            (i32.load (i32.const 4)))
          (func (export "verify") (param $p i32) (param $n i32) (result i32)
            (local $i i32) (local $wrong i32)
            (block $done
              (loop $next
                (br_if $done (i32.ge_u (local.get $i) (local.get $n)))
                (if (i32.ne
                      (i32.load8_u (i32.add (local.get $p) (local.get $i)))
                      (i32.and
                        (i32.add
                          (i32.mul (local.get $i) (i32.const 7))
                          (i32.shr_u (local.get $i) (i32.const 9)))
                        (i32.const 0xff)))
                  (then (local.set $wrong (i32.add (local.get $wrong) (i32.const 1)))))
                (local.set $i (i32.add (local.get $i) (i32.const 1)))
                (br $next)))
            (local.get $wrong))
          (func (export "peek") (param i32) (result i32) (i32.load (local.get 0))))
        (core instance $i (instantiate $m
          (with "libc" (instance $libc))
          (with "" (instance
            (export "read-bytes" (func $read-bytes))
            (export "read-pairs" (func $read-pairs))
            (export "read-words" (func $read-words))
            (export "read-nan" (func $read-nan))
            (export "waitable-set.new" (func $set-new))
            (export "waitable-set.wait" (func $wait))
            (export "waitable.join" (func $join))))))
        (func (export "take-bytes") (param "s" $bytes) (result u32)
          (canon lift (core func $i "take")))
        (func (export "take-pairs") (param "s" $pairs) (result u32)
          (canon lift (core func $i "take")))
        (func (export "take-words") (param "s" $words) (result u32)
          (canon lift (core func $i "take")))
        (func (export "take-nan") (param "f" $nan) (result u32)
          (canon lift (core func $i "take")))
        (func (export "read-bytes") (param "e" u32) (param "p" u32) (param "n" u32) (result u32)
          (canon lift (core func $i "read-bytes")))
        (func (export "read-pairs") (param "e" u32) (param "p" u32) (param "n" u32) (result u32)
          (canon lift (core func $i "read-pairs")))
        (func (export "read-words") (param "e" u32) (param "p" u32) (param "n" u32) (result u32)
          (canon lift (core func $i "read-words")))
        (func (export "read-nan") (param "e" u32) (param "p" u32) (result u32)
          (canon lift (core func $i "read-nan")))
        (func (export "wait") (param "e" u32) (result u32) (canon lift (core func $i "wait")))
        (func (export "verify") (param "p" u32) (param "n" u32) (result u32)
          (canon lift (core func $i "verify")))
        (func (export "peek") (param "p" u32) (result u32) (canon lift (core func $i "peek"))))

      (component $writer
        (type $pair-def (record (field "a" u8) (field "b" u32)))
        (import "pair" (type $pair (eq $pair-def)))
        (type $bytes (stream u8))
        (type $pairs (stream $pair))
        (type $words (stream u32))
        (type $nan (future f64))
        (import "take-bytes" (func $take-bytes (param "s" $bytes) (result u32)))
        (import "take-pairs" (func $take-pairs (param "s" $pairs) (result u32)))
        (import "take-words" (func $take-words (param "s" $words) (result u32)))
        (import "take-nan" (func $take-nan (param "f" $nan) (result u32)))
        (core module $libc (memory (export "memory") 18))
        (core instance $libc (instantiate $libc))
        (core func $take-bytes (canon lower (func $take-bytes)))
        (core func $take-pairs (canon lower (func $take-pairs)))
        (core func $take-words (canon lower (func $take-words)))
        (core func $take-nan (canon lower (func $take-nan)))
        (core func $new-bytes (canon stream.new $bytes))
        (core func $new-pairs (canon stream.new $pairs))
        (core func $new-words (canon stream.new $words))
        (core func $new-nan (canon future.new $nan))
        (core func $write-bytes
          (canon stream.write $bytes async (memory (core memory $libc "memory"))))
        (core func $write-pairs
          (canon stream.write $pairs async (memory (core memory $libc "memory"))))
        (core func $write-words
          (canon stream.write $words async (memory (core memory $libc "memory"))))
        (core func $write-nan
          (canon future.write $nan async (memory (core memory $libc "memory"))))
        (core module $m
          (import "libc" "memory" (memory 18))
          (import "" "take-bytes" (func $take-bytes (param i32) (result i32)))
          (import "" "take-pairs" (func $take-pairs (param i32) (result i32)))
          (import "" "take-words" (func $take-words (param i32) (result i32)))
          (import "" "take-nan" (func $take-nan (param i32) (result i32)))
          (import "" "new-bytes" (func $new-bytes (result i64)))
          (import "" "new-pairs" (func $new-pairs (result i64)))
          (import "" "new-words" (func $new-words (result i64)))
          (import "" "new-nan" (func $new-nan (result i64)))
          (import "" "write-bytes" (func $write-bytes (param i32 i32 i32) (result i32)))
          (import "" "write-pairs" (func $write-pairs (param i32 i32 i32) (result i32)))
          (import "" "write-words" (func $write-words (param i32 i32 i32) (result i32)))
          (import "" "write-nan" (func $write-nan (param i32 i32) (result i32)))
          (func (export "give-bytes") (param i32) (result i32) (call $take-bytes (local.get 0)))
          (func (export "give-pairs") (param i32) (result i32) (call $take-pairs (local.get 0)))
          (func (export "give-words") (param i32) (result i32) (call $take-words (local.get 0)))
          (func (export "give-nan") (param i32) (result i32) (call $take-nan (local.get 0)))
          (func (export "new-bytes") (result i64) (call $new-bytes))
          (func (export "new-pairs") (result i64) (call $new-pairs))
          (func (export "new-words") (result i64) (call $new-words))
          (func (export "new-nan") (result i64) (call $new-nan))
          (func (export "write-bytes") (param i32 i32 i32) (result i32)
            (call $write-bytes (local.get 0) (local.get 1) (local.get 2)))
          (func (export "write-pairs") (param i32 i32 i32) (result i32)
            (call $write-pairs (local.get 0) (local.get 1) (local.get 2)))
          (func (export "write-words") (param i32 i32 i32) (result i32)
            (call $write-words (local.get 0) (local.get 1) (local.get 2)))
          (func (export "write-nan") (param i32 i32) (result i32)
            (call $write-nan (local.get 0) (local.get 1)))
          (func (export "fill") (param $p i32) (param $n i32)
            (local $i i32)
            (block $done
              (loop $next
                (br_if $done (i32.ge_u (local.get $i) (local.get $n)))
                (i32.store8
                  (i32.add (local.get $p) (local.get $i))
                  (i32.add
                    (i32.mul (local.get $i) (i32.const 7))
                    (i32.shr_u (local.get $i) (i32.const 9))))
                (local.set $i (i32.add (local.get $i) (i32.const 1)))
                (br $next))))
          (func (export "poke") (param i32 i32) (i32.store (local.get 0) (local.get 1))))
        (core instance $i (instantiate $m
          (with "libc" (instance $libc))
          (with "" (instance
            (export "take-bytes" (func $take-bytes))
            (export "take-pairs" (func $take-pairs))
            (export "take-words" (func $take-words))
            (export "take-nan" (func $take-nan))
            (export "new-bytes" (func $new-bytes))
            (export "new-pairs" (func $new-pairs))
            (export "new-words" (func $new-words))
            (export "new-nan" (func $new-nan))
            (export "write-bytes" (func $write-bytes))
            (export "write-pairs" (func $write-pairs))
            (export "write-words" (func $write-words))
            (export "write-nan" (func $write-nan))))))
        (func (export "give-bytes") (param "e" u32) (result u32)
          (canon lift (core func $i "give-bytes")))
        (func (export "give-pairs") (param "e" u32) (result u32)
          (canon lift (core func $i "give-pairs")))
        (func (export "give-words") (param "e" u32) (result u32)
          (canon lift (core func $i "give-words")))
        (func (export "give-nan") (param "e" u32) (result u32)
          (canon lift (core func $i "give-nan")))
        (func (export "new-bytes") (result u64) (canon lift (core func $i "new-bytes")))
        (func (export "new-pairs") (result u64) (canon lift (core func $i "new-pairs")))
        (func (export "new-words") (result u64) (canon lift (core func $i "new-words")))
        (func (export "new-nan") (result u64) (canon lift (core func $i "new-nan")))
        (func (export "write-bytes") (param "e" u32) (param "p" u32) (param "n" u32) (result u32)
          (canon lift (core func $i "write-bytes")))
        (func (export "write-pairs") (param "e" u32) (param "p" u32) (param "n" u32) (result u32)
          (canon lift (core func $i "write-pairs")))
        (func (export "write-words") (param "e" u32) (param "p" u32) (param "n" u32) (result u32)
          (canon lift (core func $i "write-words")))
        (func (export "write-nan") (param "e" u32) (param "p" u32) (result u32)
          (canon lift (core func $i "write-nan")))
        (func (export "fill") (param "p" u32) (param "n" u32) (canon lift (core func $i "fill")))
        (func (export "poke") (param "p" u32) (param "v" u32) (canon lift (core func $i "poke"))))

      (instance $r (instantiate $reader))
      (instance $w (instantiate $writer
        (with "pair" (type $r "pair"))
        (with "take-bytes" (func $r "take-bytes"))
        (with "take-pairs" (func $r "take-pairs"))
        (with "take-words" (func $r "take-words"))
        (with "take-nan" (func $r "take-nan"))))
      (export "read-bytes" (func $r "read-bytes"))
      (export "read-pairs" (func $r "read-pairs"))
      (export "read-words" (func $r "read-words"))
      (export "read-nan" (func $r "read-nan"))
      (export "wait" (func $r "wait"))
      (export "verify" (func $r "verify"))
      (export "peek" (func $r "peek"))
      (export "give-bytes" (func $w "give-bytes"))
      (export "give-pairs" (func $w "give-pairs"))
      (export "give-words" (func $w "give-words"))
      (export "give-nan" (func $w "give-nan"))
      (export "new-bytes" (func $w "new-bytes"))
      (export "new-pairs" (func $w "new-pairs"))
      (export "new-words" (func $w "new-words"))
      (export "new-nan" (func $w "new-nan"))
      (export "write-bytes" (func $w "write-bytes"))
      (export "write-pairs" (func $w "write-pairs"))
      (export "write-words" (func $w "write-words"))
      (export "write-nan" (func $w "write-nan"))
      (export "fill" (func $w "fill"))
      (export "poke" (func $w "poke")))
    "#
);

/// Instantiate `binary` in a fresh store with nothing registered.
async fn instantiate(binary: &[u8]) -> (Store<()>, Instance) {
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, binary)
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

/// The export `name` of `instance`.
fn func(instance: &Instance, name: &str) -> Func {
    instance
        .get_func(name)
        .unwrap_or_else(|| panic!("the component exports `{name}`"))
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
    let values = func(instance, name).call(store, &args).await?;
    Ok(values.first().cloned())
}

/// Call `name` with the numbers `args` and report the one value it
/// returned.
async fn call(store: &mut Store<()>, instance: &Instance, name: &str, args: &[u32]) -> Option<Val> {
    match try_call(store, instance, name, args).await {
        Ok(value) => value,
        Err(error) => panic!("{name} failed: {}", chain(&error)),
    }
}

/// Call `name` and expect it to return one `u32`.
async fn call_u32(store: &mut Store<()>, instance: &Instance, name: &str, args: &[u32]) -> u32 {
    match call(store, instance, name, args).await {
        Some(Val::U32(value)) => value,
        other => panic!("{name} answered {other:?}"),
    }
}

/// Create a stream or future with the writer's `new-<kind>`, hand its
/// readable end to the reader with `give-<kind>`, and answer the
/// writable end's index in the writer and the readable end's in the
/// reader.
async fn stream(store: &mut Store<()>, instance: &Instance, kind: &str) -> (u32, u32) {
    let ends = match call(store, instance, &format!("new-{kind}"), &[]).await {
        Some(Val::U64(packed)) => packed,
        other => panic!("new-{kind} answered {other:?}"),
    };
    let readable = call_u32(store, instance, &format!("give-{kind}"), &[ends as u32]).await;
    ((ends >> 32) as u32, readable)
}

/// The packed result of a copy: `result` in the low four bits and
/// `count` above them.
fn packed(result: u32, count: u32) -> u32 {
    result | (count << 4)
}

#[wcmp_macros::test]
async fn it_copies_a_mebibyte_of_u8_through_one_read_and_one_write_with_no_value_built() {
    let (mut store, instance) = instantiate(PATHS).await;
    call(&mut store, &instance, "fill", &[BUFFER, MEBIBYTE]).await;
    let (writable, readable) = stream(&mut store, &instance, "bytes").await;
    assert_eq!(
        call_u32(
            &mut store,
            &instance,
            "read-bytes",
            &[readable, BUFFER, MEBIBYTE]
        )
        .await,
        BLOCKED,
        "the one read of the whole mebibyte waits for the writer"
    );

    // A copy that built one value per byte would charge a mebibyte of
    // host values against the budget, which a budget of nothing
    // refuses. The byte copy builds none and charges nothing.
    store.set_hostcall_fuel(0);
    let before = memory_accesses();
    let written = call_u32(
        &mut store,
        &instance,
        "write-bytes",
        &[writable, BUFFER, MEBIBYTE],
    )
    .await;
    let after = memory_accesses();
    assert_eq!(
        written,
        packed(0, MEBIBYTE),
        "the write moves the whole mebibyte"
    );
    assert_eq!(
        (after.0 - before.0, after.1 - before.1),
        (1, 1),
        "the copy is one runtime-layer read of the writer's memory and one write of the reader's"
    );

    assert_eq!(
        call_u32(&mut store, &instance, "wait", &[readable]).await,
        packed(0, MEBIBYTE),
        "the one read took the whole mebibyte"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "verify", &[BUFFER, MEBIBYTE]).await,
        0,
        "and every byte the reader holds is the byte the writer wrote"
    );
}

#[wcmp_macros::test]
async fn it_copies_a_record_payload_through_values() {
    let (mut store, instance) = instantiate(PATHS).await;
    // Three records of a `u8` and a `u32`, eight bytes apiece. The
    // bytes between the two fields are padding, which a copy through
    // values does not carry: the writer's are set, and the reader's
    // arrive as zeros.
    for (i, (a, b)) in [(0xa1, 1000), (0xb2, 2000), (0xc3, 3000)]
        .into_iter()
        .enumerate()
    {
        let at = BUFFER + 8 * i as u32;
        call(&mut store, &instance, "poke", &[at, 0xeeee_ee00 | a]).await;
        call(&mut store, &instance, "poke", &[at + 4, b]).await;
    }
    let (writable, readable) = stream(&mut store, &instance, "pairs").await;
    assert_eq!(
        call_u32(&mut store, &instance, "read-pairs", &[readable, BUFFER, 4]).await,
        BLOCKED
    );
    assert_eq!(
        call_u32(&mut store, &instance, "write-pairs", &[writable, BUFFER, 3]).await,
        packed(0, 3)
    );
    assert_eq!(
        call_u32(&mut store, &instance, "wait", &[readable]).await,
        packed(0, 3)
    );
    let mut arrived = Vec::new();
    for i in 0..3 {
        let at = BUFFER + 8 * i;
        arrived.push((
            call_u32(&mut store, &instance, "peek", &[at]).await,
            call_u32(&mut store, &instance, "peek", &[at + 4]).await,
        ));
    }
    assert_eq!(
        arrived,
        [(0xa1, 1000), (0xb2, 2000), (0xc3, 3000)],
        "each field arrived, and the padding did not"
    );
}

#[wcmp_macros::test]
async fn it_charges_the_copy_budget_for_a_record_payload() {
    let (mut store, instance) = instantiate(PATHS).await;
    let (writable, readable) = stream(&mut store, &instance, "pairs").await;
    assert_eq!(
        call_u32(&mut store, &instance, "read-pairs", &[readable, BUFFER, 1]).await,
        BLOCKED
    );
    // The same budget of nothing that a byte copy of a mebibyte
    // passes refuses one record, because a copy through values builds
    // the value it moves.
    store.set_hostcall_fuel(0);
    let error = try_call(&mut store, &instance, "write-pairs", &[writable, BUFFER, 1])
        .await
        .expect_err("a copy through values charges the budget");
    let message = chain(&error);
    assert!(
        message.contains("fuel allocated for hostcalls has been exhausted"),
        "expected the budget cause, got {message}"
    );
}

#[wcmp_macros::test]
async fn it_copies_u32_words_as_bytes_from_where_each_side_has_got_to() {
    let (mut store, instance) = instantiate(PATHS).await;
    let words: Vec<u32> = (1..=6).map(|i| 0x1111_1111 * i).collect();
    for (i, word) in words.iter().enumerate() {
        call(
            &mut store,
            &instance,
            "poke",
            &[BUFFER + 4 * i as u32, *word],
        )
        .await;
    }

    // The write of six is pending. The first read takes two of them,
    // so the second finds the write two values in and must read from
    // there.
    let (writable, readable) = stream(&mut store, &instance, "words").await;
    assert_eq!(
        call_u32(&mut store, &instance, "write-words", &[writable, BUFFER, 6]).await,
        BLOCKED
    );
    assert_eq!(
        call_u32(&mut store, &instance, "read-words", &[readable, BUFFER, 2]).await,
        packed(0, 2)
    );
    assert_eq!(
        call_u32(
            &mut store,
            &instance,
            "read-words",
            &[readable, BUFFER + 8, 4]
        )
        .await,
        packed(0, 4)
    );
    let mut arrived = Vec::new();
    for i in 0..6 {
        arrived.push(call_u32(&mut store, &instance, "peek", &[BUFFER + 4 * i]).await);
    }
    assert_eq!(
        arrived, words,
        "the second read took the writer's third to sixth words"
    );

    // The read of four is pending. The first write gives it one, and
    // the read stays pending until its event is delivered, so the
    // second write finds it one value in and must write from there.
    let (writable, readable) = stream(&mut store, &instance, "words").await;
    let target = BUFFER + 64;
    assert_eq!(
        call_u32(&mut store, &instance, "read-words", &[readable, target, 4]).await,
        BLOCKED
    );
    assert_eq!(
        call_u32(&mut store, &instance, "write-words", &[writable, BUFFER, 1]).await,
        packed(0, 1)
    );
    assert_eq!(
        call_u32(
            &mut store,
            &instance,
            "write-words",
            &[writable, BUFFER + 12, 3]
        )
        .await,
        packed(0, 3)
    );
    assert_eq!(
        call_u32(&mut store, &instance, "wait", &[readable]).await,
        packed(0, 4),
        "the one read took both writes"
    );
    let mut arrived = Vec::new();
    for i in 0..4 {
        arrived.push(call_u32(&mut store, &instance, "peek", &[target + 4 * i]).await);
    }
    assert_eq!(
        arrived,
        [words[0], words[3], words[4], words[5]],
        "the second write landed after the first write's word"
    );
}

#[wcmp_macros::test]
async fn it_carries_the_bits_of_a_nan_with_a_payload_through_a_future_of_f64() {
    // A signalling NaN whose payload is not the canonical one. A copy
    // that quieted or canonicalised NaNs on the way would change
    // these bits.
    const NAN: u64 = 0x7ff4_dead_beef_0001;
    assert!(f64::from_bits(NAN).is_nan());
    let (mut store, instance) = instantiate(PATHS).await;
    call(&mut store, &instance, "poke", &[BUFFER, NAN as u32]).await;
    call(
        &mut store,
        &instance,
        "poke",
        &[BUFFER + 4, (NAN >> 32) as u32],
    )
    .await;

    let (writable, readable) = stream(&mut store, &instance, "nan").await;
    assert_eq!(
        call_u32(&mut store, &instance, "read-nan", &[readable, BUFFER]).await,
        BLOCKED
    );
    assert_eq!(
        call_u32(&mut store, &instance, "write-nan", &[writable, BUFFER]).await,
        packed(0, 0),
        "the write meets the pending read and completes"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "wait", &[readable]).await,
        packed(0, 0)
    );
    let low = call_u32(&mut store, &instance, "peek", &[BUFFER]).await;
    let high = call_u32(&mut store, &instance, "peek", &[BUFFER + 4]).await;
    assert_eq!(
        u64::from(high) << 32 | u64::from(low),
        NAN,
        "the reader holds the writer's NaN bit for bit"
    );
}
