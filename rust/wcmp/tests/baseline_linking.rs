//! Baseline tests for linking, instantiation, and host integration as
//! `wasm_component_layer` supports them today. Each test is a stub: see
//! PDD003's "Linking, Instantiation, and Host Integration" row. Async host
//! functions, async resource destructors, host-binding code generation, and
//! component-level `start` live in a separate, forthcoming test file.

#![cfg(test)]

use std::sync::{Arc, Mutex};
use wcmp::{
    Accessor, Component, Engine, Error, ExternType, ExternalName, FunctionParameter, FunctionType,
    HostCall, HostResource, InterfaceIdentifier, LinkError, Linker, Module, PrimitiveType,
    ResourceType, Store, Val, ValueType,
};
use wcmp_macros::{component, wasm};

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

#[wcmp_macros::test]
async fn it_constructs_an_engine() {
    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");

    // Engines are advertised as cheap to clone; exercise that.
    let _clone = engine.clone();
}

#[wcmp_macros::test]
async fn it_constructs_a_store() {
    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");

    // Construct against an engine; confirm the host-data slot is reachable
    // through `data` and `data_mut`.
    let mut store: Store<u32> = Store::new(&engine, 7).expect("store construction succeeds");
    assert_eq!(*store.data(), 7);

    *store.data_mut() = 42;
    assert_eq!(*store.data(), 42);
}

#[wcmp_macros::test]
async fn it_loads_a_component_from_bytes() {
    const GREETER: &[u8] = component!(
        r#"
        (component
          (type $iface (instance
            (export "greet" (func (param "who" string) (result string)))))
          (import "wasi:cli/run@0.2.0" (instance $i (type $iface)))
          (core module $m
            (func (export "do-greet") (param i32) (result i32) local.get 0))
          (core instance $c (instantiate $m))
          (func (export "do-greet") (param "n" u32) (result u32)
            (canon lift (core func $c "do-greet"))))
        "#
    );

    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let component = Component::new(&engine, GREETER)
        .await
        .expect("component parses");

    assert_eq!(component.imports.len(), 1);
    assert_eq!(component.exports.len(), 1);

    // The interface-named import is parsed into the polyfill's
    // identifier types.
    let import = &component.imports[0];
    let interface = match &import.name {
        ExternalName::Interface(id) => id,
        other => panic!("expected an interface-named import, got {other:?}"),
    };
    assert_eq!(interface.package().namespace(), "wasi");
    assert_eq!(interface.package().name(), "cli");
    assert_eq!(interface.name(), "run");

    // The import itself is the imported instance; its single item
    // is the `greet` function whose type the polyfill resolved to a
    // function.
    let instance = match &import.ty {
        ExternType::Instance(i) => i,
        other => panic!("expected an instance import, got {other:?}"),
    };
    assert_eq!(instance.items.len(), 1);
    assert_eq!(instance.items[0].name, "greet");
    assert!(matches!(instance.items[0].ty, ExternType::Function(_)));

    // The export is a lifted function published under a plain
    // (kebab-case) name.
    let export = &component.exports[0];
    assert_eq!(export.name, ExternalName::Plain("do-greet".to_owned()));
    assert!(matches!(export.ty, ExternType::Function(_)));
}

#[wcmp_macros::test]
async fn it_instantiates_a_component_through_a_linker() {
    const ADDER: &[u8] = component!(
        r#"
        (component
          (core module $m
            (func (export "add") (param i32 i32) (result i32)
              local.get 0 local.get 1 i32.add))
          (core instance $i (instantiate $m))
          (func (export "add") (param "a" s32) (param "b" s32) (result s32)
            (canon lift (core func $i "add"))))
        "#
    );

    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let component = Component::new(&engine, ADDER)
        .await
        .expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");

    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiation succeeds");
    let add = instance.get_func("add").expect("`add` export is present");

    let results = add
        .call(&mut store, &[Val::S32(2), Val::S32(3)])
        .await
        .expect("call succeeds");
    assert_eq!(results.as_ref(), &[Val::S32(5)]);
}

#[wcmp_macros::test]
async fn it_supports_multiple_independent_instances() {
    // A component with mutable per-instance state: each instance
    // owns its own `counter` global, so two instances of the same
    // component must observe independent monotonic counts.
    const COUNTER: &[u8] = component!(
        r#"
        (component
          (core module $m
            (global $counter (mut i32) (i32.const 0))
            (func (export "next") (result i32)
              global.get $counter
              i32.const 1
              i32.add
              global.set $counter
              global.get $counter))
          (core instance $i (instantiate $m))
          (func (export "next") (result s32)
            (canon lift (core func $i "next"))))
        "#
    );

    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let component = Component::new(&engine, COUNTER)
        .await
        .expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");

    let first = linker
        .instantiate(&mut store, &component)
        .await
        .expect("first instantiation succeeds");
    let second = linker
        .instantiate(&mut store, &component)
        .await
        .expect("second instantiation succeeds");

    let first_next = first
        .get_func("next")
        .expect("`next` export present on first instance");
    let second_next = second
        .get_func("next")
        .expect("`next` export present on second instance");

    assert_eq!(
        first_next
            .call(&mut store, &[])
            .await
            .expect("first call")
            .as_ref(),
        &[Val::S32(1)],
    );
    assert_eq!(
        first_next
            .call(&mut store, &[])
            .await
            .expect("second call")
            .as_ref(),
        &[Val::S32(2)],
    );

    // The second instance's counter is unaffected by the first
    // instance's mutations.
    assert_eq!(
        second_next
            .call(&mut store, &[])
            .await
            .expect("third call")
            .as_ref(),
        &[Val::S32(1)],
    );
}

#[wcmp_macros::test]
async fn it_resolves_package_and_interface_identifiers_with_semver() {
    // The component imports an empty interface qualified with a
    // versioned identifier. The linker registers a candidate whose
    // version falls in the WIT-spec compatibility range.
    // Resolution succeeds and instantiation completes — the import
    // contributes no host items and the component's exports use
    // only its own internal core module.
    const HARNESS: &[u8] = component!(
        r#"
        (component
          (type $iface (instance))
          (import "wasi:cli/run@0.2.0" (instance (type $iface)))
          (core module $m
            (func (export "answer") (result i32)
              i32.const 42))
          (core instance $i (instantiate $m))
          (func (export "answer") (result s32)
            (canon lift (core func $i "answer"))))
        "#
    );

    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let component = Component::new(&engine, HARNESS)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);

    // Register a candidate whose patch version is higher than the
    // import's; WIT compatibility range matches and the linker
    // chooses the highest-version match.
    let candidate: InterfaceIdentifier = "wasi:cli/run@0.2.7".parse().expect("identifier parses");
    let _ = linker.instance(&candidate);

    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiation with semver-qualified import succeeds");
    let answer = instance
        .get_func("answer")
        .expect("`answer` export present");
    assert_eq!(
        answer
            .call(&mut store, &[])
            .await
            .expect("call succeeds")
            .as_ref(),
        &[Val::S32(42)],
    );
}

#[wcmp_macros::test]
async fn it_defines_an_untyped_host_function() {
    // The component imports a doubling function from
    // `pdd008:host/maths@0.1.0` and re-exports a function that
    // calls it. The host registers the doubler via the untyped
    // (`Val`-based) API.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (type $iface (instance
            (export "double" (func (param "n" s32) (result s32)))))
          (import "pdd008:host/maths@0.1.0" (instance $imports (type $iface)))
          (alias export $imports "double" (func $double))
          (core func $core-double (canon lower (func $double)))
          (core module $m
            (func (import "host" "double") (param i32) (result i32))
            (func (export "call-double") (param i32) (result i32)
              local.get 0
              call 0))
          (core instance $i (instantiate $m
            (with "host" (instance
              (export "double" (func $core-double))))))
          (func (export "do-double") (param "n" s32) (result s32)
            (canon lift (core func $i "call-double"))))
        "#
    );

    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd008:host/maths@0.1.0"
        .parse()
        .expect("identifier parses");
    let mut instance = linker.instance(&iface);
    instance
        .func_new(
            "double",
            FunctionType {
                parameters: vec![FunctionParameter {
                    name: "n".to_owned(),
                    ty: ValueType::Primitive(PrimitiveType::S32),
                }],
                result: Some(ValueType::Primitive(PrimitiveType::S32)),
                async_: false,
            },
            |_data, args, results| {
                let Val::S32(n) = args[0] else {
                    panic!("expected s32 arg");
                };
                results[0] = Val::S32(n * 2);
                Ok(())
            },
        )
        .expect("the registration");

    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    let inst = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiation succeeds");
    let do_double = inst
        .get_func("do-double")
        .expect("`do-double` export present");
    let results = do_double
        .call(&mut store, &[Val::S32(21)])
        .await
        .expect("call succeeds");
    assert_eq!(results.as_ref(), &[Val::S32(42)]);
}

#[wcmp_macros::test]
async fn it_defines_a_typed_host_function() {
    // Same component shape as the untyped test, but the host
    // registers via `func_wrap` with statically-typed Rust args and
    // return. Mismatching the signature surfaces a `TypeMismatch`
    // at link time.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (type $iface (instance
            (export "double" (func (param "n" s32) (result s32)))))
          (import "pdd008:host/maths@0.1.0" (instance $imports (type $iface)))
          (alias export $imports "double" (func $double))
          (core func $core-double (canon lower (func $double)))
          (core module $m
            (func (import "host" "double") (param i32) (result i32))
            (func (export "call-double") (param i32) (result i32)
              local.get 0
              call 0))
          (core instance $i (instantiate $m
            (with "host" (instance
              (export "double" (func $core-double))))))
          (func (export "do-double") (param "n" s32) (result s32)
            (canon lift (core func $i "call-double"))))
        "#
    );

    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");

    // Happy path: the typed registration agrees with the import.
    let mut linker: Linker<()> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd008:host/maths@0.1.0"
        .parse()
        .expect("identifier parses");
    let mut instance = linker.instance(&iface);
    instance
        .func_wrap(
            "double",
            |_data: HostCall<'_, ()>, (n,): (i32,)| -> wcmp::Result<i32> { Ok(n * 2) },
        )
        .expect("the registration");

    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    let inst = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiation succeeds");
    let do_double = inst
        .get_func("do-double")
        .expect("`do-double` export present");
    let results = do_double
        .call(&mut store, &[Val::S32(21)])
        .await
        .expect("call succeeds");
    assert_eq!(results.as_ref(), &[Val::S32(42)]);

    // Type-mismatch path: registering an i64-returning closure
    // against an s32-returning import surfaces a TypeMismatch at
    // link time.
    let mut bad_linker: Linker<()> = Linker::new(&engine);
    let mut bad_instance = bad_linker.instance(&iface);
    bad_instance
        .func_wrap(
            "double",
            |_data: HostCall<'_, ()>, (n,): (i32,)| -> wcmp::Result<i64> { Ok(i64::from(n * 2)) },
        )
        .expect("the registration");

    let mut bad_store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    let err = match bad_linker.instantiate(&mut bad_store, &component).await {
        Ok(_) => panic!("type mismatch should have been caught at link time"),
        Err(err) => err,
    };
    assert!(
        matches!(err, Error::TypeMismatch(_)),
        "expected Error::TypeMismatch, got {err:?}",
    );
}

