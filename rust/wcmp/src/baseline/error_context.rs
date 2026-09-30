//! Baseline tests for the error-context built-ins.
//!
//! A guest creates an error context with `error-context.new`, reads
//! its debug message back with `error-context.debug-message`, and
//! drops its handle with `error-context.drop`. The store keeps one
//! record per error context, which these tests read to see it enter
//! the store and leave it again. The corpus proves the may-leave
//! rule and the bounds check of the address in Wasmtime's own
//! components; the tests here prove what those components do not
//! reach: the message in every string encoding, the order of the
//! bounds check and `realloc`, a handle of another kind, and the
//! engine gate.

#![cfg(test)]

use crate::internal::FuncInternal;
use crate::store::StoreInternalExt;
use crate::{Component, Engine, EngineConfig, Error, Func, Instance, Linker, Store, Val};
use wcmp_macros::component;

/// A component that declares `error-context.new` and
/// `error-context.debug-message` once in each string encoding, and
/// `error-context.drop`, and exports a function that calls each.
///
/// Its `realloc` is a bump allocator that honours the alignment it is
/// asked for, so the message a read writes lands clear of the bytes
/// the test put the original at. `poke` and `peek` let the host put
/// bytes in the memory and read them back, and `new-set` hands out a
/// handle of another kind.
const ROUND_TRIP: &[u8] = component!(
    r#"
    (component
      (core module $libc
        (memory (export "memory") 1)
        (global $bump (mut i32) (i32.const 4096))
        (func (export "realloc") (param i32 i32 i32 i32) (result i32)
          (local $ret i32)
          (local.set $ret
            (i32.and
              (i32.add (global.get $bump) (i32.sub (local.get 2) (i32.const 1)))
              (i32.sub (i32.const 0) (local.get 2))))
          (global.set $bump (i32.add (local.get $ret) (local.get 3)))
          (local.get $ret))
        (func (export "poke") (param i32 i32) (i32.store8 (local.get 0) (local.get 1)))
        (func (export "peek") (param i32) (result i32) (i32.load8_u (local.get 0)))
        (func (export "peek32") (param i32) (result i32) (i32.load (local.get 0))))
      (core instance $libc (instantiate $libc))

      (core func $new-utf8
        (canon error-context.new (memory (core memory $libc "memory"))))
      (core func $new-utf16
        (canon error-context.new (memory (core memory $libc "memory")) string-encoding=utf16))
      (core func $new-latin1
        (canon error-context.new (memory (core memory $libc "memory"))
          string-encoding=latin1+utf16))
      (core func $read-utf8
        (canon error-context.debug-message (memory (core memory $libc "memory"))
          (realloc (core func $libc "realloc"))))
      (core func $read-utf16
        (canon error-context.debug-message (memory (core memory $libc "memory"))
          (realloc (core func $libc "realloc")) string-encoding=utf16))
      (core func $read-latin1
        (canon error-context.debug-message (memory (core memory $libc "memory"))
          (realloc (core func $libc "realloc")) string-encoding=latin1+utf16))
      (core func $drop (canon error-context.drop))
      (core func $set-new (canon waitable-set.new))

      (core module $m
        (import "" "new-utf8" (func $new-utf8 (param i32 i32) (result i32)))
        (import "" "new-utf16" (func $new-utf16 (param i32 i32) (result i32)))
        (import "" "new-latin1" (func $new-latin1 (param i32 i32) (result i32)))
        (import "" "read-utf8" (func $read-utf8 (param i32 i32)))
        (import "" "read-utf16" (func $read-utf16 (param i32 i32)))
        (import "" "read-latin1" (func $read-latin1 (param i32 i32)))
        (import "" "drop" (func $drop (param i32)))
        (import "" "set-new" (func $set-new (result i32)))
        (func (export "new-utf8") (param i32 i32) (result i32)
          (call $new-utf8 (local.get 0) (local.get 1)))
        (func (export "new-utf16") (param i32 i32) (result i32)
          (call $new-utf16 (local.get 0) (local.get 1)))
        (func (export "new-latin1") (param i32 i32) (result i32)
          (call $new-latin1 (local.get 0) (local.get 1)))
        (func (export "read-utf8") (param i32 i32)
          (call $read-utf8 (local.get 0) (local.get 1)))
        (func (export "read-utf16") (param i32 i32)
          (call $read-utf16 (local.get 0) (local.get 1)))
        (func (export "read-latin1") (param i32 i32)
          (call $read-latin1 (local.get 0) (local.get 1)))
        (func (export "drop") (param i32) (call $drop (local.get 0)))
        (func (export "new-set") (result i32) (call $set-new)))
      (core instance $m (instantiate $m (with "" (instance
        (export "new-utf8" (func $new-utf8))
        (export "new-utf16" (func $new-utf16))
        (export "new-latin1" (func $new-latin1))
        (export "read-utf8" (func $read-utf8))
        (export "read-utf16" (func $read-utf16))
        (export "read-latin1" (func $read-latin1))
        (export "drop" (func $drop))
        (export "set-new" (func $set-new))))))

      (func (export "new-utf8") (param "p" u32) (param "n" u32) (result u32)
        (canon lift (core func $m "new-utf8")))
      (func (export "new-utf16") (param "p" u32) (param "n" u32) (result u32)
        (canon lift (core func $m "new-utf16")))
      (func (export "new-latin1") (param "p" u32) (param "n" u32) (result u32)
        (canon lift (core func $m "new-latin1")))
      (func (export "read-utf8") (param "h" u32) (param "a" u32)
        (canon lift (core func $m "read-utf8")))
      (func (export "read-utf16") (param "h" u32) (param "a" u32)
        (canon lift (core func $m "read-utf16")))
      (func (export "read-latin1") (param "h" u32) (param "a" u32)
        (canon lift (core func $m "read-latin1")))
      (func (export "drop") (param "h" u32) (canon lift (core func $m "drop")))
      (func (export "new-set") (result u32) (canon lift (core func $m "new-set")))
      (func (export "poke") (param "a" u32) (param "b" u32)
        (canon lift (core func $libc "poke")))
      (func (export "peek") (param "a" u32) (result u32)
        (canon lift (core func $libc "peek")))
      (func (export "peek32") (param "a" u32) (result u32)
        (canon lift (core func $libc "peek32"))))
    "#
);

