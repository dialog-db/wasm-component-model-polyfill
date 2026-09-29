//! Baseline tests for owned handles that move through a stream.
//!
//! A value a stream carries can hold an owned handle. A copy moves it
//! as a call moves one: the writer's boundary context lifts it out of
//! the writer's handle table, which removes the entry, and the
//! reader's lowers it into the reader's table, which inserts a new
//! entry under the reader's own index. Each handle a copy moves must
//! therefore leave the writer's table and arrive in the reader's once,
//! and a handle the copy did not reach must stay where it was.
//!
//! The tests compose three components. One defines the resource and
//! makes handles of it, one writes a `stream<own<r>>`, and one reads
//! it. The outer component exports the functions of the writer and
//! the reader, so a test drives each side directly and reads each
//! side's handle table.

#![cfg(test)]

use crate::internal::FuncInternal;
use crate::resource::{HandleKind, TableId};
use crate::store::StoreInternalExt;
use crate::{Component, Engine, Error, Func, Instance, Linker, Store, Val};
use wcmp_macros::component;

/// The word a copy returns when it has not finished.
const BLOCKED: u32 = 0xffff_ffff;

/// A definer, a reader, and a writer of a `stream<own<r>>`, composed.
///
/// The definer's `make` returns an owned handle of `r` with the
/// representation it is given. The writer's `make` calls it, so the
/// handle lands in the writer's table, and answers its index there.
/// The writer's `give` passes the readable end at the index it is
/// given to the reader's `take`, which answers the index the end
/// arrived under. `read` and `write` are the asynchronous copy
/// built-ins, `wait` delivers the event of the end it is given and
/// answers its packed result, and `poke` and `peek` reach the
/// writer's and the reader's memory.
const HANDLES: &[u8] = component!(
    r#"
    (component
      (component $definer
        (type $r (resource (rep i32)))
        (core func $new (canon resource.new $r))
        (core module $m
          (import "" "new" (func $new (param i32) (result i32)))
          (func (export "make") (param i32) (result i32) (call $new (local.get 0))))
        (core instance $i (instantiate $m (with "" (instance (export "new" (func $new))))))
        (export $r' "r" (type $r))
        (func (export "make") (param "rep" u32) (result (own $r'))
          (canon lift (core func $i "make"))))

      (component $reader
        (import "r" (type $r (sub resource)))
        (type $s (stream (own $r)))
        (core module $libc (memory (export "memory") 1))
        (core instance $libc (instantiate $libc))
        (core func $read (canon stream.read $s async (memory (core memory $libc "memory"))))
        (core func $set-new (canon waitable-set.new))
        (core func $wait (canon waitable-set.wait (memory (core memory $libc "memory"))))
        (core func $join (canon waitable.join))
        (core module $m
          (import "libc" "memory" (memory 1))
          (import "" "stream.read" (func $read (param i32 i32 i32) (result i32)))
          (import "" "waitable-set.new" (func $set-new (result i32)))
          (import "" "waitable-set.wait" (func $wait (param i32 i32) (result i32)))
          (import "" "waitable.join" (func $join (param i32 i32)))
          (func (export "take") (param i32) (result i32) (local.get 0))
          (func (export "read") (param i32 i32 i32) (result i32)
            (call $read (local.get 0) (local.get 1) (local.get 2)))
          (func (export "wait") (param $end i32) (result i32)
            (local $set i32)
            (local.set $set (call $set-new))
            (call $join (local.get $end) (local.get $set))
            (drop (call $wait (local.get $set) (i32.const 0)))
            (call $join (local.get $end) (i32.const 0))
            (i32.load (i32.const 4)))
          (func (export "peek") (param i32) (result i32) (i32.load (local.get 0))))
        (core instance $i (instantiate $m
          (with "libc" (instance $libc))
          (with "" (instance
            (export "stream.read" (func $read))
            (export "waitable-set.new" (func $set-new))
            (export "waitable-set.wait" (func $wait))
            (export "waitable.join" (func $join))))))
        (func (export "take") (param "s" $s) (result u32) (canon lift (core func $i "take")))
        (func (export "read") (param "e" u32) (param "p" u32) (param "n" u32) (result u32)
          (canon lift (core func $i "read")))
        (func (export "wait") (param "e" u32) (result u32) (canon lift (core func $i "wait")))
        (func (export "peek") (param "p" u32) (result u32) (canon lift (core func $i "peek"))))

      (component $writer
        (import "r" (type $r (sub resource)))
        (type $s (stream (own $r)))
        (import "make" (func $make (param "rep" u32) (result (own $r))))
        (import "take" (func $take (param "s" $s) (result u32)))
        (core module $libc (memory (export "memory") 1))
        (core instance $libc (instantiate $libc))
        (core func $make (canon lower (func $make)))
        (core func $take (canon lower (func $take)))
        (core func $stream-new (canon stream.new $s))
        (core func $write (canon stream.write $s async (memory (core memory $libc "memory"))))
        (core module $m
          (import "libc" "memory" (memory 1))
          (import "" "make" (func $make (param i32) (result i32)))
          (import "" "take" (func $take (param i32) (result i32)))
          (import "" "stream.new" (func $stream-new (result i64)))
          (import "" "stream.write" (func $write (param i32 i32 i32) (result i32)))
          (func (export "make") (param i32) (result i32) (call $make (local.get 0)))
          (func (export "give") (param i32) (result i32) (call $take (local.get 0)))
          (func (export "new-stream") (result i64) (call $stream-new))
          (func (export "write") (param i32 i32 i32) (result i32)
            (call $write (local.get 0) (local.get 1) (local.get 2)))
          (func (export "poke") (param i32 i32) (i32.store (local.get 0) (local.get 1))))
        (core instance $i (instantiate $m
          (with "libc" (instance $libc))
          (with "" (instance
            (export "make" (func $make))
            (export "take" (func $take))
            (export "stream.new" (func $stream-new))
            (export "stream.write" (func $write))))))
        (func (export "make") (param "rep" u32) (result u32) (canon lift (core func $i "make")))
        (func (export "give") (param "e" u32) (result u32) (canon lift (core func $i "give")))
        (func (export "new-stream") (result u64) (canon lift (core func $i "new-stream")))
        (func (export "write") (param "e" u32) (param "p" u32) (param "n" u32) (result u32)
          (canon lift (core func $i "write")))
        (func (export "poke") (param "p" u32) (param "v" u32) (canon lift (core func $i "poke"))))

      (instance $d (instantiate $definer))
      (instance $r (instantiate $reader (with "r" (type $d "r"))))
      (instance $w (instantiate $writer
        (with "r" (type $d "r"))
        (with "make" (func $d "make"))
        (with "take" (func $r "take"))))
      (export "read" (func $r "read"))
      (export "wait" (func $r "wait"))
      (export "peek" (func $r "peek"))
      (export "make" (func $w "make"))
      (export "give" (func $w "give"))
      (export "new-stream" (func $w "new-stream"))
      (export "write" (func $w "write"))
      (export "poke" (func $w "poke")))
    "#
);

/// Instantiate `binary` in a fresh store with nothing registered.
async fn instantiate(binary: &[u8]) -> (Store<()>, Instance) {
    let engine = Engine::new().expect("engine");
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

/// Call `name` with the numbers `args` and report the one value it
/// returned.
async fn call(store: &mut Store<()>, instance: &Instance, name: &str, args: &[u32]) -> Option<Val> {
    let args: Vec<Val> = args.iter().map(|&n| Val::U32(n)).collect();
    match func(instance, name).call(store, &args).await {
        Ok(values) => values.first().cloned(),
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

/// The handle table of the component instance whose export `name`
/// is, read through the instance index the export's canon options
/// name.
fn handle_table(instance: &Instance, name: &str) -> TableId {
    let export = func(instance, name);
    let state = export.abi_state().lock().expect("the instance's ABI state");
    state.handle_tables[export.options().instance]
}

/// The representation of the owned handle at `index` of `table`, or
/// `None` when no owned handle is there.
fn owned_rep(store: &Store<()>, table: TableId, index: u32) -> Option<u32> {
    let guard = store.internal_ref().tables().lock().expect("handle tables");
    match guard.entry(table, index) {
        Some(HandleKind::Own { rep, .. }) => Some(rep),
        _ => None,
    }
}

/// The representations of every owned handle in `table`, in the
/// order of their indices.
fn owned_reps(store: &Store<()>, table: TableId) -> Vec<u32> {
    (0..64)
        .filter_map(|index| owned_rep(store, table, index))
        .collect()
}

/// The packed result of a copy: `result` in the low four bits and
/// `count` above them.
fn packed(result: u32, count: u32) -> u32 {
    result | (count << 4)
}

#[wcmp_macros::test]
async fn it_moves_each_owned_handle_from_the_writers_table_to_the_readers_once() {
    let (mut store, instance) = instantiate(HANDLES).await;
    let writer = handle_table(&instance, "write");
    let reader = handle_table(&instance, "read");
    let mut handles = Vec::new();
    for (slot, rep) in [10, 11, 12].into_iter().enumerate() {
        let handle = call_u32(&mut store, &instance, "make", &[rep]).await;
        call(
            &mut store,
            &instance,
            "poke",
            &[200 + 4 * slot as u32, handle],
        )
        .await;
        handles.push(handle);
    }
    assert_eq!(owned_reps(&store, writer), [10, 11, 12]);
    let ends = match call(&mut store, &instance, "new-stream", &[]).await {
        Some(Val::U64(packed)) => packed,
        other => panic!("new-stream answered {other:?}"),
    };
    let writable = (ends >> 32) as u32;
    let readable = call_u32(&mut store, &instance, "give", &[ends as u32]).await;

    // The read has room for two and the write offers three, so the
    // copy is partial: two handles move and the third stays.
    assert_eq!(
        call_u32(&mut store, &instance, "read", &[readable, 100, 2]).await,
        BLOCKED
    );
    assert_eq!(
        call_u32(&mut store, &instance, "write", &[writable, 200, 3]).await,
        packed(0, 2),
        "the write moves the two the read has room for"
    );
    assert_eq!(owned_rep(&store, writer, handles[0]), None);
    assert_eq!(owned_rep(&store, writer, handles[1]), None);
    assert_eq!(
        owned_rep(&store, writer, handles[2]),
        Some(12),
        "the handle the copy did not reach stays in the writer's table"
    );
    let first = call_u32(&mut store, &instance, "peek", &[100]).await;
    let second = call_u32(&mut store, &instance, "peek", &[104]).await;
    assert_eq!(owned_rep(&store, reader, first), Some(10));
    assert_eq!(owned_rep(&store, reader, second), Some(11));
    assert_eq!(
        call_u32(&mut store, &instance, "wait", &[readable]).await,
        packed(0, 2)
    );

    assert_eq!(
        call_u32(&mut store, &instance, "read", &[readable, 108, 4]).await,
        BLOCKED
    );
    assert_eq!(
        call_u32(&mut store, &instance, "write", &[writable, 208, 1]).await,
        packed(0, 1),
        "a second write moves the third"
    );
    let third = call_u32(&mut store, &instance, "peek", &[108]).await;
    assert_eq!(owned_rep(&store, reader, third), Some(12));

    assert_eq!(
        owned_reps(&store, writer),
        Vec::<u32>::new(),
        "every handle left the writer's table"
    );
    let mut arrived = owned_reps(&store, reader);
    arrived.sort_unstable();
    assert_eq!(
        arrived,
        [10, 11, 12],
        "and each arrived in the reader's once"
    );
}