#[wcmp_macros::test]
async fn it_defines_a_host_resource_with_a_sync_destructor() {
    // The component imports a resource type `thing` from a host
    // interface and re-exports a function that takes ownership of a
    // handle and drops it. The host registers the resource with a
    // synchronous destructor that bumps a shared counter; observing
    // the counter after the call asserts that the destructor ran
    // exactly once.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (import "pdd009:host/resources@0.1.0" (instance $i
            (export "thing" (type (sub resource)))))
          (alias export $i "thing" (type $thing))
          (core func $thing-drop (canon resource.drop $thing))
          (core module $m
            (func (import "host" "drop") (param i32))
            (func (export "consume") (param i32)
              local.get 0
              call 0))
          (core instance $core (instantiate $m
            (with "host" (instance
              (export "drop" (func $thing-drop))))))
          (func (export "consume") (param "h" (own $thing))
            (canon lift (core func $core "consume"))))
        "#
    );

    #[derive(Default)]
    struct HostData {
        dropped: Arc<Mutex<Vec<u32>>>,
    }

    let dropped = Arc::new(Mutex::new(Vec::<u32>::new()));
    let host_data = HostData {
        dropped: dropped.clone(),
    };

    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let mut linker: Linker<HostData> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd009:host/resources@0.1.0"
        .parse()
        .expect("identifier parses");
    let mut linker_iface = linker.instance(&iface);
    let type_id = linker_iface
        .resource("thing", |data: &mut HostData, rep: u32| {
            data.dropped.lock().expect("dropped lock").push(rep);
            Ok(())
        })
        .expect("the registration");

    let mut store: Store<HostData> = Store::new(&engine, host_data).expect("store construction");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiation succeeds");
    let consume = instance
        .get_func("consume")
        .expect("`consume` export present");

    // Mint a handle for a host-side resource (rep `42` is opaque to
    // the polyfill — it is whatever value the host wants the
    // destructor to receive) and pass ownership to the guest.
    let handle = store
        .resource_new(type_id, 42)
        .expect("resource_new succeeds");
    let results = consume
        .call(&mut store, &[Val::Own(handle)])
        .await
        .expect("call succeeds");
    assert!(results.is_empty(), "consume returns no values");

    // The destructor observed the drop exactly once with the
    // host-supplied rep.
    let observed: Vec<u32> = dropped.lock().expect("dropped lock").clone();
    assert_eq!(observed, vec![42]);
}

#[wcmp_macros::test]
async fn it_invokes_an_exported_component_function() {
    // End-to-end smoke test exercising the canonical ABI's
    // string round-trip on both argument and result. The component
    // exports a function that takes a string and returns its
    // length as an s32 — proving the polyfill can lower a host
    // string into guest memory via cabi_realloc, hand it to the
    // guest, lift the s32 result, and run post-return.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (core module $m
            (memory (export "memory") 1)
            (global $bump (mut i32) (i32.const 16))
            (func (export "cabi_realloc")
                  (param i32 i32 i32 i32) (result i32)
              (local $ptr i32)
              global.get $bump
              local.set $ptr
              global.get $bump
              local.get 3
              i32.add
              global.set $bump
              local.get $ptr)
            (func (export "string-length") (param i32 i32) (result i32)
              local.get 1))
          (core instance $i (instantiate $m))
          (func (export "string-length") (param "s" string) (result s32)
            (canon lift (core func $i "string-length")
                       (memory (core memory $i "memory"))
                       (realloc (core func $i "cabi_realloc")))))
        "#
    );

    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiation succeeds");
    let string_length = instance
        .get_func("string-length")
        .expect("`string-length` export present");
    let results = string_length
        .call(&mut store, &[Val::String("hello, world".to_owned())])
        .await
        .expect("call succeeds");
    assert_eq!(results.as_ref(), &[Val::S32(12)]);
}

#[wcmp_macros::test]
async fn it_dispatches_to_multiple_host_functions_in_one_interface() {
    // The component imports two host functions from the same
    // interface and re-exports a function that calls them in
    // sequence. The host registers both; both must be dispatched.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (type $iface (instance
            (export "incr" (func (param "n" s32) (result s32)))
            (export "decr" (func (param "n" s32) (result s32)))))
          (import "pdd008-tests:host/maths@0.1.0" (instance $imports (type $iface)))
          (alias export $imports "incr" (func $incr))
          (alias export $imports "decr" (func $decr))
          (core func $core-incr (canon lower (func $incr)))
          (core func $core-decr (canon lower (func $decr)))
          (core module $m
            (func (import "host" "incr") (param i32) (result i32))
            (func (import "host" "decr") (param i32) (result i32))
            (func (export "incr-then-decr") (param i32) (result i32)
              local.get 0
              call 0
              call 1))
          (core instance $i (instantiate $m
            (with "host" (instance
              (export "incr" (func $core-incr))
              (export "decr" (func $core-decr))))))
          (func (export "incr-then-decr") (param "n" s32) (result s32)
            (canon lift (core func $i "incr-then-decr"))))
        "#
    );
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd008-tests:host/maths@0.1.0".parse().expect("identifier");
    let mut iface_view = linker.instance(&iface);
    iface_view
        .func_wrap(
            "incr",
            |_: HostCall<'_, ()>, (n,): (i32,)| -> wcmp::Result<i32> { Ok(n + 1) },
        )
        .expect("the registration");
    iface_view
        .func_wrap(
            "decr",
            |_: HostCall<'_, ()>, (n,): (i32,)| -> wcmp::Result<i32> { Ok(n - 1) },
        )
        .expect("the registration");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let inst = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let f = inst.get_func("incr-then-decr").expect("export");
    let result = f.call(&mut store, &[Val::S32(7)]).await.expect("call");
    assert_eq!(result.as_ref(), &[Val::S32(7)]);
}

#[wcmp_macros::test]
async fn it_passes_a_string_argument_to_a_host_function() {
    // The libc-shared-instance pattern (mirroring wasmtime's
    // `tests/all/component_model/import.rs::simple`) sidesteps the
    // forward-reference puzzle: a dedicated `(core module $libc)`
    // is instantiated up front, providing memory and realloc; the
    // `canon lower` reads from it, and the user core module
    // imports its memory. The trampoline lifts the host's string
    // out of `$libc.memory` after the guest writes (ptr, len) to
    // the flat slots.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (type $iface (instance
            (export "echo" (func (param "s" string)))))
          (import "pdd-tests:host/io@0.1.0" (instance $imports (type $iface)))
          (alias export $imports "echo" (func $echo))
          (core module $libc
            (memory (export "memory") 1)
            (global $bump (mut i32) (i32.const 16))
            (func (export "realloc") (param i32 i32 i32 i32) (result i32)
              (local $ptr i32)
              global.get $bump local.set $ptr
              global.get $bump local.get 3 i32.add global.set $bump
              local.get $ptr))
          (core instance $libc (instantiate $libc))
          (core func $core-echo
            (canon lower (func $echo) (memory (core memory $libc "memory")) (realloc (core func $libc "realloc"))))
          (core module $m
            (import "host" "echo" (func $echo (param i32) (param i32)))
            (import "libc" "memory" (memory 1))
            (func (export "send")
              i32.const 5
              i32.const 11
              call $echo)
            (data (i32.const 5) "hello world"))
          (core instance $i (instantiate $m
            (with "host" (instance (export "echo" (func $core-echo))))
            (with "libc" (instance $libc))))
          (func (export "send")
            (canon lift (core func $i "send"))))
        "#
    );

    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let mut linker: Linker<Arc<Mutex<Option<String>>>> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd-tests:host/io@0.1.0".parse().expect("identifier");
    linker
        .instance(&iface)
        .func_new(
            "echo",
            FunctionType {
                parameters: vec![FunctionParameter {
                    name: "s".to_owned(),
                    ty: ValueType::Primitive(PrimitiveType::String),
                }],
                result: None,
                async_: false,
            },
            |mut observed: HostCall<'_, Arc<Mutex<Option<String>>>>, args, _| {
                let Val::String(s) = &args[0] else {
                    panic!("expected string");
                };
                *observed.data_mut().lock().expect("lock") = Some(s.clone());
                Ok(())
            },
        )
        .expect("the registration");

    let observed = Arc::new(Mutex::new(None));
    let mut store: Store<Arc<Mutex<Option<String>>>> =
        Store::new(&engine, observed.clone()).expect("store");
    let inst = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let send = inst.get_func("send").expect("`send` export");
    let _ = send.call(&mut store, &[]).await.expect("call");

    let observed = observed.lock().expect("lock").clone();
    assert_eq!(observed.as_deref(), Some("hello world"));
}

#[wcmp_macros::test]
async fn it_propagates_a_host_function_error_through_the_call() {
    // A host function returns Err(_); the polyfill surfaces it as
    // an Error wrapping the runtime substrate's trap (the runtime
    // layer translates the host error into a guest trap).
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (type $iface (instance
            (export "fail" (func))))
          (import "pdd008-tests:host/io@0.1.0" (instance $imports (type $iface)))
          (alias export $imports "fail" (func $fail))
          (core func $core-fail (canon lower (func $fail)))
          (core module $m
            (func (import "host" "fail"))
            (func (export "trigger") call 0))
          (core instance $i (instantiate $m
            (with "host" (instance
              (export "fail" (func $core-fail))))))
          (func (export "trigger")
            (canon lift (core func $i "trigger"))))
        "#
    );
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd008-tests:host/io@0.1.0".parse().expect("identifier");
    linker
        .instance(&iface)
        .func_new(
            "fail",
            FunctionType {
                parameters: Vec::new(),
                result: None,
                async_: false,
            },
            |_: HostCall<'_, ()>, _args, _results| {
                Err(Error::Internal {
                    message: "host refused".to_owned(),
                })
            },
        )
        .expect("the registration");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let inst = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let trigger = inst.get_func("trigger").expect("trigger export");
    let outcome = trigger.call(&mut store, &[]).await;
    let err = outcome.expect_err("call should fail");
    // The error is currently wrapped by the runtime substrate's
    // trap surface; the structured polyfill error is preserved as
    // a `#[source]` chain. Asserting the top-level `Error::Abi` /
    // `Error::Instantiation` shape is enough to prove host errors
    // propagate.
    assert!(
        matches!(err, Error::Instantiation(_) | Error::Abi(_)),
        "expected wrapped host error, got {err:?}"
    );
}

