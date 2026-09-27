//! Baseline tests for an error context that crosses between two
//! components.
//!
//! `error-context` is a value type: four bytes in memory with an
//! alignment of four, and one `i32` in the flat form, as a handle is.
//! A call between two composed components copies one with the fused
//! adapter's `error-context.transfer` intrinsic, and a copy through a
//! stream copies one through the boundary contexts of its two sides.
//! Either way the sender keeps its handle, the receiver gains one of
//! its own over the same record, and the record's count of handles
//! rises by one. The record leaves the store when the last handle,
//! wherever it is, drops.
//!
//! The tests compose a sender with a receiver. The sender creates
//! each error context and hands it to the receiver in a parameter,
//! inside a record, inside a list, and through two streams: one of
//! error contexts and one of records that hold one. The receiver
//! reads each debug message back and drops each handle.

#![cfg(test)]

use crate::concurrency::ErrorContextId;
use crate::internal::FuncInternal;
use crate::resource::TableId;
use crate::store::{StoreContextInternalExt, StoreInternalExt};
use crate::{Component, Engine, EngineConfig, Error, Func, Instance, Linker, Store, Val};
use wcmp_macros::component;

/// The word a copy returns when it has not finished.
const BLOCKED: u32 = 0xffff_ffff;

/// The message of Wasmtime's `ResourceTableError::Full`.
const TABLE_FULL: &str = "resource table has no free keys";

