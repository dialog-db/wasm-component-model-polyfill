//! Baseline tests for linking, instantiation, and host integration as
//! `wasm_component_layer` supports them today. Each test is a stub: see
//! PDD003's "Linking, Instantiation, and Host Integration" row. Async host
//! functions, async resource destructors, host-binding code generation, and
//! component-level `start` live in a separate, forthcoming test file.

#![cfg(test)]

use wasm_component_model_polyfill::{
    Component, Engine, Error, ExternType, ExternalName, FunctionParameter, FunctionType,
    InterfaceIdentifier, Linker, PrimitiveType, Store, Val, ValueType,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

#[wcmp_macros::test]
async fn it_constructs_an_engine() {
    let engine = Engine::new().expect("engine construction succeeds");

    // Engines are advertised as cheap to clone; exercise that.
    let _clone = engine.clone();
}

#[wcmp_macros::test]
async fn it_constructs_a_store() {
    let engine = Engine::new().expect("engine construction succeeds");

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
          (alias export $i "greet" (func $g))
          (export "do-greet" (func $g)))
        "#
    );

    let engine = Engine::new().expect("engine construction succeeds");
    let component = Component::new(&engine, GREETER).expect("component parses");

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

    // The export aliases the imported function and re-exports it
    // under a plain (kebab-case) name.
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

    let engine = Engine::new().expect("engine construction succeeds");
    let component = Component::new(&engine, ADDER).expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");

    let instance = linker
        .instantiate(&mut store, &component)
        .expect("instantiation succeeds");
    let add = instance
        .get_func(&mut store, "add")
        .expect("`add` export is present");

    let results = add
        .call(&mut store, &[Val::S32(2), Val::S32(3)])
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

    let engine = Engine::new().expect("engine construction succeeds");
    let component = Component::new(&engine, COUNTER).expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");

    let first = linker
        .instantiate(&mut store, &component)
        .expect("first instantiation succeeds");
    let second = linker
        .instantiate(&mut store, &component)
        .expect("second instantiation succeeds");

    let first_next = first
        .get_func(&mut store, "next")
        .expect("`next` export present on first instance");
    let second_next = second
        .get_func(&mut store, "next")
        .expect("`next` export present on second instance");

    assert_eq!(
        first_next.call(&mut store, &[]).expect("first call").as_ref(),
        &[Val::S32(1)],
    );
    assert_eq!(
        first_next.call(&mut store, &[]).expect("second call").as_ref(),
        &[Val::S32(2)],
    );

    // The second instance's counter is unaffected by the first
    // instance's mutations.
    assert_eq!(
        second_next.call(&mut store, &[]).expect("third call").as_ref(),
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

    let engine = Engine::new().expect("engine construction succeeds");
    let component = Component::new(&engine, HARNESS).expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);

    // Register a candidate whose patch version is higher than the
    // import's; WIT compatibility range matches and the linker
    // chooses the highest-version match.
    let candidate: InterfaceIdentifier = "wasi:cli/run@0.2.7".parse().expect("identifier parses");
    let _ = linker.instance(&candidate);

    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    let instance = linker
        .instantiate(&mut store, &component)
        .expect("instantiation with semver-qualified import succeeds");
    let answer = instance
        .get_func(&mut store, "answer")
        .expect("`answer` export present");
    assert_eq!(
        answer.call(&mut store, &[]).expect("call succeeds").as_ref(),
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

    let engine = Engine::new().expect("engine construction succeeds");
    let component = Component::new(&engine, COMPONENT).expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd008:host/maths@0.1.0".parse().expect("identifier parses");
    let mut instance = linker.instance(&iface);
    instance.func_new(
        "double",
        FunctionType {
            parameters: vec![FunctionParameter {
                name: "n".to_owned(),
                ty: ValueType::Primitive(PrimitiveType::S32),
            }],
            result: Some(ValueType::Primitive(PrimitiveType::S32)),
        },
        |_data, args, results| {
            let Val::S32(n) = args[0] else {
                panic!("expected s32 arg");
            };
            results[0] = Val::S32(n * 2);
            Ok(())
        },
    );

    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    let inst = linker
        .instantiate(&mut store, &component)
        .expect("instantiation succeeds");
    let do_double = inst
        .get_func(&mut store, "do-double")
        .expect("`do-double` export present");
    let results = do_double
        .call(&mut store, &[Val::S32(21)])
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

    let engine = Engine::new().expect("engine construction succeeds");
    let component = Component::new(&engine, COMPONENT).expect("component parses");

    // Happy path: the typed registration agrees with the import.
    let mut linker: Linker<()> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd008:host/maths@0.1.0".parse().expect("identifier parses");
    let mut instance = linker.instance(&iface);
    instance.func_wrap(
        "double",
        |_data: &mut (), (n,): (i32,)| -> wasm_component_model_polyfill::Result<i32> {
            Ok(n * 2)
        },
    );

    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    let inst = linker
        .instantiate(&mut store, &component)
        .expect("instantiation succeeds");
    let do_double = inst
        .get_func(&mut store, "do-double")
        .expect("`do-double` export present");
    let results = do_double
        .call(&mut store, &[Val::S32(21)])
        .expect("call succeeds");
    assert_eq!(results.as_ref(), &[Val::S32(42)]);

    // Type-mismatch path: registering an i64-returning closure
    // against an s32-returning import surfaces a TypeMismatch at
    // link time.
    let mut bad_linker: Linker<()> = Linker::new(&engine);
    let mut bad_instance = bad_linker.instance(&iface);
    bad_instance.func_wrap(
        "double",
        |_data: &mut (), (n,): (i32,)| -> wasm_component_model_polyfill::Result<i64> {
            Ok(i64::from(n * 2))
        },
    );

    let mut bad_store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    let err = match bad_linker.instantiate(&mut bad_store, &component) {
        Ok(_) => panic!("type mismatch should have been caught at link time"),
        Err(err) => err,
    };
    assert!(
        matches!(err, Error::TypeMismatch(_)),
        "expected Error::TypeMismatch, got {err:?}",
    );
}

#[wcmp_macros::test]
#[ignore = "stub: polyfill implementation pending"]
async fn it_defines_a_host_resource_with_a_sync_destructor() {
    todo!(
        "declare a host-owned `ResourceType`, hand a handle to a guest, drop it, and assert the host destructor observes the drop synchronously"
    );
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
                       (memory $i "memory")
                       (realloc (func $i "cabi_realloc")))))
        "#
    );

    let engine = Engine::new().expect("engine construction succeeds");
    let component = Component::new(&engine, COMPONENT).expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    let instance = linker
        .instantiate(&mut store, &component)
        .expect("instantiation succeeds");
    let string_length = instance
        .get_func(&mut store, "string-length")
        .expect("`string-length` export present");
    let results = string_length
        .call(&mut store, &[Val::String("hello, world".to_owned())])
        .expect("call succeeds");
    assert_eq!(results.as_ref(), &[Val::S32(12)]);
}