#[wcmp_macros::test]
async fn it_supports_typed_host_function_with_unit_result() {
    // `func_wrap` with a closure returning `Result<()>` exercises
    // the no-return branch of the typed registration path.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (type $iface (instance
            (export "ping" (func))))
          (import "pdd008-tests:host/io@0.1.0" (instance $imports (type $iface)))
          (alias export $imports "ping" (func $ping))
          (core func $core-ping (canon lower (func $ping)))
          (core module $m
            (func (import "host" "ping"))
            (func (export "go") call 0))
          (core instance $i (instantiate $m
            (with "host" (instance
              (export "ping" (func $core-ping))))))
          (func (export "go")
            (canon lift (core func $i "go"))))
        "#
    );
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let mut linker: Linker<u32> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd008-tests:host/io@0.1.0".parse().expect("identifier");
    linker
        .instance(&iface)
        .func_wrap(
            "ping",
            |mut data: HostCall<'_, u32>, (): ()| -> wcmp::Result<()> {
                *data.data_mut() += 1;
                Ok(())
            },
        )
        .expect("the registration");
    let mut store: Store<u32> = Store::new(&engine, 0).expect("store");
    let inst = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let go = inst.get_func("go").expect("go export");
    go.call(&mut store, &[]).await.expect("call");
    go.call(&mut store, &[]).await.expect("call again");
    assert_eq!(*store.data(), 2, "ping fired twice");
}

#[wcmp_macros::test]
async fn it_rejects_a_component_whose_import_signature_disagrees_with_the_registered_host() {
    // The component declares `double(s32) -> s32`; the host
    // registers `double(s32) -> s64`. Resolution catches the
    // mismatch at link time, before instantiation.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (type $iface (instance
            (export "double" (func (param "n" s32) (result s32)))))
          (import "pdd008:host/maths@0.1.0" (instance $imports (type $iface)))
          (alias export $imports "double" (func $double))
          (core func $core-double (canon lower (func $double)))
          (core module $m
            (func (import "host" "double") (param i32) (result i32))
            (func (export "go") (param i32) (result i32) local.get 0 call 0))
          (core instance $i (instantiate $m
            (with "host" (instance
              (export "double" (func $core-double))))))
          (func (export "go") (param "n" s32) (result s32)
            (canon lift (core func $i "go"))))
        "#
    );
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd008:host/maths@0.1.0".parse().expect("identifier");
    linker
        .instance(&iface)
        .func_wrap(
            "double",
            |_: HostCall<'_, ()>, (n,): (i32,)| -> wcmp::Result<i64> { Ok(i64::from(n) * 2) },
        )
        .expect("the registration");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let Err(err) = linker.instantiate(&mut store, &component).await else {
        panic!("link should fail");
    };
    assert!(matches!(err, Error::TypeMismatch(_)), "got {err:?}");
}

#[wcmp_macros::test]
async fn it_rejects_a_call_whose_argument_count_disagrees_with_the_signature() {
    // The polyfill's `Func::call` already returns a structured
    // error when `args.len()` mismatches the declared signature.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (core module $m
            (func (export "id") (param i32) (result i32) local.get 0))
          (core instance $i (instantiate $m))
          (func (export "id") (param "v" s32) (result s32)
            (canon lift (core func $i "id"))))
        "#
    );
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let inst = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let id = inst.get_func("id").expect("id export");
    let outcome = id.call(&mut store, &[Val::S32(1), Val::S32(2)]).await;
    let err = outcome.expect_err("call should fail");
    assert!(matches!(err, Error::Abi(_)), "got {err:?}");
}

#[wcmp_macros::test]
async fn it_rejects_a_typed_export_call_whose_argument_type_disagrees() {
    // Lowering an i64 against an s32 parameter slot trips
    // `AbiCause::HostValueMismatch` at lower time.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (core module $m
            (func (export "id") (param i32) (result i32) local.get 0))
          (core instance $i (instantiate $m))
          (func (export "id") (param "v" s32) (result s32)
            (canon lift (core func $i "id"))))
        "#
    );
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let inst = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let id = inst.get_func("id").expect("id export");
    let outcome = id.call(&mut store, &[Val::S64(1)]).await;
    let err = outcome.expect_err("call should fail");
    assert!(matches!(err, Error::Abi(_)), "got {err:?}");
}

#[wcmp_macros::test]
async fn it_resolves_an_unversioned_import_against_an_unversioned_registration() {
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (type $iface (instance))
          (import "pdd008-tests:host/empty" (instance (type $iface)))
          (core module $m
            (func (export "answer") (result i32) i32.const 42))
          (core instance $i (instantiate $m))
          (func (export "answer") (result s32)
            (canon lift (core func $i "answer"))))
        "#
    );
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd008-tests:host/empty".parse().expect("identifier");
    let _ = linker.instance(&iface);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let inst = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let ans = inst.get_func("answer").expect("answer export");
    let result = ans.call(&mut store, &[]).await.expect("call");
    assert_eq!(result.as_ref(), &[Val::S32(42)]);
}

#[wcmp_macros::test]
async fn it_treats_an_empty_unmatched_interface_import_as_vacuous() {
    // The resolver allows an empty-interface import to remain
    // unmatched at link time — its absence has no runtime
    // consequence because the import declares no items.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (type $iface (instance))
          (import "pdd008-tests:host/unrelated@0.1.0" (instance (type $iface)))
          (core module $m
            (func (export "noop") nop))
          (core instance $i (instantiate $m))
          (func (export "noop")
            (canon lift (core func $i "noop"))))
        "#
    );
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let inst = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiation succeeds — vacuous import");
    let noop = inst.get_func("noop").expect("noop export");
    let result = noop.call(&mut store, &[]).await.expect("call");
    assert!(result.is_empty());
}

#[wcmp_macros::test]
async fn it_rejects_an_import_with_a_required_item_when_the_registration_version_is_incompatible() {
    // The component's import has a function item; resolution
    // must find a versioned registration in range. A registration
    // outside the WIT compatibility range surfaces
    // `LinkError::IncompatibleVersion`.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (type $iface (instance
            (export "double" (func (param "n" s32) (result s32)))))
          (import "pdd008-tests:host/maths@0.2.0" (instance $imports (type $iface)))
          (alias export $imports "double" (func $double))
          (core func $core-double (canon lower (func $double)))
          (core module $m
            (func (import "host" "double") (param i32) (result i32))
            (func (export "go") (param i32) (result i32) local.get 0 call 0))
          (core instance $i (instantiate $m
            (with "host" (instance
              (export "double" (func $core-double))))))
          (func (export "go") (param "n" s32) (result s32)
            (canon lift (core func $i "go"))))
        "#
    );
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let too_old: InterfaceIdentifier = "pdd008-tests:host/maths@0.1.0".parse().expect("identifier");
    linker
        .instance(&too_old)
        .func_wrap(
            "double",
            |_: HostCall<'_, ()>, (n,): (i32,)| -> wcmp::Result<i32> { Ok(n * 2) },
        )
        .expect("the registration");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let Err(err) = linker.instantiate(&mut store, &component).await else {
        panic!("link should fail");
    };
    assert!(matches!(err, Error::Link(_)), "got {err:?}");
}

#[wcmp_macros::test]
async fn it_inspects_a_components_imports_and_exports() {
    // Component introspection — the parsed-component view exposes
    // imports and exports along with their declared `ExternType`.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (type $iface (instance
            (export "id" (func (param "n" s32) (result s32)))))
          (import "pdd008-tests:host/io@0.1.0" (instance (type $iface)))
          (core module $m
            (func (export "answer") (result i32) i32.const 42))
          (core instance $i (instantiate $m))
          (func (export "answer") (result s32)
            (canon lift (core func $i "answer"))))
        "#
    );
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    assert_eq!(component.imports.len(), 1);
    assert_eq!(component.exports.len(), 1);
    let import = &component.imports[0];
    let interface = match &import.name {
        ExternalName::Interface(id) => id,
        other => panic!("expected interface import, got {other:?}"),
    };
    assert_eq!(interface.name(), "io");
    assert!(matches!(import.ty, ExternType::Instance(_)));
    let export = &component.exports[0];
    assert_eq!(export.name, ExternalName::Plain("answer".to_owned()));
    assert!(matches!(export.ty, ExternType::Function(_)));
}

// ----------------------------------------------------------------
// Stubs: capabilities not yet realised.
// ----------------------------------------------------------------

/// A component that imports `log` under a plain name and calls it
/// with a string from its data segment. The memory lives in a
/// separate core instance so the lowered import can name it.
const PLAIN_LOG: &[u8] = component!(
    r#"
    (component
      (import "log" (func $log (param "message" string)))
      (core module $libc (memory (export "memory") 1))
      (core instance $libc (instantiate $libc))
      (core func $core-log (canon lower (func $log) (memory (core memory $libc "memory"))))
      (core module $m
        (import "libc" "memory" (memory 1))
        (import "host" "log" (func $log (param i32 i32)))
        (data (i32.const 8) "hello from the guest")
        (func (export "run") i32.const 8 i32.const 20 call $log))
      (core instance $i (instantiate $m
        (with "libc" (instance $libc))
        (with "host" (instance (export "log" (func $core-log))))))
      (func (export "run") (canon lift (core func $i "run"))))
    "#
);

#[wcmp_macros::test]
async fn it_supports_a_plain_named_top_level_import() {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, PLAIN_LOG)
        .await
        .expect("component parses");
    let mut linker: Linker<Vec<String>> = Linker::new(&engine);
    linker
        .root()
        .func_wrap(
            "log",
            |mut messages: HostCall<'_, Vec<String>>, (message,): (String,)| -> wcmp::Result<()> {
                messages.data_mut().push(message);
                Ok(())
            },
        )
        .expect("the registration");
    let mut store: Store<Vec<String>> = Store::new(&engine, Vec::new()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("a plain-named import resolves through the root namespace");
    let run = instance.get_func("run").expect("run export");
    run.call(&mut store, &[]).await.expect("run");
    assert_eq!(store.data(), &vec!["hello from the guest".to_owned()]);
}

#[wcmp_macros::test]
async fn it_reports_an_unregistered_plain_named_import_as_unresolved() {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, PLAIN_LOG)
        .await
        .expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let err = match linker.instantiate(&mut store, &component).await {
        Ok(_) => panic!("nothing is registered under `log`, so instantiation must fail"),
        Err(err) => err,
    };
    let names_log = match &err {
        Error::Link(link) => matches!(
            link.as_ref(),
            LinkError::UnresolvedImport {
                import: ExternalName::Plain(name),
                item: None,
            } if name == "log"
        ),
        _ => false,
    };
    assert!(
        names_log,
        "expected an unresolved-import link error naming `log`, got {err:?}"
    );

    // A root-level miss names no item, and reads as it always has.
    assert_eq!(
        err.to_string(),
        "link error: no registered linker instance satisfies import `log`"
    );
}

#[wcmp_macros::test]
async fn it_supports_a_plain_named_instance_import() {
    // `(import "host" (instance …))`: the instance's items live in
    // the root namespace under the plain name.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (import "host" (instance $host
            (export "double" (func (param "x" u32) (result u32)))))
          (alias export $host "double" (func $double))
          (core func $core-double (canon lower (func $double)))
          (core module $m
            (import "host" "double" (func $double (param i32) (result i32)))
            (func (export "run") (param i32) (result i32)
              local.get 0 call $double i32.const 1 i32.add))
          (core instance $i (instantiate $m
            (with "host" (instance (export "double" (func $core-double))))))
          (func (export "run") (param "x" u32) (result u32)
            (canon lift (core func $i "run"))))
        "#
    );
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .instance("host")
        .func_wrap(
            "double",
            |_: HostCall<'_, ()>, (x,): (u32,)| -> wcmp::Result<u32> { Ok(x * 2) },
        )
        .expect("the registration");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("a plain-named instance import resolves through the root namespace");
    let run = instance
        .get_func("run")
        .expect("run export")
        .typed::<(u32,), u32>()
        .expect("typed");
    assert_eq!(run.call(&mut store, (20,)).await.expect("run"), 41);
}