/// A sender and a receiver of error contexts, composed. The outer
/// component exports every function of both, so a test calls each
/// side directly and reads each side's handle table.
///
/// The receiver's `take-one`, `take-record`, `take-list`,
/// `take-stream`, and `take-records` each answer where the value
/// arrived: the index of the error context, of the one inside the
/// record, of the readable end, or the address of the list in the
/// receiver's memory. `take-record` traps unless the record's other
/// two fields are 7 and 9, and `take-list` unless the list holds two.
/// `message` reads the debug message of the error context at an
/// index back as a string, and `drop` drops it. `read` and
/// `read-records` are the asynchronous copy built-ins, `wait`
/// delivers the event of the end it is given and answers its packed
/// result, and `peek` reads the receiver's memory. Its `realloc` is a
/// bump allocator that honours the alignment it is asked for, because
/// a list lands after debug messages of any length.
///
/// The sender's `new` creates an error context whose debug message is
/// the bytes at the address it is given, which `poke8` puts there.
/// `give-one`, `give-record`, and `give-list` pass error contexts to
/// the receiver in each of those three forms, and `give-stream` and
/// `give-records` pass a readable end. `write` and `write-records`
/// are the asynchronous copy built-ins, and `poke` writes a word of
/// the sender's memory. `new-set` hands out a handle of another kind,
/// and `leak` returns the index it is given to the host as an error
/// context.
const COMPOSED: &[u8] = component!(
    r#"
    (component
      (component $receiver
        (type $rec-def (record (field "tag" u8) (field "context" error-context) (field "tail" u8)))
        (export $rec "rec" (type $rec-def))
        (type $s (stream error-context))
        (type $rs (stream $rec))
        (core module $libc
          (memory (export "memory") 1)
          (global $bump (mut i32) (i32.const 1024))
          (func (export "realloc") (param i32 i32 i32 i32) (result i32)
            (local $ptr i32)
            (local.set $ptr
              (i32.and
                (i32.add (global.get $bump) (i32.sub (local.get 2) (i32.const 1)))
                (i32.sub (i32.const 0) (local.get 2))))
            (global.set $bump (i32.add (local.get $ptr) (local.get 3)))
            (local.get $ptr)))
        (core instance $libc (instantiate $libc))
        (core func $debug-message
          (canon error-context.debug-message (memory (core memory $libc "memory"))
            (realloc (core func $libc "realloc"))))
        (core func $drop (canon error-context.drop))
        (core func $read (canon stream.read $s async (memory (core memory $libc "memory"))))
        (core func $read-records
          (canon stream.read $rs async (memory (core memory $libc "memory"))))
        (core func $set-new (canon waitable-set.new))
        (core func $wait (canon waitable-set.wait (memory (core memory $libc "memory"))))
        (core func $join (canon waitable.join))
        (core module $m
          (import "libc" "memory" (memory 1))
          (import "" "debug-message" (func $debug-message (param i32 i32)))
          (import "" "drop" (func $drop (param i32)))
          (import "" "read" (func $read (param i32 i32 i32) (result i32)))
          (import "" "read-records" (func $read-records (param i32 i32 i32) (result i32)))
          (import "" "waitable-set.new" (func $set-new (result i32)))
          (import "" "waitable-set.wait" (func $wait (param i32 i32) (result i32)))
          (import "" "waitable.join" (func $join (param i32 i32)))
          (func (export "take") (param i32) (result i32) (local.get 0))
          (func (export "take-record") (param i32 i32 i32) (result i32)
            (if (i32.ne (local.get 0) (i32.const 7)) (then unreachable))
            (if (i32.ne (local.get 2) (i32.const 9)) (then unreachable))
            (local.get 1))
          (func (export "take-list") (param i32 i32) (result i32)
            (if (i32.ne (local.get 1) (i32.const 2)) (then unreachable))
            (local.get 0))
          (func (export "message") (param i32) (result i32)
            (call $debug-message (local.get 0) (i32.const 16))
            (i32.const 16))
          (func (export "drop") (param i32) (call $drop (local.get 0)))
          (func (export "read") (param i32 i32 i32) (result i32)
            (call $read (local.get 0) (local.get 1) (local.get 2)))
          (func (export "read-records") (param i32 i32 i32) (result i32)
            (call $read-records (local.get 0) (local.get 1) (local.get 2)))
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
            (export "debug-message" (func $debug-message))
            (export "drop" (func $drop))
            (export "read" (func $read))
            (export "read-records" (func $read-records))
            (export "waitable-set.new" (func $set-new))
            (export "waitable-set.wait" (func $wait))
            (export "waitable.join" (func $join))))))
        (func (export "take-one") (param "e" error-context) (result u32)
          (canon lift (core func $i "take")))
        (func (export "take-record") (param "r" $rec) (result u32)
          (canon lift (core func $i "take-record")))
        (func (export "take-list") (param "l" (list error-context)) (result u32)
          (canon lift (core func $i "take-list")
            (memory (core memory $libc "memory"))
            (realloc (core func $libc "realloc"))))
        (func (export "take-stream") (param "s" $s) (result u32)
          (canon lift (core func $i "take")))
        (func (export "take-records") (param "s" $rs) (result u32)
          (canon lift (core func $i "take")))
        (func (export "message") (param "h" u32) (result string)
          (canon lift (core func $i "message") (memory (core memory $libc "memory"))))
        (func (export "drop") (param "h" u32) (canon lift (core func $i "drop")))
        (func (export "read") (param "e" u32) (param "p" u32) (param "n" u32) (result u32)
          (canon lift (core func $i "read")))
        (func (export "read-records") (param "e" u32) (param "p" u32) (param "n" u32) (result u32)
          (canon lift (core func $i "read-records")))
        (func (export "wait") (param "e" u32) (result u32) (canon lift (core func $i "wait")))
        (func (export "peek") (param "p" u32) (result u32) (canon lift (core func $i "peek"))))

      (component $sender
        (type $rec-def (record (field "tag" u8) (field "context" error-context) (field "tail" u8)))
        (import "rec" (type $rec (eq $rec-def)))
        (type $s (stream error-context))
        (type $rs (stream $rec))
        (import "take-one" (func $take-one (param "e" error-context) (result u32)))
        (import "take-record" (func $take-record (param "r" $rec) (result u32)))
        (import "take-list" (func $take-list (param "l" (list error-context)) (result u32)))
        (import "take-stream" (func $take-stream (param "s" $s) (result u32)))
        (import "take-records" (func $take-records (param "s" $rs) (result u32)))
        (core module $libc (memory (export "memory") 1))
        (core instance $libc (instantiate $libc))
        (core func $new (canon error-context.new (memory (core memory $libc "memory"))))
        (core func $drop (canon error-context.drop))
        (core func $stream-new (canon stream.new $s))
        (core func $records-new (canon stream.new $rs))
        (core func $write (canon stream.write $s async (memory (core memory $libc "memory"))))
        (core func $write-records
          (canon stream.write $rs async (memory (core memory $libc "memory"))))
        (core func $set-new (canon waitable-set.new))
        (core func $take-one (canon lower (func $take-one)))
        (core func $take-record (canon lower (func $take-record)))
        (core func $take-list
          (canon lower (func $take-list) (memory (core memory $libc "memory"))))
        (core func $take-stream (canon lower (func $take-stream)))
        (core func $take-records (canon lower (func $take-records)))
        (core module $m
          (import "libc" "memory" (memory 1))
          (import "" "new" (func $new (param i32 i32) (result i32)))
          (import "" "drop" (func $drop (param i32)))
          (import "" "stream-new" (func $stream-new (result i64)))
          (import "" "records-new" (func $records-new (result i64)))
          (import "" "write" (func $write (param i32 i32 i32) (result i32)))
          (import "" "write-records" (func $write-records (param i32 i32 i32) (result i32)))
          (import "" "set-new" (func $set-new (result i32)))
          (import "" "take-one" (func $take-one (param i32) (result i32)))
          (import "" "take-record" (func $take-record (param i32 i32 i32) (result i32)))
          (import "" "take-list" (func $take-list (param i32 i32) (result i32)))
          (import "" "take-stream" (func $take-stream (param i32) (result i32)))
          (import "" "take-records" (func $take-records (param i32) (result i32)))
          (func (export "new") (param i32 i32) (result i32)
            (call $new (local.get 0) (local.get 1)))
          (func (export "drop") (param i32) (call $drop (local.get 0)))
          (func (export "stream-new") (result i64) (call $stream-new))
          (func (export "records-new") (result i64) (call $records-new))
          (func (export "write") (param i32 i32 i32) (result i32)
            (call $write (local.get 0) (local.get 1) (local.get 2)))
          (func (export "write-records") (param i32 i32 i32) (result i32)
            (call $write-records (local.get 0) (local.get 1) (local.get 2)))
          (func (export "new-set") (result i32) (call $set-new))
          (func (export "give-one") (param i32) (result i32) (call $take-one (local.get 0)))
          (func (export "give-record") (param i32) (result i32)
            (call $take-record (i32.const 7) (local.get 0) (i32.const 9)))
          (func (export "give-list") (param i32 i32) (result i32)
            (i32.store (i32.const 32) (local.get 0))
            (i32.store (i32.const 36) (local.get 1))
            (call $take-list (i32.const 32) (i32.const 2)))
          (func (export "give-stream") (param i32) (result i32)
            (call $take-stream (local.get 0)))
          (func (export "give-records") (param i32) (result i32)
            (call $take-records (local.get 0)))
          (func (export "leak") (param i32) (result i32) (local.get 0))
          (func (export "poke") (param i32 i32) (i32.store (local.get 0) (local.get 1)))
          (func (export "poke8") (param i32 i32) (i32.store8 (local.get 0) (local.get 1))))
        (core instance $i (instantiate $m
          (with "libc" (instance $libc))
          (with "" (instance
            (export "new" (func $new))
            (export "drop" (func $drop))
            (export "stream-new" (func $stream-new))
            (export "records-new" (func $records-new))
            (export "write" (func $write))
            (export "write-records" (func $write-records))
            (export "set-new" (func $set-new))
            (export "take-one" (func $take-one))
            (export "take-record" (func $take-record))
            (export "take-list" (func $take-list))
            (export "take-stream" (func $take-stream))
            (export "take-records" (func $take-records))))))
        (func (export "new") (param "p" u32) (param "n" u32) (result u32)
          (canon lift (core func $i "new")))
        (func (export "release") (param "h" u32) (canon lift (core func $i "drop")))
        (func (export "stream-new") (result u64) (canon lift (core func $i "stream-new")))
        (func (export "records-new") (result u64) (canon lift (core func $i "records-new")))
        (func (export "write") (param "e" u32) (param "p" u32) (param "n" u32) (result u32)
          (canon lift (core func $i "write")))
        (func (export "write-records") (param "e" u32) (param "p" u32) (param "n" u32) (result u32)
          (canon lift (core func $i "write-records")))
        (func (export "new-set") (result u32) (canon lift (core func $i "new-set")))
        (func (export "give-one") (param "h" u32) (result u32)
          (canon lift (core func $i "give-one")))
        (func (export "give-record") (param "h" u32) (result u32)
          (canon lift (core func $i "give-record")))
        (func (export "give-list") (param "a" u32) (param "b" u32) (result u32)
          (canon lift (core func $i "give-list")))
        (func (export "give-stream") (param "e" u32) (result u32)
          (canon lift (core func $i "give-stream")))
        (func (export "give-records") (param "e" u32) (result u32)
          (canon lift (core func $i "give-records")))
        (func (export "leak") (param "h" u32) (result error-context)
          (canon lift (core func $i "leak")))
        (func (export "poke") (param "p" u32) (param "v" u32) (canon lift (core func $i "poke")))
        (func (export "poke8") (param "p" u32) (param "v" u32)
          (canon lift (core func $i "poke8"))))

      (instance $r (instantiate $receiver))
      (instance $s (instantiate $sender
        (with "rec" (type $r "rec"))
        (with "take-one" (func $r "take-one"))
        (with "take-record" (func $r "take-record"))
        (with "take-list" (func $r "take-list"))
        (with "take-stream" (func $r "take-stream"))
        (with "take-records" (func $r "take-records"))))
      (export "message" (func $r "message"))
      (export "drop" (func $r "drop"))
      (export "read" (func $r "read"))
      (export "read-records" (func $r "read-records"))
      (export "wait" (func $r "wait"))
      (export "peek" (func $r "peek"))
      (export "new" (func $s "new"))
      (export "release" (func $s "release"))
      (export "stream-new" (func $s "stream-new"))
      (export "records-new" (func $s "records-new"))
      (export "write" (func $s "write"))
      (export "write-records" (func $s "write-records"))
      (export "new-set" (func $s "new-set"))
      (export "give-one" (func $s "give-one"))
      (export "give-record" (func $s "give-record"))
      (export "give-list" (func $s "give-list"))
      (export "give-stream" (func $s "give-stream"))
      (export "give-records" (func $s "give-records"))
      (export "leak" (func $s "leak"))
      (export "poke" (func $s "poke"))
      (export "poke8" (func $s "poke8")))
    "#
);