/// A component whose `realloc` traps, so a read that reaches it
/// fails with the `unreachable` trap. `read` creates an error
/// context with a one-byte message and reads it back to the address
/// it is given.
const REALLOC_TRAPS: &[u8] = component!(
    r#"
    (component
      (core module $libc
        (memory (export "memory") 1)
        (func (export "realloc") (param i32 i32 i32 i32) (result i32) unreachable))
      (core instance $libc (instantiate $libc))

      (core func $new (canon error-context.new (memory (core memory $libc "memory"))))
      (core func $read
        (canon error-context.debug-message (memory (core memory $libc "memory"))
          (realloc (core func $libc "realloc"))))

      (core module $m
        (import "" "mem" (memory 1))
        (import "" "new" (func $new (param i32 i32) (result i32)))
        (import "" "read" (func $read (param i32 i32)))
        (func (export "read") (param i32)
          (i32.store8 (i32.const 0) (i32.const 0x61))
          (call $read (call $new (i32.const 0) (i32.const 1)) (local.get 0))))
      (core instance $m (instantiate $m (with "" (instance
        (export "mem" (memory $libc "memory"))
        (export "new" (func $new))
        (export "read" (func $read))))))

      (func (export "read") (param "a" u32) (canon lift (core func $m "read"))))
    "#
);

/// Three components, each declaring one of the three built-ins.
const DECLARES_NEW: &[u8] = component!(
    r#"
    (component
      (core module $libc (memory (export "memory") 1))
      (core instance $libc (instantiate $libc))
      (core func $f (canon error-context.new (memory (core memory $libc "memory")))))
    "#
);