#[wcmp_macros::test]
async fn it_navigates_instance_typed_exports() {
    // The component publishes an `(instance)` export under the
    // `test:guest/foo` interface name; the inner `select-nth`
    // function picks an element of a `list<string>` argument. The
    // navigator reaches it through `exports().instance(...).func(...)`
    // and round-trips a real call.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (core module $m
            (memory (export "memory") 1)
            (global $bump (mut i32) (i32.const 16))
            (func $cabi-realloc (export "cabi_realloc")
                  (param i32 i32 i32 i32) (result i32)
              (local $ptr i32)
              ;; Round the bump pointer up to the alignment asked for,
              ;; as a real allocator does: the canonical ABI reads the
              ;; result back at that alignment and refuses a pointer
              ;; that does not have it.
              (global.set $bump
                (i32.and
                  (i32.add (global.get $bump)
                           (i32.sub (local.get 2) (i32.const 1)))
                  (i32.sub (i32.const 0) (local.get 2))))
              global.get $bump
              local.set $ptr
              global.get $bump
              local.get 3
              i32.add
              global.set $bump
              local.get $ptr)
            (func (export "select-nth")
                  (param $list-ptr i32) (param $list-len i32) (param $n i32)
                  (result i32)
              (local $ret i32)
              (local $elem i32)
              i32.const 0
              i32.const 0
              i32.const 4
              i32.const 8
              call $cabi-realloc
              local.set $ret
              local.get $list-ptr
              local.get $n
              i32.const 3
              i32.shl
              i32.add
              local.set $elem
              local.get $ret
              local.get $elem
              i32.load
              i32.store
              local.get $ret
              local.get $elem
              i32.load offset=4
              i32.store offset=4
              local.get $ret))
          (core instance $i (instantiate $m))
          (func $select-nth
                (param "x" (list string)) (param "n" u32) (result string)
            (canon lift (core func $i "select-nth")
                       (memory (core memory $i "memory"))
                       (realloc (core func $i "cabi_realloc"))))
          (instance $foo (export "select-nth" (func $select-nth)))
          (export "test:guest/foo" (instance $foo)))
        "#
    );

    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiation succeeds");

    // Root-level `func` is empty — the function lives nested inside
    // the instance-typed export.
    assert!(instance.exports().func("select-nth").is_none());

    let interface: InterfaceIdentifier = "test:guest/foo"
        .parse()
        .expect("interface identifier parses");
    let foo = instance
        .exports()
        .instance(&interface)
        .expect("instance-typed export `test:guest/foo` is present");
    let select_nth = foo
        .func("select-nth")
        .expect("`select-nth` is exported by the instance");

    let example = ["a", "b", "c"]
        .iter()
        .map(|s| Val::String((*s).to_owned()))
        .collect::<Vec<_>>();
    let results = select_nth
        .call(
            &mut store,
            &[Val::List(example.into_boxed_slice()), Val::U32(1)],
        )
        .await
        .expect("call succeeds");
    assert_eq!(results.as_ref(), &[Val::String("b".to_owned())]);

    // Looking up an unknown interface returns `None` rather than
    // raising.
    let absent: InterfaceIdentifier = "test:guest/missing".parse().expect("identifier parses");
    assert!(instance.exports().instance(&absent).is_none());
}

#[wcmp_macros::test]
async fn it_supports_a_typed_export_call_surface() {
    // The same `select-nth` shape as the navigator test, used here
    // to exercise the typed-call surface end-to-end and the link-
    // time signature check.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (core module $m
            (memory (export "memory") 1)
            (global $bump (mut i32) (i32.const 16))
            (func $cabi-realloc (export "cabi_realloc")
                  (param i32 i32 i32 i32) (result i32)
              (local $ptr i32)
              ;; Round the bump pointer up to the alignment asked for,
              ;; as a real allocator does: the canonical ABI reads the
              ;; result back at that alignment and refuses a pointer
              ;; that does not have it.
              (global.set $bump
                (i32.and
                  (i32.add (global.get $bump)
                           (i32.sub (local.get 2) (i32.const 1)))
                  (i32.sub (i32.const 0) (local.get 2))))
              global.get $bump
              local.set $ptr
              global.get $bump
              local.get 3
              i32.add
              global.set $bump
              local.get $ptr)
            (func (export "select-nth")
                  (param $list-ptr i32) (param $list-len i32) (param $n i32)
                  (result i32)
              (local $ret i32)
              (local $elem i32)
              i32.const 0
              i32.const 0
              i32.const 4
              i32.const 8
              call $cabi-realloc
              local.set $ret
              local.get $list-ptr
              local.get $n
              i32.const 3
              i32.shl
              i32.add
              local.set $elem
              local.get $ret
              local.get $elem
              i32.load
              i32.store
              local.get $ret
              local.get $elem
              i32.load offset=4
              i32.store offset=4
              local.get $ret))
          (core instance $i (instantiate $m))
          (func $select-nth
                (param "x" (list string)) (param "n" u32) (result string)
            (canon lift (core func $i "select-nth")
                       (memory (core memory $i "memory"))
                       (realloc (core func $i "cabi_realloc"))))
          (instance $foo (export "select-nth" (func $select-nth)))
          (export "test:guest/foo" (instance $foo)))
        "#
    );

    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiation succeeds");
    let interface: InterfaceIdentifier = "test:guest/foo"
        .parse()
        .expect("interface identifier parses");
    let foo = instance
        .exports()
        .instance(&interface)
        .expect("`test:guest/foo` is present");

    // Happy path: the requested Rust signature matches the export.
    let select_nth = foo
        .func("select-nth")
        .expect("`select-nth` present")
        .typed::<(Vec<String>, u32), String>()
        .expect("typed conversion succeeds");

    let example: Vec<String> = ["a", "b", "c"].iter().map(|s| (*s).to_owned()).collect();
    let result = select_nth
        .call(&mut store, (example.clone(), 1))
        .await
        .expect("typed call succeeds");
    assert_eq!(result, "b");

    // Mismatched return type: the export returns `string`, the
    // requested signature claims `u32`. The conversion fails before
    // any call is made.
    let mismatch = foo
        .func("select-nth")
        .expect("`select-nth` present")
        .typed::<(Vec<String>, u32), u32>()
        .expect_err("typed conversion rejects a return-type mismatch");
    assert!(
        matches!(mismatch, Error::TypeMismatch(_)),
        "expected Error::TypeMismatch, got {mismatch:?}",
    );
}

#[wcmp_macros::test]
async fn it_rejects_a_call_made_through_a_different_store() {
    // An instance's core state lives in exactly one store. A
    // handle called with another store is refused before the
    // runtime layer sees it.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (core module $m
            (func (export "one") (result i32) i32.const 1))
          (core instance $i (instantiate $m))
          (func (export "one") (result u32)
            (canon lift (core func $i "one"))))
        "#
    );
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut owner: Store<()> = Store::new(&engine, ()).expect("store");
    let mut other: Store<()> = Store::new(&engine, ()).expect("store");
    let inst = linker
        .instantiate(&mut owner, &component)
        .await
        .expect("instantiate");
    let one = inst.get_func("one").expect("one export");

    let err = one
        .call(&mut other, &[])
        .await
        .expect_err("a call through a different store is rejected");
    assert!(
        matches!(
            &err,
            Error::Instantiation(cause)
                if matches!(**cause, wcmp::InstantiationError::WrongStore)
        ),
        "expected InstantiationError::WrongStore, got {err:?}"
    );

    // The owning store still works.
    let result = one
        .call(&mut owner, &[])
        .await
        .expect("call through the owner");
    assert_eq!(result.as_ref(), &[Val::U32(1)]);
}

#[wcmp_macros::test]
async fn it_reports_an_unsupported_feature_as_a_structured_error() {
    // A component whose export lifts under the garbage-collected
    // data model uses a feature the polyfill has not built. The
    // translator reports it at construction as a structured
    // `Error::Unsupported` naming the feature, never as a panic.
    // Validation admits the option only with its feature enabled, so
    // the engine enables it.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (core module $m
            (func (export "f")))
          (core instance $i (instantiate $m))
          (func (export "f")
            (canon lift (core func $i "f") gc)))
        "#
    );
    let mut config = wcmp::EngineConfig::default();
    config.wasm_component_model_gc(true);
    let engine = Engine::with_backend(crate::test_backend::backend())
        .and_then(|engine| engine.with_config(&config))
        .expect("engine");
    let err = Component::new(&engine, COMPONENT)
        .await
        .expect_err("the garbage-collected data model is not supported yet");
    assert!(
        matches!(&err, Error::Unsupported { feature } if feature.contains("garbage-collected")),
        "expected Error::Unsupported, got {err:?}"
    );
}

#[wcmp_macros::test]
async fn it_navigates_plain_named_instance_exports() {
    // A component may publish an instance under a plain name rather
    // than a WIT interface identifier, and an instance may nest
    // another. The navigator reaches every level by name, and an
    // instance with no items is still reachable.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (core module $m
            (func (export "f") (result i32) i32.const 42)
            (func (export "g") (param i32) (result i32)
              local.get 0 i32.const 1 i32.add))
          (core instance $i (instantiate $m))
          (func $f (result u32) (canon lift (core func $i "f")))
          (func $g (param "x" u32) (result u32) (canon lift (core func $i "g")))
          (instance $b (export "g" (func $g)))
          (instance $a
            (export "f" (func $f))
            (export "b" (instance $b)))
          (export "a" (instance $a))
          (instance $e)
          (export "e" (instance $e)))
        "#
    );

    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("a component with plain-named instance exports parses");

    // The type projection keeps the nested shape.
    let a = component
        .exports
        .iter()
        .find(|export| export.name == ExternalName::Plain("a".to_owned()))
        .expect("`a` is a declared export");
    let ExternType::Instance(a_type) = &a.ty else {
        panic!("expected `a` to be an instance export, got {:?}", a.ty);
    };
    assert_eq!(a_type.items.len(), 2);
    assert_eq!(a_type.items[0].name, "f");
    assert!(matches!(a_type.items[0].ty, ExternType::Function(_)));
    assert_eq!(a_type.items[1].name, "b");
    let ExternType::Instance(b_type) = &a_type.items[1].ty else {
        panic!(
            "expected `b` to be a nested instance, got {:?}",
            a_type.items[1].ty
        );
    };
    assert_eq!(b_type.items.len(), 1);
    assert_eq!(b_type.items[0].name, "g");

    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiation succeeds");

    // The functions live inside the instances, not at the root.
    assert!(instance.get_func("f").is_none());
    assert!(instance.exports().func("g").is_none());

    let a = instance
        .exports()
        .instance("a")
        .expect("the plain-named instance export `a` is present");
    assert_eq!(*a.name(), ExternalName::Plain("a".to_owned()));
    let f = a.func("f").expect("`f` is exported by `a`");
    assert_eq!(
        f.call(&mut store, &[])
            .await
            .expect("call succeeds")
            .as_ref(),
        &[Val::U32(42)]
    );

    // A nested instance is reached through the enclosing view.
    let b = a.instance("b").expect("`b` is nested inside `a`");
    assert_eq!(*b.name(), ExternalName::Plain("b".to_owned()));
    let g = b
        .func("g")
        .expect("`g` is exported by `b`")
        .typed::<(u32,), u32>()
        .expect("typed conversion succeeds");
    assert_eq!(g.call(&mut store, (41,)).await.expect("call succeeds"), 42);
    assert!(a.func("g").is_none());
    assert!(b.func("f").is_none());
    assert!(b.instance("b").is_none());

    // An `ExternalName` from the component's export list is a key too.
    let by_name = instance
        .exports()
        .instance(ExternalName::Plain("a".to_owned()))
        .expect("the export list's name addresses the same instance");
    assert!(by_name.func("f").is_some());

    // An empty instance export is reachable; an absent one is not.
    let e = instance
        .exports()
        .instance("e")
        .expect("the empty instance export `e` is present");
    assert!(e.func("f").is_none());
    assert!(instance.exports().instance("missing").is_none());
    assert!(instance.exports().instance("f").is_none());
}