/// Instantiate `COMPOSED` in a fresh store of an engine whose
/// error-context gate is open, with nothing registered.
async fn instantiate() -> (Store<()>, Instance) {
    let mut config = EngineConfig::new();
    config.wasm_component_model_error_context(true);
    let engine = Engine::with_config(&config).expect("engine");
    let component = Component::new(&engine, COMPOSED)
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

/// Every message in an error's source chain, joined so that a trap a
/// built-in raised can be matched wherever the substrate put it.
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
/// returned, or every message in the chain of the error it failed
/// with.
async fn try_call(
    store: &mut Store<()>,
    instance: &Instance,
    name: &str,
    args: &[u32],
) -> Result<Option<Val>, String> {
    let args: Vec<Val> = args.iter().map(|&n| Val::U32(n)).collect();
    func(instance, name)
        .call(store, &args)
        .await
        .map(|values| values.first().cloned())
        .map_err(|error| chain(&error))
}

/// Call `name` and expect it to succeed.
async fn call(store: &mut Store<()>, instance: &Instance, name: &str, args: &[u32]) -> Option<Val> {
    try_call(store, instance, name, args)
        .await
        .unwrap_or_else(|message| panic!("{name} failed: {message}"))
}

/// Call `name` and expect it to return one `u32`.
async fn call_u32(store: &mut Store<()>, instance: &Instance, name: &str, args: &[u32]) -> u32 {
    match call(store, instance, name, args).await {
        Some(Val::U32(value)) => value,
        other => panic!("{name} answered {other:?}"),
    }
}

/// Call `name` and expect it to trap, reporting the message.
async fn call_trap(store: &mut Store<()>, instance: &Instance, name: &str, args: &[u32]) -> String {
    match try_call(store, instance, name, args).await {
        Err(message) => message,
        Ok(value) => panic!("{name} returned {value:?} rather than trapping"),
    }
}

/// The debug message of the error context at `index` of the
/// receiver's table, as the receiver reads it.
async fn message(store: &mut Store<()>, instance: &Instance, index: u32) -> String {
    match call(store, instance, "message", &[index]).await {
        Some(Val::String(message)) => message,
        other => panic!("message answered {other:?}"),
    }
}

/// Create an error context in the sender whose debug message is
/// `text`, and answer its index in the sender's table.
async fn new_context(store: &mut Store<()>, instance: &Instance, text: &str) -> u32 {
    for (offset, byte) in text.bytes().enumerate() {
        call(
            store,
            instance,
            "poke8",
            &[64 + offset as u32, u32::from(byte)],
        )
        .await;
    }
    call_u32(store, instance, "new", &[64, text.len() as u32]).await
}

/// Make a stream in the sender with `new`, hand its readable end to
/// the receiver with `give`, and answer the sender's writable end and
/// the receiver's readable end.
async fn stream(store: &mut Store<()>, instance: &Instance, new: &str, give: &str) -> (u32, u32) {
    let ends = match call(store, instance, new, &[]).await {
        Some(Val::U64(packed)) => packed,
        other => panic!("{new} answered {other:?}"),
    };
    let readable = call_u32(store, instance, give, &[ends as u32]).await;
    ((ends >> 32) as u32, readable)
}

/// The packed result of a copy: `result` in the low four bits and
/// `count` above them.
fn packed(result: u32, count: u32) -> u32 {
    result | (count << 4)
}

/// The handle table of the component instance whose export `name`
/// is, read through the instance index the export's canon options
/// name.
fn handle_table(instance: &Instance, name: &str) -> TableId {
    let export = func(instance, name);
    let state = export.abi_state().lock().expect("the instance's ABI state");
    state.handle_tables[export.options().instance]
}

/// The error context the entry at `index` of `table` names.
fn context_at(store: &mut Store<()>, table: TableId, index: u32) -> ErrorContextId {
    store
        .internal()
        .tables()
        .lock()
        .expect("handle tables")
        .error_context_from_handle(table, index)
        .expect("the entry is an error context")
}

/// How many handles name the error context `context`, or `None` once
/// its record has left the store.
fn handle_count(store: &mut Store<()>, context: ErrorContextId) -> Option<u32> {
    store
        .internal()
        .tables()
        .lock()
        .expect("handle tables")
        .tasks
        .error_context(context)
        .map(|record| record.handle_count)
}

/// How many error-context records the store holds.
fn error_context_count(store: &mut Store<()>) -> usize {
    store
        .internal()
        .tables()
        .lock()
        .expect("handle tables")
        .tasks
        .error_context_count()
}

/// How many records count against the cap now.
fn record_count(store: &mut Store<()>) -> usize {
    store
        .internal()
        .lock_tables()
        .expect("handle tables")
        .tasks
        .record_count()
}

#[wcmp_macros::test]
async fn it_sends_an_error_context_in_a_parameter_a_list_and_a_stream() {
    let (mut store, instance) = instantiate().await;
    let sender = handle_table(&instance, "new");
    let handle = new_context(&mut store, &instance, "disk full").await;
    let context = context_at(&mut store, sender, handle);
    assert_eq!(handle_count(&mut store, context), Some(1));

    // In a parameter.
    let one = call_u32(&mut store, &instance, "give-one", &[handle]).await;
    assert_eq!(message(&mut store, &instance, one).await, "disk full");
    assert_eq!(
        handle_count(&mut store, context),
        Some(2),
        "the receiver's handle is one more, and the sender keeps its own"
    );

    // Twice in a list.
    let list = call_u32(&mut store, &instance, "give-list", &[handle, handle]).await;
    let mut received = vec![one];
    for address in [list, list + 4] {
        let index = call_u32(&mut store, &instance, "peek", &[address]).await;
        assert_eq!(message(&mut store, &instance, index).await, "disk full");
        received.push(index);
    }
    assert_eq!(handle_count(&mut store, context), Some(4));

    // Twice as the payload of a stream.
    let (writable, readable) = stream(&mut store, &instance, "stream-new", "give-stream").await;
    assert_eq!(
        call_u32(&mut store, &instance, "read", &[readable, 100, 2]).await,
        BLOCKED
    );
    call(&mut store, &instance, "poke", &[200, handle]).await;
    call(&mut store, &instance, "poke", &[204, handle]).await;
    assert_eq!(
        call_u32(&mut store, &instance, "write", &[writable, 200, 2]).await,
        packed(0, 2),
        "the write copies both"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "wait", &[readable]).await,
        packed(0, 2)
    );
    for address in [100, 104] {
        let index = call_u32(&mut store, &instance, "peek", &[address]).await;
        assert_eq!(message(&mut store, &instance, index).await, "disk full");
        received.push(index);
    }
    assert_eq!(handle_count(&mut store, context), Some(6));
    let mut distinct = received.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        received.len(),
        "each crossing gave the receiver a handle of its own: {received:?}"
    );

    // The sender drops its handle first: the receiver's keep the
    // record in the store until the last of them drops.
    call(&mut store, &instance, "release", &[handle]).await;
    assert_eq!(handle_count(&mut store, context), Some(5));
    let (last, rest) = received.split_last().expect("five handles");
    for index in rest {
        call(&mut store, &instance, "drop", &[*index]).await;
    }
    assert_eq!(error_context_count(&mut store), 1, "one handle is left");
    assert_eq!(message(&mut store, &instance, *last).await, "disk full");
    call(&mut store, &instance, "drop", &[*last]).await;
    assert_eq!(
        error_context_count(&mut store),
        0,
        "the record left the store with its last handle"
    );
}