const DECLARES_DEBUG_MESSAGE: &[u8] = component!(
    r#"
    (component
      (core module $libc
        (memory (export "memory") 1)
        (func (export "realloc") (param i32 i32 i32 i32) (result i32) unreachable))
      (core instance $libc (instantiate $libc))
      (core func $f
        (canon error-context.debug-message (memory (core memory $libc "memory"))
          (realloc (core func $libc "realloc")))))
    "#
);

const DECLARES_DROP: &[u8] = component!(
    r#"
    (component
      (core func $f (canon error-context.drop)))
    "#
);

/// An engine with the error-context gate open, or closed.
fn engine(gate: bool) -> Engine {
    let mut config = EngineConfig::new();
    config.wasm_component_model_error_context(gate);
    Engine::with_backend(crate::runtime_layer::test_backend())
        .and_then(|engine| engine.with_config(&config))
        .expect("engine")
}

/// Instantiate `binary` in a fresh store of an engine whose
/// error-context gate is open, with nothing registered.
async fn instantiate(binary: &[u8]) -> (Store<()>, Instance) {
    let engine = engine(true);
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

/// One export of the instance, by name.
fn func(instance: &Instance, name: &str) -> Func {
    instance.get_func(name).expect("the export is declared")
}

/// Call `name` with `args` and report the one value it returned, or
/// the message of the trap it raised.
async fn call(
    store: &mut Store<()>,
    instance: &Instance,
    name: &str,
    args: &[Val],
) -> Result<Option<Val>, String> {
    func(instance, name)
        .call(store, args)
        .await
        .map(|values| values.first().cloned())
        .map_err(|error| chain(&error))
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

/// Call `name` and expect it to return one `u32`.
async fn call_u32(store: &mut Store<()>, instance: &Instance, name: &str, args: &[Val]) -> u32 {
    match call(store, instance, name, args).await {
        Ok(Some(Val::U32(value))) => value,
        other => panic!("{name} answered {other:?}"),
    }
}

/// Call `name` and expect it to trap, reporting the message.
async fn call_trap(store: &mut Store<()>, instance: &Instance, name: &str, args: &[Val]) -> String {
    match call(store, instance, name, args).await {
        Err(message) => message,
        Ok(value) => panic!("{name} returned {value:?} rather than trapping"),
    }
}

/// Put `bytes` in the guest's memory at `address`.
async fn poke(store: &mut Store<()>, instance: &Instance, address: u32, bytes: &[u8]) {
    for (offset, byte) in bytes.iter().enumerate() {
        call(
            store,
            instance,
            "poke",
            &[
                Val::U32(address + offset as u32),
                Val::U32(u32::from(*byte)),
            ],
        )
        .await
        .expect("the byte is stored");
    }
}

/// Read `length` bytes of the guest's memory at `address`.
async fn peek(store: &mut Store<()>, instance: &Instance, address: u32, length: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(length);
    for offset in 0..length as u32 {
        let byte = call_u32(store, instance, "peek", &[Val::U32(address + offset)]).await;
        bytes.push(byte as u8);
    }
    bytes
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

/// Whether the instance's handle table holds an entry at `index`.
fn has_entry(store: &mut Store<()>, instance: &Instance, index: u32) -> bool {
    let table = {
        let export = func(instance, "drop");
        let state = export.abi_state().lock().expect("the instance's ABI state");
        state.handle_tables[export.options().instance]
    };
    let guard = store.internal().tables().lock().expect("handle tables");
    guard.entry(table, index).is_some()
}

/// The UTF-16 bytes of `s`, little-endian, and its count of code
/// units.
fn utf16(s: &str) -> (Vec<u8>, u32) {
    let units: Vec<u16> = s.encode_utf16().collect();
    let bytes = units.iter().flat_map(|unit| unit.to_le_bytes()).collect();
    (bytes, units.len() as u32)
}

/// The high bit of a `latin1+utf16` length word, which marks a
/// UTF-16 string.
const UTF16_TAG: u32 = 1 << 31;

#[wcmp_macros::test]
async fn it_reads_back_the_debug_message_byte_for_byte_in_each_encoding() {
    let (utf16_bytes, utf16_units) = utf16("naïve λ");
    let (tagged_bytes, tagged_units) = utf16("π ≈ 3");
    // Each case is the encoding's pair of built-ins, the bytes of a
    // message in that encoding, and the length word that goes with
    // them. The Latin-1 encoding is tried in both of its
    // representations, and UTF-8 with the empty message as well.
    let cases: [(&str, &str, Vec<u8>, u32); 6] = [
        (
            "new-utf8",
            "read-utf8",
            "tea 🍵 time".as_bytes().to_vec(),
            "tea 🍵 time".len() as u32,
        ),
        ("new-utf8", "read-utf8", Vec::new(), 0),
        ("new-utf16", "read-utf16", utf16_bytes, utf16_units),
        ("new-latin1", "read-latin1", b"caf\xe9".to_vec(), 4),
        (
            "new-latin1",
            "read-latin1",
            tagged_bytes,
            tagged_units | UTF16_TAG,
        ),
        ("new-latin1", "read-latin1", Vec::new(), 0),
    ];

    for (new, read, bytes, length) in cases {
        let (mut store, instance) = instantiate(ROUND_TRIP).await;
        let (message, destination) = (64u32, 512u32);
        poke(&mut store, &instance, message, &bytes).await;

        let handle = call_u32(
            &mut store,
            &instance,
            new,
            &[Val::U32(message), Val::U32(length)],
        )
        .await;
        assert_eq!(
            error_context_count(&mut store),
            1,
            "`{new}` put one record in the store"
        );

        call(
            &mut store,
            &instance,
            read,
            &[Val::U32(handle), Val::U32(destination)],
        )
        .await
        .expect("the message is read back");
        let pointer = call_u32(&mut store, &instance, "peek32", &[Val::U32(destination)]).await;
        let written = call_u32(
            &mut store,
            &instance,
            "peek32",
            &[Val::U32(destination + 4)],
        )
        .await;
        assert_eq!(
            written, length,
            "`{read}` stores the length word `{new}` was given"
        );
        assert_ne!(
            pointer, message,
            "the message is written where `realloc` said, not over the original"
        );
        assert_eq!(
            peek(&mut store, &instance, pointer, bytes.len()).await,
            bytes,
            "`{read}` writes back the bytes `{new}` read"
        );

        call(&mut store, &instance, "drop", &[Val::U32(handle)])
            .await
            .expect("the handle drops");
        assert!(
            !has_entry(&mut store, &instance, handle),
            "the entry left the instance's handle table"
        );
        assert_eq!(
            error_context_count(&mut store),
            0,
            "the record left the store with its last handle"
        );
    }
}

#[wcmp_macros::test]
async fn it_keeps_the_record_until_the_drop() {
    let (mut store, instance) = instantiate(ROUND_TRIP).await;
    poke(&mut store, &instance, 64, b"kept").await;
    let handle = call_u32(
        &mut store,
        &instance,
        "new-utf8",
        &[Val::U32(64), Val::U32(4)],
    )
    .await;

    // Reading the message leaves the record where it was, and so does
    // overwriting the bytes the message was read from.
    poke(&mut store, &instance, 64, b"gone").await;
    call(
        &mut store,
        &instance,
        "read-utf8",
        &[Val::U32(handle), Val::U32(512)],
    )
    .await
    .expect("the message is read back");
    let pointer = call_u32(&mut store, &instance, "peek32", &[Val::U32(512)]).await;
    assert_eq!(peek(&mut store, &instance, pointer, 4).await, b"kept");
    assert_eq!(
        error_context_count(&mut store),
        1,
        "the record is still there"
    );

    call(&mut store, &instance, "drop", &[Val::U32(handle)])
        .await
        .expect("the handle drops");
    assert_eq!(error_context_count(&mut store), 0, "the drop took it away");
}

#[wcmp_macros::test]
async fn it_fails_an_out_of_bounds_address_before_realloc_runs() {
    // The component's `realloc` traps. A read to an address whose
    // eight bytes leave the memory fails with the bounds message and
    // not with that trap, so the check came first.
    let (mut store, instance) = instantiate(REALLOC_TRAPS).await;
    let message = call_trap(&mut store, &instance, "read", &[Val::U32(65532)]).await;
    assert!(
        message.contains("invalid debug message pointer: out of bounds"),
        "the address is refused, got {message}"
    );
    assert!(
        !message.contains("unreachable"),
        "the refusal came before `realloc` ran, got {message}"
    );

    // The same read to an address that fits reaches the `realloc`.
    let (mut store, instance) = instantiate(REALLOC_TRAPS).await;
    let message = call_trap(&mut store, &instance, "read", &[Val::U32(65528)]).await;
    assert!(
        !message.contains("invalid debug message pointer"),
        "an address whose eight bytes fit passes the check, got {message}"
    );
}

#[wcmp_macros::test]
async fn it_fails_a_handle_of_another_kind() {
    for built_in in ["read-utf8", "drop"] {
        let (mut store, instance) = instantiate(ROUND_TRIP).await;
        let set = call_u32(&mut store, &instance, "new-set", &[]).await;
        let args = match built_in {
            "drop" => vec![Val::U32(set)],
            _ => vec![Val::U32(set), Val::U32(512)],
        };
        let message = call_trap(&mut store, &instance, built_in, &args).await;
        assert!(
            message.contains("handle is not an error-context"),
            "`{built_in}` refuses a waitable set, got {message}"
        );
        assert!(
            has_entry(&mut store, &instance, set),
            "`{built_in}` left the set's entry where it was"
        );
    }
}

#[wcmp_macros::test]
async fn it_fails_an_unknown_handle_with_wasmtimes_message() {
    for built_in in ["read-utf8", "drop"] {
        let (mut store, instance) = instantiate(ROUND_TRIP).await;
        let args = match built_in {
            "drop" => vec![Val::U32(7)],
            _ => vec![Val::U32(7), Val::U32(512)],
        };
        let message = call_trap(&mut store, &instance, built_in, &args).await;
        assert!(
            message.contains("unknown handle index 7"),
            "`{built_in}` refuses an index that names nothing, got {message}"
        );
    }
}

#[wcmp_macros::test]
async fn it_fails_a_message_out_of_bounds_with_the_string_bounds_check() {
    let (mut store, instance) = instantiate(ROUND_TRIP).await;
    let message = call_trap(
        &mut store,
        &instance,
        "new-utf8",
        &[Val::U32(65530), Val::U32(16)],
    )
    .await;
    assert!(
        message.contains("string pointer/length out of bounds of memory"),
        "the message is refused as any string out of bounds is, got {message}"
    );
}

#[wcmp_macros::test]
async fn it_refuses_each_built_in_as_unsupported_with_the_gate_off() {
    // The gate is off unless the host opens it, as in Wasmtime, so a
    // default engine refuses the built-ins as a closed one does.
    for (name, binary) in [
        ("error-context.new", DECLARES_NEW),
        ("error-context.debug-message", DECLARES_DEBUG_MESSAGE),
        ("error-context.drop", DECLARES_DROP),
    ] {
        for closed in [
            Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine"),
            engine(false),
        ] {
            let err = Component::new(&closed, binary)
                .await
                .expect_err("the gate is off");
            let expected = format!("`{name}` requires the component model error-context feature");
            assert!(
                matches!(&err, Error::Unsupported { feature } if feature.contains(&expected)),
                "expected `{name}` to be refused under the closed gate, got {err:?}"
            );
        }
        Component::new(&engine(true), binary)
            .await
            .unwrap_or_else(|err| panic!("`{name}` is accepted under the open gate: {err:?}"));
    }
}