/// A component whose import `a` holds an instance `b` that holds the
/// function `f`: an import path two levels deep.
const NESTED_IMPORT: &[u8] = component!(
    r#"
    (component
      (import "a" (instance $a
        (export "b" (instance
          (export "f" (func (result u32)))))))
      (alias export $a "b" (instance $b))
      (core func $f (canon lower (func $b "f")))
      (core module $m
        (import "" "f" (func $f (result i32)))
        (func (export "run") (result i32) call $f))
      (core instance $i (instantiate $m
        (with "" (instance (export "f" (func $f))))))
      (func (export "run") (result u32) (canon lift (core func $i "run"))))
    "#
);

#[wcmp_macros::test]
async fn it_links_an_import_nested_two_levels() {
    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let component = Component::new(&engine, NESTED_IMPORT)
        .await
        .expect("a component with a two-level import parses");

    // The type projection describes the nesting.
    assert_eq!(component.imports.len(), 1);
    assert_eq!(
        component.imports[0].name,
        ExternalName::Plain("a".to_owned())
    );
    let ExternType::Instance(a) = &component.imports[0].ty else {
        panic!(
            "expected an instance import, got {:?}",
            component.imports[0].ty
        );
    };
    assert_eq!(a.items.len(), 1);
    assert_eq!(a.items[0].name, "b");
    let ExternType::Instance(b) = &a.items[0].ty else {
        panic!("expected a nested instance, got {:?}", a.items[0].ty);
    };
    assert_eq!(b.items.len(), 1);
    assert_eq!(b.items[0].name, "f");
    assert!(matches!(b.items[0].ty, ExternType::Function(_)));

    // Nested `instance` calls register the item where the component
    // looks for it, as in Wasmtime.
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .instance("a")
        .instance("b")
        .func_wrap("f", |_, (): ()| Ok(7u32))
        .expect("the registration");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the nested registration satisfies the import");
    let run = instance
        .get_func("run")
        .expect("`run` is exported")
        .typed::<(), u32>()
        .expect("typed");
    assert_eq!(run.call(&mut store, ()).await.expect("call succeeds"), 7);

    // The function registered one level up does not satisfy it, and
    // the error names the import and the item inside it that is
    // missing.
    let mut wrong_level: Linker<()> = Linker::new(&engine);
    wrong_level
        .root()
        .instance("a")
        .func_wrap("f", |_, (): ()| Ok(7u32))
        .expect("the registration");
    let err = match wrong_level.instantiate(&mut store, &component).await {
        Ok(_) => panic!("a function at the wrong level must not link"),
        Err(err) => err,
    };
    assert_eq!(
        err.to_string(),
        "link error: import `a`: instance export `b` has the wrong type: no registered linker \
         instance satisfies import `a`"
    );
    match err {
        Error::Link(inner) => match *inner {
            LinkError::UnresolvedImport { import, item } => {
                assert_eq!(import, ExternalName::Plain("a".to_owned()));
                assert_eq!(item.as_deref(), Some("b"));
            }
            other => panic!("expected an unresolved import, got {other:?}"),
        },
        other => panic!("expected a link error, got {other:?}"),
    }
}

#[wcmp_macros::test]
async fn it_links_a_resource_nested_two_levels() {
    // The resource `r` and its constructor live inside `a.b`; the
    // component mints a handle through the constructor and drops it,
    // which runs the host destructor registered at the nested level.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (import "a" (instance $a
            (export "b" (instance
              (export "r" (type $r (sub resource)))
              (export "make" (func (result (own $r))))))))
          (alias export $a "b" (instance $b))
          (alias export $b "r" (type $r))
          (core func $make (canon lower (func $b "make")))
          (core func $drop (canon resource.drop $r))
          (core module $m
            (import "" "make" (func $make (result i32)))
            (import "" "drop" (func $drop (param i32)))
            (func (export "run") call $make call $drop))
          (core instance $i (instantiate $m
            (with "" (instance
              (export "make" (func $make))
              (export "drop" (func $drop))))))
          (func (export "run") (canon lift (core func $i "run"))))
        "#
    );
    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let mut linker: Linker<u32> = Linker::new(&engine);
    let mut root = linker.root();
    let mut a = root.instance("a");
    let mut b = a.instance("b");
    let r = b
        .resource("r", |drops, _rep| {
            *drops += 1;
            Ok(())
        })
        .expect("the registration");
    b.func_new(
        "make",
        FunctionType {
            parameters: vec![],
            result: Some(ValueType::Own(ResourceType::new("r"))),
            async_: false,
        },
        move |call, _, results| {
            results[0] = Val::Own(call.resource_new(r, 42)?);
            Ok(())
        },
    )
    .expect("the registration");
    let mut store: Store<u32> = Store::new(&engine, 0).expect("store construction succeeds");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the nested resource registration satisfies the import");
    instance
        .get_func("run")
        .expect("`run` is exported")
        .call(&mut store, &[])
        .await
        .expect("call succeeds");
    assert_eq!(*store.data(), 1, "the nested destructor ran once");
}

#[wcmp_macros::test]
async fn it_links_an_interface_named_import_with_a_nested_instance() {
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (import "test:guest/host" (instance $h
            (export "nested" (instance
              (export "f" (func (result u32)))))))
          (alias export $h "nested" (instance $n))
          (core func $f (canon lower (func $n "f")))
          (core module $m
            (import "" "f" (func $f (result i32)))
            (func (export "run") (result i32) call $f))
          (core instance $i (instantiate $m
            (with "" (instance (export "f" (func $f))))))
          (func (export "run") (result u32) (canon lift (core func $i "run"))))
        "#
    );
    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let host: InterfaceIdentifier = "test:guest/host".parse().expect("identifier parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .instance(&host)
        .instance("nested")
        .func_wrap("f", |_, (): ()| Ok(4u32))
        .expect("the registration");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the nested registration under the interface satisfies the import");
    let run = instance
        .get_func("run")
        .expect("`run` is exported")
        .typed::<(), u32>()
        .expect("typed");
    assert_eq!(run.call(&mut store, ()).await.expect("call succeeds"), 4);
}