#[wcmp_macros::test]
async fn it_lays_out_an_error_context_inside_a_record_and_a_list() {
    let (mut store, instance) = instantiate().await;
    let handle = new_context(&mut store, &instance, "timeout").await;

    // In the flat form a record of a byte, an error context, and a
    // byte is three `i32`s, and the receiver traps unless the two
    // bytes arrive around the context in their places.
    let flat = call_u32(&mut store, &instance, "give-record", &[handle]).await;
    assert_eq!(message(&mut store, &instance, flat).await, "timeout");

    // In memory the record takes twelve bytes: the byte at 0, the
    // context at 4, the byte at 8. The stream copy lays it out on
    // both sides, so each field lands in the receiver where the
    // sender put it.
    let (writable, readable) = stream(&mut store, &instance, "records-new", "give-records").await;
    assert_eq!(
        call_u32(&mut store, &instance, "read-records", &[readable, 100, 2]).await,
        BLOCKED
    );
    for (base, tag, tail) in [(200, 7, 9), (212, 3, 5)] {
        call(&mut store, &instance, "poke", &[base, tag]).await;
        call(&mut store, &instance, "poke", &[base + 4, handle]).await;
        call(&mut store, &instance, "poke", &[base + 8, tail]).await;
    }
    assert_eq!(
        call_u32(&mut store, &instance, "write-records", &[writable, 200, 2]).await,
        packed(0, 2)
    );
    for (base, tag, tail) in [(100, 7, 9), (112, 3, 5)] {
        assert_eq!(call_u32(&mut store, &instance, "peek", &[base]).await, tag);
        assert_eq!(
            call_u32(&mut store, &instance, "peek", &[base + 8]).await,
            tail
        );
        let index = call_u32(&mut store, &instance, "peek", &[base + 4]).await;
        assert_eq!(message(&mut store, &instance, index).await, "timeout");
    }

    // A list of two is two contexts four bytes apart.
    let list = call_u32(&mut store, &instance, "give-list", &[handle, handle]).await;
    for address in [list, list + 4] {
        let index = call_u32(&mut store, &instance, "peek", &[address]).await;
        assert_eq!(message(&mut store, &instance, index).await, "timeout");
    }
}

#[wcmp_macros::test]
async fn it_fails_a_lift_of_a_handle_of_another_kind() {
    // In a parameter, the adapter's transfer refuses the entry.
    let (mut store, instance) = instantiate().await;
    let set = call_u32(&mut store, &instance, "new-set", &[]).await;
    let message = call_trap(&mut store, &instance, "give-one", &[set]).await;
    assert!(
        message.contains("handle is not an error-context"),
        "the transfer refuses a waitable set, got {message}"
    );

    // Through a stream, the writer's lift refuses it.
    let (mut store, instance) = instantiate().await;
    let set = call_u32(&mut store, &instance, "new-set", &[]).await;
    let (writable, readable) = stream(&mut store, &instance, "stream-new", "give-stream").await;
    assert_eq!(
        call_u32(&mut store, &instance, "read", &[readable, 100, 1]).await,
        BLOCKED
    );
    call(&mut store, &instance, "poke", &[200, set]).await;
    let message = call_trap(&mut store, &instance, "write", &[writable, 200, 1]).await;
    assert!(
        message.contains("handle is not an error-context"),
        "the copy refuses a waitable set, got {message}"
    );
}

#[wcmp_macros::test]
async fn it_fails_a_count_past_u32_max_with_the_reference_count_overflow() {
    for through_stream in [false, true] {
        let (mut store, instance) = instantiate().await;
        let sender = handle_table(&instance, "new");
        let handle = new_context(&mut store, &instance, "full").await;
        let context = context_at(&mut store, sender, handle);
        store
            .internal()
            .tables()
            .lock()
            .expect("handle tables")
            .tasks
            .error_context_mut(context)
            .expect("the record")
            .handle_count = u32::MAX;

        let message = if through_stream {
            let (writable, readable) =
                stream(&mut store, &instance, "stream-new", "give-stream").await;
            assert_eq!(
                call_u32(&mut store, &instance, "read", &[readable, 100, 1]).await,
                BLOCKED
            );
            call(&mut store, &instance, "poke", &[200, handle]).await;
            call_trap(&mut store, &instance, "write", &[writable, 200, 1]).await
        } else {
            call_trap(&mut store, &instance, "give-one", &[handle]).await
        };
        assert!(
            message.contains("reference count overflow"),
            "a handle past the count fails (through a stream: {through_stream}), got {message}"
        );
        assert_eq!(
            handle_count(&mut store, context),
            Some(u32::MAX),
            "the count stays where it was"
        );
    }
}