#[wcmp_macros::test]
async fn it_links_a_recursively_empty_instance_import_without_a_registration() {
    // Wasmtime needs no definition for an instance whose items are
    // themselves empty instances, at any depth.
    const EMPTY: &[u8] = component!(
        r#"
        (component
          (import "not-provided" (instance))
          (import "not-provided2" (instance
            (export "x" (instance
              (export "y" (instance)))))))
        "#
    );
    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let component = Component::new(&engine, EMPTY).await.expect("parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    linker
        .instantiate(&mut store, &component)
        .await
        .expect("recursively empty instance imports link with nothing registered");

    // An item that needs a definition still needs one.
    let component = Component::new(&engine, NESTED_IMPORT)
        .await
        .expect("parses");
    assert!(linker.instantiate(&mut store, &component).await.is_err());
}

#[wcmp_macros::test]
async fn it_rejects_a_registration_of_the_wrong_kind() {
    // Wasmtime's `import.wast`: a function registered under the name
    // the component imports as an instance is a link failure,
    // `expected instance found func`, not a vacuous match.
    const IMPORTS_INSTANCE: &[u8] = component!(
        r#"
        (component (import "host-return-two" (instance)))
        "#
    );
    const IMPORTS_FUNC: &[u8] = component!(
        r#"
        (component (import "host" (func (result u32))))
        "#
    );
    const IMPORTS_ITEM: &[u8] = component!(
        r#"
        (component
          (import "host" (instance
            (export "f" (func (result u32))))))
        "#
    );
    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");

    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap("host-return-two", |_, (): ()| Ok(2u32))
        .expect("the registration");
    linker
        .root()
        .instance("host")
        .resource("f", |_, _| Ok(()))
        .expect("the registration");

    let kind_mismatch = |err: Error| match err {
        Error::Link(inner) => match *inner {
            LinkError::KindMismatch {
                import,
                item,
                expected,
                found,
            } => (import, item, expected, found),
            other => panic!("expected a kind mismatch, got {other:?}"),
        },
        other => panic!("expected a link error, got {other:?}"),
    };

    let component = Component::new(&engine, IMPORTS_INSTANCE)
        .await
        .expect("parses");
    let err = match linker.instantiate(&mut store, &component).await {
        Ok(_) => panic!("a function where an instance is imported must not link"),
        Err(err) => err,
    };
    // The import itself is at fault, so the message names no item.
    assert_eq!(
        err.to_string(),
        "link error: import `host-return-two`: expected instance found func"
    );
    assert_eq!(
        kind_mismatch(err),
        (
            ExternalName::Plain("host-return-two".to_owned()),
            None,
            "instance",
            "func"
        )
    );

    // The reverse: an instance where a function is imported.
    let component = Component::new(&engine, IMPORTS_FUNC).await.expect("parses");
    let err = match linker.instantiate(&mut store, &component).await {
        Ok(_) => panic!("an instance where a function is imported must not link"),
        Err(err) => err,
    };
    assert_eq!(
        kind_mismatch(err),
        (
            ExternalName::Plain("host".to_owned()),
            None,
            "func",
            "instance"
        )
    );

    // An item inside an instance import names the item, in the
    // structured cause and in the message: Wasmtime's error chain
    // puts `instance export `f` has the wrong type` between the
    // import it names and the reason, and the flat message carries
    // the same three parts in the same order.
    let component = Component::new(&engine, IMPORTS_ITEM).await.expect("parses");
    let err = match linker.instantiate(&mut store, &component).await {
        Ok(_) => panic!("a resource where a function item is imported must not link"),
        Err(err) => err,
    };
    assert_eq!(
        err.to_string(),
        "link error: import `host`: instance export `f` has the wrong type: expected func found \
         resource"
    );
    assert_eq!(
        kind_mismatch(err),
        (
            ExternalName::Plain("host".to_owned()),
            Some("f".to_owned()),
            "func",
            "resource"
        )
    );

    // A name with nothing registered is still a plain unresolved
    // import.
    let empty: Linker<()> = Linker::new(&engine);
    let component = Component::new(&engine, IMPORTS_FUNC).await.expect("parses");
    match empty.instantiate(&mut store, &component).await {
        Err(Error::Link(inner)) => {
            assert!(matches!(*inner, LinkError::UnresolvedImport { .. }));
        }
        Err(other) => panic!("expected an unresolved import, got {other:?}"),
        Ok(_) => panic!("an import with nothing registered must not link"),
    }
}

/// A component that imports a function under an *interface* name and
/// calls it.
///
/// The Component Model constrains an import's sort from the shape of
/// its name only for the annotated plain names — `[constructor]`,
/// `[method]`, `[static]` — and for an `implements`-annotated
/// import, so an interface name on a function import is a shape a
/// component can present. Such an import asks for a single host
/// item, which is what the root namespace holds, so the resolver
/// looks for it there under the whole name.
const INTERFACE_NAMED_FUNCTION: &[u8] = component!(
    r#"
    (component
      (import "pdd-tests:host/answers@0.1.0" (func $answer (param "x" u32) (result u32)))
      (core func $lowered (canon lower (func $answer)))
      (core module $m
        (import "" "answer" (func $answer (param i32) (result i32)))
        (func (export "run") (result i32) (call $answer (i32.const 20))))
      (core instance $i (instantiate $m
        (with "" (instance (export "answer" (func $lowered))))))
      (func (export "run") (result u32) (canon lift (core func $i "run"))))
    "#
);

/// The interface identifier the above component's import is named
/// by, and the string a host registers it under on the root view.
const ANSWERS: &str = "pdd-tests:host/answers@0.1.0";

#[wcmp_macros::test]
async fn it_links_a_function_import_under_an_interface_name_through_the_root_namespace() {
    // The host registers the function on the root view under the
    // import's whole name, as it would in Wasmtime, and the guest's
    // call reaches the closure.
    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let component = Component::new(&engine, INTERFACE_NAMED_FUNCTION)
        .await
        .expect("component parses");

    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap(ANSWERS, |_call: HostCall<'_, ()>, (x,): (u32,)| Ok(x * 2))
        .expect("the registration");

    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the root registration under the interface name satisfies the import");
    let run = instance
        .get_func("run")
        .expect("`run` is exported")
        .typed::<(), u32>()
        .expect("typed");
    assert_eq!(run.call(&mut store, ()).await.expect("call succeeds"), 40);
}

#[wcmp_macros::test]
async fn it_refuses_a_function_import_under_an_interface_name_with_no_root_registration() {
    // Nothing registered is an unresolved import that names the
    // interface-named import — and a *linker instance* registered
    // under the same identifier does not satisfy it either. The
    // import wants one host item; a linker instance holds an
    // interface's worth of them, and none of them is the import.
    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let component = Component::new(&engine, INTERFACE_NAMED_FUNCTION)
        .await
        .expect("component parses");
    let answers: InterfaceIdentifier = ANSWERS.parse().expect("identifier parses");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");

    let empty: Linker<()> = Linker::new(&engine);
    let err = match empty.instantiate(&mut store, &component).await {
        Ok(_) => panic!("an import with nothing registered must not link"),
        Err(err) => err,
    };
    assert!(
        matches!(
            &err,
            Error::Link(inner)
                if matches!(
                    &**inner,
                    LinkError::UnresolvedImport { import, .. }
                        if *import == ExternalName::Interface(answers.clone())
                )
        ),
        "expected an unresolved import naming `{ANSWERS}`, got {err:?}",
    );

    let mut by_interface: Linker<()> = Linker::new(&engine);
    by_interface
        .instance(&answers)
        .func_wrap("answer", |_call: HostCall<'_, ()>, (x,): (u32,)| Ok(x * 2))
        .expect("the registration");
    let err = match by_interface.instantiate(&mut store, &component).await {
        Ok(_) => panic!("a linker instance must not satisfy a function import"),
        Err(err) => err,
    };
    assert!(
        matches!(
            &err,
            Error::Link(inner) if matches!(&**inner, LinkError::UnresolvedImport { .. })
        ),
        "expected an unresolved import, got {err:?}",
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_registration_of_another_kind_under_an_interface_name() {
    // The kind check runs on the root entry whatever the import's
    // name looks like: an instance registered under the name the
    // component imports as a function is `expected func found
    // instance`, as it is for a plain name.
    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let component = Component::new(&engine, INTERFACE_NAMED_FUNCTION)
        .await
        .expect("component parses");
    let answers: InterfaceIdentifier = ANSWERS.parse().expect("identifier parses");

    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .instance(ANSWERS)
        .func_wrap("answer", |_call: HostCall<'_, ()>, (x,): (u32,)| Ok(x * 2))
        .expect("the registration");

    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    let err = match linker.instantiate(&mut store, &component).await {
        Ok(_) => panic!("an instance where a function is imported must not link"),
        Err(err) => err,
    };
    match err {
        Error::Link(inner) => match *inner {
            LinkError::KindMismatch {
                import,
                item,
                expected,
                found,
            } => {
                assert_eq!(import, ExternalName::Interface(answers));
                assert_eq!(item, None);
                assert_eq!(expected, "func");
                assert_eq!(found, "instance");
            }
            other => panic!("expected a kind mismatch, got {other:?}"),
        },
        other => panic!("expected a link error, got {other:?}"),
    }
}

/// The same component as [`INTERFACE_NAMED_FUNCTION`], importing a
/// later patch of the same interface version. A host that
/// registered `@0.1.0` on the root view satisfies it, because the
/// root matches a versioned name the way a registered interface key
/// is matched.
const INTERFACE_NAMED_FUNCTION_LATER_PATCH: &[u8] = component!(
    r#"
    (component
      (import "pdd-tests:host/answers@0.1.3" (func $answer (param "x" u32) (result u32)))
      (core func $lowered (canon lower (func $answer)))
      (core module $m
        (import "" "answer" (func $answer (param i32) (result i32)))
        (func (export "run") (result i32) (call $answer (i32.const 20))))
      (core instance $i (instantiate $m
        (with "" (instance (export "answer" (func $lowered))))))
      (func (export "run") (result u32) (canon lift (core func $i "run"))))
    "#
);

/// The interface identifier the later-patch component imports.
const ANSWERS_LATER_PATCH: &str = "pdd-tests:host/answers@0.1.3";

#[wcmp_macros::test]
async fn it_links_a_function_import_under_an_interface_name_to_a_compatible_root_registration() {
    // The host registered `@0.1.0`; the component imports `@0.1.3`.
    // Both sit on the `0.1` compatibility track, so the root
    // registration answers and the guest's call reaches its closure.
    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let component = Component::new(&engine, INTERFACE_NAMED_FUNCTION_LATER_PATCH)
        .await
        .expect("component parses");

    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap(ANSWERS, |_call: HostCall<'_, ()>, (x,): (u32,)| Ok(x * 2))
        .expect("the registration");

    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("a compatible root registration satisfies the import");
    let run = instance
        .get_func("run")
        .expect("`run` is exported")
        .typed::<(), u32>()
        .expect("typed");
    assert_eq!(run.call(&mut store, ()).await.expect("call succeeds"), 40);
}

#[wcmp_macros::test]
async fn it_prefers_an_exact_root_registration_over_a_compatible_one() {
    // With `@0.1.0` and `@0.1.3` both on the root view, an import of
    // `@0.1.3` takes the exact registration. The two closures
    // differ, so the value the guest gets back names the one that
    // answered.
    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let component = Component::new(&engine, INTERFACE_NAMED_FUNCTION_LATER_PATCH)
        .await
        .expect("component parses");

    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap(ANSWERS, |_call: HostCall<'_, ()>, (x,): (u32,)| Ok(x * 2))
        .expect("the registration");
    linker
        .root()
        .func_wrap(
            ANSWERS_LATER_PATCH,
            |_call: HostCall<'_, ()>, (x,): (u32,)| Ok(x * 3),
        )
        .expect("the registration");

    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the exact root registration satisfies the import");
    let run = instance
        .get_func("run")
        .expect("`run` is exported")
        .typed::<(), u32>()
        .expect("typed");
    assert_eq!(run.call(&mut store, ()).await.expect("call succeeds"), 60);
}

#[wcmp_macros::test]
async fn it_refuses_a_root_registration_on_another_compatibility_track() {
    // A registration on another track answers nothing, and the
    // unresolved import names the version the component asked for
    // rather than the one the host offered.
    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let component = Component::new(&engine, INTERFACE_NAMED_FUNCTION_LATER_PATCH)
        .await
        .expect("component parses");
    let wanted: InterfaceIdentifier = ANSWERS_LATER_PATCH.parse().expect("identifier parses");

    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap(
            "pdd-tests:host/answers@0.2.0",
            |_call: HostCall<'_, ()>, (x,): (u32,)| Ok(x * 2),
        )
        .expect("the registration");

    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    let err = match linker.instantiate(&mut store, &component).await {
        Ok(_) => panic!("a registration off the track must not link"),
        Err(err) => err,
    };
    assert!(
        matches!(
            &err,
            Error::Link(inner)
                if matches!(
                    &**inner,
                    LinkError::UnresolvedImport { import, .. }
                        if *import == ExternalName::Interface(wanted.clone())
                )
        ),
        "expected an unresolved import naming `{ANSWERS_LATER_PATCH}`, got {err:?}",
    );
}

#[wcmp_macros::test]
async fn it_prefers_an_exact_linker_instance_over_a_compatible_one() {
    // The interface registry answers an exact version before a
    // compatible one, as the root namespace does. With `@0.2.0` and
    // `@0.2.7` both registered, an import of `@0.2.0` reaches the
    // `@0.2.0` registration's closure.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (type $iface (instance
            (export "double" (func (param "n" s32) (result s32)))))
          (import "pdd-tests:host/maths@0.2.0" (instance $imports (type $iface)))
          (alias export $imports "double" (func $double))
          (core func $core-double (canon lower (func $double)))
          (core module $m
            (func (import "host" "double") (param i32) (result i32))
            (func (export "go") (param i32) (result i32) local.get 0 call 0))
          (core instance $i (instantiate $m
            (with "host" (instance
              (export "double" (func $core-double))))))
          (func (export "go") (param "n" s32) (result s32)
            (canon lift (core func $i "go"))))
        "#
    );

    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let exact: InterfaceIdentifier = "pdd-tests:host/maths@0.2.0".parse().expect("identifier");
    let newer: InterfaceIdentifier = "pdd-tests:host/maths@0.2.7".parse().expect("identifier");
    linker
        .instance(&exact)
        .func_wrap("double", |_call: HostCall<'_, ()>, (n,): (i32,)| Ok(n * 2))
        .expect("the registration");
    linker
        .instance(&newer)
        .func_wrap("double", |_call: HostCall<'_, ()>, (n,): (i32,)| Ok(n * 3))
        .expect("the registration");

    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the exact registration satisfies the import");
    let go = instance
        .get_func("go")
        .expect("`go` is exported")
        .typed::<(i32,), i32>()
        .expect("typed");
    assert_eq!(go.call(&mut store, (7,)).await.expect("call succeeds"), 14);
}

#[wcmp_macros::test]
async fn it_refuses_a_mismatched_signature_under_an_interface_name() {
    // The signature comparison runs too, and the diagnostic names
    // the root entry the registration was attached to.
    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let component = Component::new(&engine, INTERFACE_NAMED_FUNCTION)
        .await
        .expect("component parses");

    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap(ANSWERS, |_call: HostCall<'_, ()>, (x,): (u64,)| Ok(x * 2))
        .expect("the registration");

    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    let err = match linker.instantiate(&mut store, &component).await {
        Ok(_) => panic!("a mismatched signature must not link"),
        Err(err) => err,
    };
    assert!(
        matches!(
            &err,
            Error::TypeMismatch(mismatch)
                if matches!(
                    &mismatch.position,
                    wcmp::TypeMismatchPosition::
                        HostFunctionRegistrationPlain { name } if name == ANSWERS
                )
        ),
        "expected a type mismatch naming `{ANSWERS}`, got {err:?}",
    );
}

/// A component that imports a *resource type* under an interface
/// name and re-exports a function that drops a handle to it.
///
/// The import's name says where a host would look the item up and
/// nothing about the import's sort, so a resource named this way
/// resolves through the root namespace under the whole name, just
/// as the function above does.
const INTERFACE_NAMED_RESOURCE: &[u8] = component!(
    r#"
    (component
      (import "pdd-tests:host/thing@0.1.0" (type $thing (sub resource)))
      (core func $thing-drop (canon resource.drop $thing))
      (core module $m
        (func (import "host" "drop") (param i32))
        (func (export "consume") (param i32)
          local.get 0
          call 0))
      (core instance $core (instantiate $m
        (with "host" (instance
          (export "drop" (func $thing-drop))))))
      (func (export "consume") (param "h" (own $thing))
        (canon lift (core func $core "consume"))))
    "#
);

/// The interface identifier the resource import above is named by,
/// and the string a host registers the resource under on the root
/// view.
const THING: &str = "pdd-tests:host/thing@0.1.0";

#[wcmp_macros::test]
async fn it_links_a_resource_import_under_an_interface_name_through_the_root_namespace() {
    // The host registers the resource on the root view under the
    // import's whole name, and the identity it gets back mints a
    // handle the guest can drop: the destructor sees the host's rep.
    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let component = Component::new(&engine, INTERFACE_NAMED_RESOURCE)
        .await
        .expect("component parses");

    let mut linker: Linker<Arc<Mutex<Vec<u32>>>> = Linker::new(&engine);
    let type_id = linker
        .root()
        .resource(THING, |data: &mut Arc<Mutex<Vec<u32>>>, rep: u32| {
            data.lock().expect("dropped lock").push(rep);
            Ok(())
        })
        .expect("the registration");

    let dropped = Arc::new(Mutex::new(Vec::<u32>::new()));
    let mut store: Store<Arc<Mutex<Vec<u32>>>> =
        Store::new(&engine, dropped.clone()).expect("store construction succeeds");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the root registration under the interface name satisfies the import");
    let consume = instance.get_func("consume").expect("`consume` is exported");
    let handle = store
        .resource_new(type_id, 7)
        .expect("resource_new succeeds");
    let results = consume
        .call(&mut store, &[Val::Own(handle)])
        .await
        .expect("call succeeds");
    assert!(results.is_empty(), "consume returns no values");
    assert_eq!(*dropped.lock().expect("dropped lock"), vec![7]);
}

#[wcmp_macros::test]
async fn it_refuses_a_resource_import_under_an_interface_name_with_no_root_registration() {
    // Nothing registered is an unresolved import that names the
    // interface-named import — and a *linker instance* registered
    // under the same identifier does not satisfy it either, however
    // its items are labelled. The import wants one host resource,
    // and the root namespace is where one host item lives.
    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let component = Component::new(&engine, INTERFACE_NAMED_RESOURCE)
        .await
        .expect("component parses");
    let thing: InterfaceIdentifier = THING.parse().expect("identifier parses");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");

    let empty: Linker<()> = Linker::new(&engine);
    let err = match empty.instantiate(&mut store, &component).await {
        Ok(_) => panic!("a resource import with nothing registered must not link"),
        Err(err) => err,
    };
    assert!(
        matches!(
            &err,
            Error::Link(inner)
                if matches!(
                    &**inner,
                    LinkError::UnresolvedImport { import, .. }
                        if *import == ExternalName::Interface(thing.clone())
                )
        ),
        "expected an unresolved import naming `{THING}`, got {err:?}",
    );

    let mut by_interface: Linker<()> = Linker::new(&engine);
    by_interface
        .instance(&thing)
        .resource("thing", |_data: &mut (), _rep: u32| Ok(()))
        .expect("the registration");
    let err = match by_interface.instantiate(&mut store, &component).await {
        Ok(_) => panic!("a linker instance must not satisfy a resource import"),
        Err(err) => err,
    };
    assert!(
        matches!(
            &err,
            Error::Link(inner) if matches!(&**inner, LinkError::UnresolvedImport { .. })
        ),
        "expected an unresolved import, got {err:?}",
    );
}

/// A component that imports a core module under an interface name,
/// instantiates it, and lifts the module's `f`. A module import is
/// the third sort the root namespace answers for whatever the shape
/// of the name it is written under.
const INTERFACE_NAMED_MODULE: &[u8] = component!(
    r#"
    (component
      (import "pdd-tests:host/machine@0.1.0" (core module $m
        (export "f" (func (result i32)))))
      (core instance $i (instantiate $m))
      (func (export "f") (result u32) (canon lift (core func $i "f"))))
    "#
);

/// The interface identifier the module import above is named by,
/// and the string a host registers the module under on the root
/// view.
const MACHINE: &str = "pdd-tests:host/machine@0.1.0";

/// A core module satisfying the module type `MACHINE` is imported
/// with.
const PROVIDES_F: &[u8] = wasm!(
    r#"
    (module (func (export "f") (result i32) i32.const 101))
    "#
);

#[wcmp_macros::test]
async fn it_links_a_module_import_under_an_interface_name_through_the_root_namespace() {
    // The host registers the module on the root view under the
    // import's whole name; the component instantiates it and the
    // lifted export returns the module's value.
    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let component = Component::new(&engine, INTERFACE_NAMED_MODULE)
        .await
        .expect("component parses");
    let module = Module::new(&engine, PROVIDES_F)
        .await
        .expect("the module compiles");

    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .module(MACHINE, &module)
        .expect("the registration");

    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the root registration under the interface name satisfies the import");
    let f = instance
        .get_func("f")
        .expect("`f` is exported")
        .typed::<(), u32>()
        .expect("typed conversion succeeds");
    assert_eq!(f.call(&mut store, ()).await.expect("call succeeds"), 101);
}

#[wcmp_macros::test]
async fn it_refuses_a_module_import_under_an_interface_name_with_no_root_registration() {
    // As for the function and the resource: nothing registered is an
    // unresolved import naming the interface-named import, and a
    // linker instance under the same identifier holds items rather
    // than the single item the import asks for.
    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let component = Component::new(&engine, INTERFACE_NAMED_MODULE)
        .await
        .expect("component parses");
    let module = Module::new(&engine, PROVIDES_F)
        .await
        .expect("the module compiles");
    let machine: InterfaceIdentifier = MACHINE.parse().expect("identifier parses");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");

    let empty: Linker<()> = Linker::new(&engine);
    let err = match empty.instantiate(&mut store, &component).await {
        Ok(_) => panic!("a module import with nothing registered must not link"),
        Err(err) => err,
    };
    assert!(
        matches!(
            &err,
            Error::Link(inner)
                if matches!(
                    &**inner,
                    LinkError::UnresolvedImport { import, .. }
                        if *import == ExternalName::Interface(machine.clone())
                )
        ),
        "expected an unresolved import naming `{MACHINE}`, got {err:?}",
    );

    let mut by_interface: Linker<()> = Linker::new(&engine);
    by_interface
        .instance(&machine)
        .module("m", &module)
        .expect("the registration");
    let err = match by_interface.instantiate(&mut store, &component).await {
        Ok(_) => panic!("a linker instance must not satisfy a module import"),
        Err(err) => err,
    };
    assert!(
        matches!(
            &err,
            Error::Link(inner) if matches!(&**inner, LinkError::UnresolvedImport { .. })
        ),
        "expected an unresolved import, got {err:?}",
    );
}

/// A component that imports a *type* under an interface name. The
/// type is a record the component also defines, and the import
/// asserts the two are equal.
const INTERFACE_NAMED_TYPE: &[u8] = component!(
    r#"
    (component
      (type $point (record (field "x" u32) (field "y" u32)))
      (import "pdd-tests:host/point@0.1.0" (type (eq $point))))
    "#
);

#[wcmp_macros::test]
async fn it_drops_a_type_import_under_an_interface_name_before_resolution() {
    // A type import that is not a resource asks the host for
    // nothing: it carries type information for the component's own
    // use, and translation drops it rather than recording an import
    // the host would have to answer. The component declares no
    // import at all, so an empty linker instantiates it and the
    // resolver never sees the type sort. The resolver's refusal of
    // a type import is covered as a unit test of `resolve_root`,
    // which is the only way to present that arm a type at all.
    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let component = Component::new(&engine, INTERFACE_NAMED_TYPE)
        .await
        .expect("component parses");
    assert!(
        component.imports.is_empty(),
        "a non-resource type import is not a host import, got {:?}",
        component.imports,
    );

    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    linker
        .instantiate(&mut store, &component)
        .await
        .expect("a component whose only type import was dropped needs no registration");
}

/// The declared type of a function that takes nothing and answers
/// with one `u32`, carrying the `async` effect when asked for.
///
/// The untyped registration entries take a declared type, and the
/// duplicate-registration tests below need one to hand them. What
/// the type says does not enter into whether a registration is a
/// duplicate, so every one of them registers this shape.
fn nullary_u32(async_: bool) -> FunctionType {
    FunctionType {
        parameters: vec![],
        result: Some(ValueType::Primitive(PrimitiveType::U32)),
        async_,
    }
}

/// Assert that `error` is the failure a second registration under
/// `name` produces, carrying Wasmtime's wording.
fn assert_defined_twice(error: &Error, name: &str) {
    let Error::Link(inner) = error else {
        panic!("expected a link error, got {error:?}");
    };
    assert!(
        matches!(
            &**inner,
            LinkError::DuplicateRegistration { name: reported } if reported == name
        ),
        "expected a duplicate registration naming `{name}`, got {inner:?}",
    );
    assert_eq!(
        inner.to_string(),
        format!("map entry `{name}` defined twice"),
        "the message is the one Wasmtime's name map writes",
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_second_function_registration_under_one_name() {
    // Each of the four function entries takes a name, and a name is
    // taken once. The first registration of each pair stands and the
    // second is refused, whichever of the four made either of them:
    // the rule is the name map's, not the entry's.
    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let iface: InterfaceIdentifier = ANSWERS.parse().expect("identifier parses");

    let mut linker: Linker<()> = Linker::new(&engine);
    let mut view = linker.instance(&iface);

    view.func_new("untyped", nullary_u32(false), |_call, _args, results| {
        results[0] = Val::U32(1);
        Ok(())
    })
    .expect("the first untyped registration");
    let error = view
        .func_new("untyped", nullary_u32(false), |_call, _args, results| {
            results[0] = Val::U32(2);
            Ok(())
        })
        .expect_err("the second untyped registration is refused");
    assert_defined_twice(&error, "untyped");

    view.func_wrap("typed", |_call: HostCall<'_, ()>, (): ()| Ok(1u32))
        .expect("the first typed registration");
    let error = view
        .func_wrap("typed", |_call: HostCall<'_, ()>, (): ()| Ok(2u32))
        .expect_err("the second typed registration is refused");
    assert_defined_twice(&error, "typed");

    view.func_new_concurrent(
        "untyped-async",
        nullary_u32(true),
        |_accessor: &Accessor<()>, _args: Vec<Val>| async move { Ok(vec![Val::U32(1)]) },
    )
    .expect("the first untyped concurrent registration");
    let error = view
        .func_new_concurrent(
            "untyped-async",
            nullary_u32(true),
            |_accessor: &Accessor<()>, _args: Vec<Val>| async move { Ok(vec![Val::U32(2)]) },
        )
        .expect_err("the second untyped concurrent registration is refused");
    assert_defined_twice(&error, "untyped-async");

    view.func_wrap_concurrent(
        "typed-async",
        |_accessor: &Accessor<()>, (): ()| async move { Ok(1u32) },
    )
    .expect("the first typed concurrent registration");
    let error = view
        .func_wrap_concurrent(
            "typed-async",
            |_accessor: &Accessor<()>, (): ()| async move { Ok(2u32) },
        )
        .expect_err("the second typed concurrent registration is refused");
    assert_defined_twice(&error, "typed-async");
}

#[wcmp_macros::test]
async fn it_refuses_a_second_resource_or_module_registration_under_one_name() {
    // The two resource entries and the module entry take a name the
    // same way the function entries do, and give it out once.
    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let module = Module::new(&engine, PROVIDES_F)
        .await
        .expect("the module compiles");
    let iface: InterfaceIdentifier = ANSWERS.parse().expect("identifier parses");

    let mut linker: Linker<()> = Linker::new(&engine);
    let mut view = linker.instance(&iface);

    view.resource("thing", |_data: &mut (), _rep: u32| Ok(()))
        .expect("the first resource registration");
    let error = view
        .resource("thing", |_data: &mut (), _rep: u32| Ok(()))
        .expect_err("the second resource registration is refused");
    assert_defined_twice(&error, "thing");

    // A resource registered by value is refused under a taken name
    // too. Registering one `HostResource` under two *names* is the
    // point of the by-value entry and stays allowed; what is refused
    // is the second item under one name.
    let shared = HostResource::new(|_data: &mut (), _rep: u32| Ok(()));
    view.resource_with("widget", shared.clone())
        .expect("the first by-value resource registration");
    view.resource_with("gadget", shared.clone())
        .expect("the same resource type under a second name");
    let error = view
        .resource_with("widget", shared)
        .expect_err("the second by-value registration under one name is refused");
    assert_defined_twice(&error, "widget");

    view.module("m", &module)
        .expect("the first module registration");
    let error = view
        .module("m", &module)
        .expect_err("the second module registration is refused");
    assert_defined_twice(&error, "m");
}

#[wcmp_macros::test]
async fn it_refuses_a_registration_under_a_name_another_kind_took() {
    // One entry holds one map of names, as Wasmtime's does, so the
    // kind that took a name does not matter to the kind refused
    // under it. Addressing a nested instance takes the name as
    // surely as registering an item does, though addressing the
    // same nested instance twice is not a registration at all and
    // answers with the entry already there.
    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let module = Module::new(&engine, PROVIDES_F)
        .await
        .expect("the module compiles");

    let mut linker: Linker<()> = Linker::new(&engine);
    let mut root = linker.root();

    root.func_wrap("taken", |_call: HostCall<'_, ()>, (): ()| Ok(1u32))
        .expect("the function registration");
    let error = root
        .module("taken", &module)
        .expect_err("a module under a function's name is refused");
    assert_defined_twice(&error, "taken");
    let error = root
        .resource("taken", |_data: &mut (), _rep: u32| Ok(()))
        .expect_err("a resource under a function's name is refused");
    assert_defined_twice(&error, "taken");

    // Addressing the same nested instance twice is not a second
    // registration, and the second call answers with the entry the
    // first made.
    root.instance("nested")
        .func_wrap("inner", |_call: HostCall<'_, ()>, (): ()| Ok(1u32))
        .expect("the nested registration");
    let error = root
        .instance("nested")
        .func_wrap("inner", |_call: HostCall<'_, ()>, (): ()| Ok(2u32))
        .expect_err("the second call addressed the entry the first made");
    assert_defined_twice(&error, "inner");
    let error = root
        .func_wrap("nested", |_call: HostCall<'_, ()>, (): ()| Ok(1u32))
        .expect_err("a function under a nested instance's name is refused");
    assert_defined_twice(&error, "nested");
}

#[wcmp_macros::test]
async fn it_refuses_a_duplicate_on_a_nested_view_as_it_does_on_the_root() {
    // Every view refuses, and each one's names are its own: the
    // name a nested instance gives out is not the name its parent
    // gives out, so the same name registered on both is two
    // registrations, not a duplicate.
    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let mut linker: Linker<()> = Linker::new(&engine);
    let mut root = linker.root();

    root.func_wrap("f", |_call: HostCall<'_, ()>, (): ()| Ok(1u32))
        .expect("the root registration");

    let mut nested = root.instance("host");
    nested
        .func_wrap("f", |_call: HostCall<'_, ()>, (): ()| Ok(2u32))
        .expect("the same name inside a nested instance is a name of its own");
    let error = nested
        .func_wrap("f", |_call: HostCall<'_, ()>, (): ()| Ok(3u32))
        .expect_err("the second registration on the nested view is refused");
    assert_defined_twice(&error, "f");
}

#[wcmp_macros::test]
async fn it_keeps_the_first_registration_when_it_refuses_the_second() {
    // The refusal is not a half-registration: what stood under the
    // name before the refused call is what the component links
    // against afterwards.
    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let component = Component::new(&engine, INTERFACE_NAMED_FUNCTION)
        .await
        .expect("component parses");

    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap(ANSWERS, |_call: HostCall<'_, ()>, (x,): (u32,)| Ok(x * 2))
        .expect("the first registration");
    let error = linker
        .root()
        .func_wrap(ANSWERS, |_call: HostCall<'_, ()>, (x,): (u32,)| Ok(x * 3))
        .expect_err("the second registration is refused");
    assert_defined_twice(&error, ANSWERS);

    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the first registration still satisfies the import");
    let run = instance
        .get_func("run")
        .expect("`run` is exported")
        .typed::<(), u32>()
        .expect("typed");
    assert_eq!(
        run.call(&mut store, ()).await.expect("call succeeds"),
        40,
        "the refused registration did not displace the first",
    );
}

#[wcmp_macros::test]
async fn it_lets_a_later_registration_win_when_shadowing_is_allowed() {
    // The escape hatch. A linker told to allow shadowing takes every
    // kind's second registration, and the last one made is the one
    // the resolver sees.
    let engine =
        Engine::with_backend(crate::test_backend::backend()).expect("engine construction succeeds");
    let module = Module::new(&engine, PROVIDES_F)
        .await
        .expect("the module compiles");
    let component = Component::new(&engine, INTERFACE_NAMED_FUNCTION)
        .await
        .expect("component parses");

    let mut linker: Linker<()> = Linker::new(&engine);
    linker.allow_shadowing(true);

    let mut view = linker.root();
    view.func_new("untyped", nullary_u32(false), |_call, _args, results| {
        results[0] = Val::U32(1);
        Ok(())
    })
    .expect("the first untyped registration");
    view.func_new("untyped", nullary_u32(false), |_call, _args, results| {
        results[0] = Val::U32(2);
        Ok(())
    })
    .expect("the second untyped registration replaces the first");
    view.func_wrap("typed", |_call: HostCall<'_, ()>, (): ()| Ok(1u32))
        .expect("the first typed registration");
    view.func_wrap("typed", |_call: HostCall<'_, ()>, (): ()| Ok(2u32))
        .expect("the second typed registration replaces the first");
    view.func_new_concurrent(
        "untyped-async",
        nullary_u32(true),
        |_accessor: &Accessor<()>, _args: Vec<Val>| async move { Ok(vec![Val::U32(1)]) },
    )
    .expect("the first untyped concurrent registration");
    view.func_new_concurrent(
        "untyped-async",
        nullary_u32(true),
        |_accessor: &Accessor<()>, _args: Vec<Val>| async move { Ok(vec![Val::U32(2)]) },
    )
    .expect("the second untyped concurrent registration replaces the first");
    view.func_wrap_concurrent(
        "typed-async",
        |_accessor: &Accessor<()>, (): ()| async move { Ok(1u32) },
    )
    .expect("the first typed concurrent registration");
    view.func_wrap_concurrent(
        "typed-async",
        |_accessor: &Accessor<()>, (): ()| async move { Ok(2u32) },
    )
    .expect("the second typed concurrent registration replaces the first");
    view.resource("thing", |_data: &mut (), _rep: u32| Ok(()))
        .expect("the first resource registration");
    view.resource("thing", |_data: &mut (), _rep: u32| Ok(()))
        .expect("the second resource registration replaces the first");
    view.resource_with(
        "widget",
        HostResource::new(|_data: &mut (), _rep: u32| Ok(())),
    )
    .expect("the first by-value resource registration");
    view.resource_with(
        "widget",
        HostResource::new(|_data: &mut (), _rep: u32| Ok(())),
    )
    .expect("the second by-value resource registration replaces the first");
    view.module("m", &module)
        .expect("the first module registration");
    view.module("m", &module)
        .expect("the second module registration replaces the first");

    // Which of the two a component links against is the second.
    view.func_wrap(ANSWERS, |_call: HostCall<'_, ()>, (x,): (u32,)| Ok(x * 2))
        .expect("the first registration under the import's name");
    view.func_wrap(ANSWERS, |_call: HostCall<'_, ()>, (x,): (u32,)| Ok(x * 3))
        .expect("the second registration under the import's name");

    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the shadowed registration satisfies the import");
    let run = instance
        .get_func("run")
        .expect("`run` is exported")
        .typed::<(), u32>()
        .expect("typed");
    assert_eq!(
        run.call(&mut store, ()).await.expect("call succeeds"),
        60,
        "the last registration made is the one the resolver saw",
    );
}

#[path = "support/backend.rs"]
mod test_backend;