#[wcmp_macros::test]
async fn it_counts_error_context_records_toward_the_record_cap() {
    let (mut store, instance) = instantiate().await;
    let live = record_count(&mut store);
    let handle = new_context(&mut store, &instance, "one").await;
    assert_eq!(
        record_count(&mut store),
        live + 1,
        "the error context is one more record"
    );
    call_u32(&mut store, &instance, "give-one", &[handle]).await;
    assert_eq!(
        record_count(&mut store),
        live + 1,
        "a crossing adds a handle, not a record"
    );

    // Room for the call's task and thread, and for nothing else.
    let full = record_count(&mut store);
    store
        .internal()
        .context()
        .internal()
        .set_max_records(full + 2)
        .expect("the cap is set");
    let message = call_trap(&mut store, &instance, "new", &[64, 3]).await;
    assert!(
        message.contains(TABLE_FULL),
        "a new error context past the cap fails with Wasmtime's message, got {message}"
    );
    assert_eq!(error_context_count(&mut store), 1, "no record was created");
}

#[wcmp_macros::test]
async fn it_refuses_an_error_context_that_crosses_to_the_host() {
    let (mut store, instance) = instantiate().await;
    let handle = new_context(&mut store, &instance, "private").await;
    let message = call_trap(&mut store, &instance, "leak", &[handle]).await;
    assert!(
        message.contains("`error-context` values that cross between the host and a guest"),
        "the host sees no error context, got {message}"
    );
}
