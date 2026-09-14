//! Round-trip tests for the canonical ABI. These exercise lift and
//! lower for every value type the synchronous baseline supports, in
//! both argument and result position. A handful of stubs at the
//! bottom track capabilities the polyfill does not yet implement —
//! see PDD003's "Canonical ABI" row for the long-term scope.
//!
//! Each export-call test instantiates a component whose core module
//! is a near-identity function: arguments come in via the canonical
//! ABI, the core module rewrites them into a deterministic shape,
//! and the host asserts the lifted result matches. The shape choice
//! is deliberate — primitives that fit in flat slots travel as flat
//! slots, while heap-allocating valtypes (string, list, compound)
//! travel through a `cabi_realloc`-allocated bump-pointer arena. The
//! cabi_realloc / post-return observation tests insert host-visible
//! counters by calling out through imported functions.

#![cfg(test)]

use std::sync::{Arc, Mutex};

use wasm_component_model_polyfill::{Component, Engine, Linker, Store, Val, ValField};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// Bump-pointer `cabi_realloc` and a 1-page memory used by every
/// export that needs to lower a heap-allocating value into guest
/// memory. Inlined as a string so the per-test `component!` literal
/// can include it without repetition.
const REALLOC_AND_MEMORY: &str = r#"
    (memory (export "memory") 1)
    (global $bump (mut i32) (i32.const 16))
    (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32)
      (local $ptr i32)
      global.get $bump
      local.set $ptr
      global.get $bump
      local.get 3
      i32.add
      global.set $bump
      local.get $ptr)
"#;

#[wcmp_macros::test]
async fn it_round_trips_every_primitive_through_an_export() {
    // Components cannot easily round-trip every primitive in one
    // signature without a record/tuple wrapper; this exercises the
    // primitives whose flat encodings differ (i32 / i64 / f32 /
    // f64), proving each maps to the correct core ValType.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (core module $m
            (func (export "id-s32") (param i32) (result i32) local.get 0)
            (func (export "id-u64") (param i64) (result i64) local.get 0)
            (func (export "id-f32") (param f32) (result f32) local.get 0)
            (func (export "id-f64") (param f64) (result f64) local.get 0))
          (core instance $i (instantiate $m))
          (func (export "id-s32") (param "v" s32) (result s32)
            (canon lift (core func $i "id-s32")))
          (func (export "id-u64") (param "v" u64) (result u64)
            (canon lift (core func $i "id-u64")))
          (func (export "id-f32") (param "v" f32) (result f32)
            (canon lift (core func $i "id-f32")))
          (func (export "id-f64") (param "v" f64) (result f64)
            (canon lift (core func $i "id-f64"))))
        "#
    );
    let (mut store, instance) = instantiate(COMPONENT);
    assert_eq!(
        call(&instance, &mut store, "id-s32", &[Val::S32(-7)]).as_ref(),
        &[Val::S32(-7)]
    );
    assert_eq!(
        call(&instance, &mut store, "id-u64", &[Val::U64(u64::MAX - 3)]).as_ref(),
        &[Val::U64(u64::MAX - 3)]
    );
    assert_eq!(
        call(&instance, &mut store, "id-f32", &[Val::F32(1.5)]).as_ref(),
        &[Val::F32(1.5)]
    );
    assert_eq!(
        call(&instance, &mut store, "id-f64", &[Val::F64(-2.25)]).as_ref(),
        &[Val::F64(-2.25)]
    );
}

#[wcmp_macros::test]
async fn it_round_trips_bool_and_char_values_through_an_export() {
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (core module $m
            (func (export "id-bool") (param i32) (result i32) local.get 0)
            (func (export "id-char") (param i32) (result i32) local.get 0))
          (core instance $i (instantiate $m))
          (func (export "id-bool") (param "v" bool) (result bool)
            (canon lift (core func $i "id-bool")))
          (func (export "id-char") (param "v" char) (result char)
            (canon lift (core func $i "id-char"))))
        "#
    );
    let (mut store, instance) = instantiate(COMPONENT);
    assert_eq!(
        call(&instance, &mut store, "id-bool", &[Val::Bool(true)]).as_ref(),
        &[Val::Bool(true)]
    );
    assert_eq!(
        call(&instance, &mut store, "id-bool", &[Val::Bool(false)]).as_ref(),
        &[Val::Bool(false)]
    );
    // U+1F4A1 LIGHT BULB — a 4-byte UTF-8 / surrogate-pair UTF-16
    // scalar that catches sloppy width handling.
    assert_eq!(
        call(&instance, &mut store, "id-char", &[Val::Char('💡')]).as_ref(),
        &[Val::Char('💡')]
    );
}

#[wcmp_macros::test]
async fn it_passes_a_record_argument_to_a_host_function() {
    // Records appear as imported-instance items inside the
    // component's WIT-encoded instance type, where they do not
    // need to be top-level type exports. The trampoline's
    // lift-from-flat-slots path is exercised symmetrically with
    // the export-side lower path.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (type $iface (instance
            (type $rec' (record (field "a" s32) (field "b" s32)))
            (export "rec" (type $rec (eq $rec')))
            (type $sum-ty (func (param "r" $rec) (result s32)))
            (export "sum" (func (type $sum-ty)))))
          (import "pdd-tests:host/maths@0.1.0" (instance $imports (type $iface)))
          (alias export $imports "sum" (func $sum))
          (core func $core-sum (canon lower (func $sum)))
          (core module $m
            (func (import "host" "sum") (param i32 i32) (result i32))
            (func (export "go") (param i32 i32) (result i32)
              local.get 0
              local.get 1
              call 0))
          (core instance $i (instantiate $m
            (with "host" (instance
              (export "sum" (func $core-sum))))))
          (func (export "go") (param "a" s32) (param "b" s32) (result s32)
            (canon lift (core func $i "go"))))
        "#
    );
    use wasm_component_model_polyfill::{
        FunctionParameter, FunctionType, InterfaceIdentifier, PrimitiveType, RecordField,
        RecordType, ValField, ValueType,
    };
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, COMPONENT).expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd-tests:host/maths@0.1.0".parse().expect("identifier");
    let record_ty = ValueType::Record(RecordType::new([
        RecordField::new("a", ValueType::Primitive(PrimitiveType::S32)),
        RecordField::new("b", ValueType::Primitive(PrimitiveType::S32)),
    ]));
    linker.instance(&iface).func_new(
        "sum",
        FunctionType {
            parameters: vec![FunctionParameter {
                name: "r".to_owned(),
                ty: record_ty,
            }],
            result: Some(ValueType::Primitive(PrimitiveType::S32)),
        },
        |_: &mut (), args, results| {
            let Val::Record(fields) = &args[0] else {
                panic!("expected record");
            };
            let mut sum = 0i32;
            for ValField { value, .. } in fields.iter() {
                if let Val::S32(v) = value {
                    sum += v;
                }
            }
            results[0] = Val::S32(sum);
            Ok(())
        },
    );
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let inst = linker
        .instantiate(&mut store, &component)
        .expect("instantiate");
    let go = inst.get_func(&mut store, "go").expect("go export");
    let result = go
        .call(&mut store, &[Val::S32(7), Val::S32(35)])
        .expect("call");
    assert_eq!(result.as_ref(), &[Val::S32(42)]);
}

#[wcmp_macros::test]
async fn it_returns_a_record_from_an_export() {
    // Returning `(record (field "a" s32) (field "b" s32))` exercises
    // the wide-result memory-pointer path: the result has 2 flat
    // slots (i32, i32) > MAX_FLAT_RESULTS=1, so the core function
    // calls `cabi_realloc`, writes the two i32 fields, and returns
    // the result-area pointer. The polyfill's `lift_result` then
    // routes through `abi::lift` on `ValueType::Record`.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (import "test:host/shapes@0.1.0" (instance $iface
            (type $rec' (record (field "a" s32) (field "b" s32)))
            (export "rec" (type $rec (eq $rec')))))
          (alias export $iface "rec" (type $rec))
          (core module $m
            (memory (export "memory") 1)
            (global $bump (mut i32) (i32.const 16))
            (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32)
              (local $ptr i32)
              global.get $bump local.set $ptr
              global.get $bump local.get 3 i32.add global.set $bump
              local.get $ptr)
            (func (export "make") (param i32 i32) (result i32)
              (local $ptr i32)
              i32.const 0 i32.const 0 i32.const 4 i32.const 8 call 0
              local.set $ptr
              local.get $ptr local.get 0 i32.store
              local.get $ptr local.get 1 i32.store offset=4
              local.get $ptr))
          (core instance $i (instantiate $m))
          (type $make-ty (func (param "a" s32) (param "b" s32) (result $rec)))
          (func $make (type $make-ty)
            (canon lift (core func $i "make") (memory (core memory $i "memory")) (realloc (core func $i "cabi_realloc"))))
          (export "make" (func $make)))
        "#
    );

    use wasm_component_model_polyfill::{InterfaceIdentifier, ValField};
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, COMPONENT).expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "test:host/shapes@0.1.0".parse().expect("identifier");
    let _ = linker.instance(&iface);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .expect("instantiate");
    let make = instance
        .get_func(&mut store, "make")
        .expect("`make` export");
    let result = make
        .call(&mut store, &[Val::S32(7), Val::S32(11)])
        .expect("call");
    let [Val::Record(fields)] = result.as_ref() else {
        panic!("expected a single record result, got {result:?}");
    };
    assert_eq!(fields.len(), 2);
    assert_eq!(
        fields.as_ref(),
        &[
            ValField {
                name: "a".to_owned(),
                value: Val::S32(7),
            },
            ValField {
                name: "b".to_owned(),
                value: Val::S32(11),
            },
        ],
    );
}

#[wcmp_macros::test]
async fn it_passes_a_tuple_argument_to_an_export() {
    // Tuple `(s32, s32)` flattens to two flat i32 slots.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (core module $m
            (func (export "second") (param i32 i32) (result i32)
              local.get 1))
          (core instance $i (instantiate $m))
          (func (export "second") (param "t" (tuple s32 s32)) (result s32)
            (canon lift (core func $i "second"))))
        "#
    );
    let (mut store, instance) = instantiate(COMPONENT);
    let result = call(
        &instance,
        &mut store,
        "second",
        &[Val::Tuple(
            vec![Val::S32(11), Val::S32(22)].into_boxed_slice(),
        )],
    );
    assert_eq!(result.as_ref(), &[Val::S32(22)]);
}

#[wcmp_macros::test]
async fn it_returns_a_tuple_from_an_export() {
    // `(tuple s32 s32)` flattens identically to a two-i32 record;
    // this proves `lift_result` walks `ValueType::Tuple` through the
    // wide-result pointer path.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (core module $m
            (memory (export "memory") 1)
            (global $bump (mut i32) (i32.const 16))
            (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32)
              (local $ptr i32)
              global.get $bump local.set $ptr
              global.get $bump local.get 3 i32.add global.set $bump
              local.get $ptr)
            (func (export "make") (param i32 i32) (result i32)
              (local $ptr i32)
              i32.const 0 i32.const 0 i32.const 4 i32.const 8 call 0
              local.set $ptr
              local.get $ptr local.get 0 i32.store
              local.get $ptr local.get 1 i32.store offset=4
              local.get $ptr))
          (core instance $i (instantiate $m))
          (func (export "make") (param "a" s32) (param "b" s32) (result (tuple s32 s32))
            (canon lift (core func $i "make") (memory (core memory $i "memory")) (realloc (core func $i "cabi_realloc")))))
        "#
    );

    let (mut store, instance) = instantiate(COMPONENT);
    let result = call(&instance, &mut store, "make", &[Val::S32(13), Val::S32(17)]);
    let [Val::Tuple(elements)] = result.as_ref() else {
        panic!("expected a single tuple result, got {result:?}");
    };
    assert_eq!(elements.as_ref(), &[Val::S32(13), Val::S32(17)],);
}

#[wcmp_macros::test]
async fn it_passes_a_variant_argument_to_a_host_function() {
    // Variant `(case "none") (case "value" s32)` flattens to
    // (i32 disc, i32 payload).
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (type $iface (instance
            (type $v' (variant (case "none") (case "value" s32)))
            (export "v" (type $v (eq $v')))
            (type $decode-ty (func (param "v" $v) (result s32)))
            (export "decode" (func (type $decode-ty)))))
          (import "pdd-tests:host/maths@0.1.0" (instance $imports (type $iface)))
          (alias export $imports "decode" (func $decode))
          (core func $core-decode (canon lower (func $decode)))
          (core module $m
            (func (import "host" "decode") (param i32 i32) (result i32))
            (func (export "go") (param i32 i32) (result i32)
              local.get 0
              local.get 1
              call 0))
          (core instance $i (instantiate $m
            (with "host" (instance
              (export "decode" (func $core-decode))))))
          (func (export "go") (param "tag" s32) (param "payload" s32) (result s32)
            (canon lift (core func $i "go"))))
        "#
    );
    use wasm_component_model_polyfill::{
        FunctionParameter, FunctionType, InterfaceIdentifier, PrimitiveType, ValueType,
        VariantCase, VariantType,
    };
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, COMPONENT).expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd-tests:host/maths@0.1.0".parse().expect("identifier");
    let variant_ty = ValueType::Variant(VariantType::new([
        VariantCase::new("none", None),
        VariantCase::new("value", Some(ValueType::Primitive(PrimitiveType::S32))),
    ]));
    linker.instance(&iface).func_new(
        "decode",
        FunctionType {
            parameters: vec![FunctionParameter {
                name: "v".to_owned(),
                ty: variant_ty,
            }],
            result: Some(ValueType::Primitive(PrimitiveType::S32)),
        },
        |_: &mut (), args, results| {
            let Val::Variant {
                discriminant,
                payload,
            } = &args[0]
            else {
                panic!("expected variant");
            };
            let value = if discriminant == "value" {
                if let Some(boxed) = payload {
                    if let Val::S32(v) = **boxed { v } else { 0 }
                } else {
                    0
                }
            } else {
                0
            };
            results[0] = Val::S32(value);
            Ok(())
        },
    );
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let inst = linker
        .instantiate(&mut store, &component)
        .expect("instantiate");
    let go = inst.get_func(&mut store, "go").expect("go export");
    // Invoke twice: once with the "none" tag, once with "value 99".
    assert_eq!(
        go.call(&mut store, &[Val::S32(0), Val::S32(0)])
            .expect("call none")
            .as_ref(),
        &[Val::S32(0)]
    );
    assert_eq!(
        go.call(&mut store, &[Val::S32(1), Val::S32(99)])
            .expect("call value")
            .as_ref(),
        &[Val::S32(99)]
    );
}

#[wcmp_macros::test]
async fn it_passes_an_option_argument_to_an_export() {
    // Option<s32> flattens to (i32 disc, i32 payload).
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (core module $m
            (func (export "or-default") (param i32 i32) (result i32)
              (if (result i32)
                (i32.eqz (local.get 0))
                (then (i32.const -1))
                (else (local.get 1)))))
          (core instance $i (instantiate $m))
          (func (export "or-default") (param "v" (option s32)) (result s32)
            (canon lift (core func $i "or-default"))))
        "#
    );
    let (mut store, instance) = instantiate(COMPONENT);
    assert_eq!(
        call(&instance, &mut store, "or-default", &[Val::Option(None)]).as_ref(),
        &[Val::S32(-1)]
    );
    assert_eq!(
        call(
            &instance,
            &mut store,
            "or-default",
            &[Val::Option(Some(Box::new(Val::S32(7))))]
        )
        .as_ref(),
        &[Val::S32(7)]
    );
}

#[wcmp_macros::test]
async fn it_passes_a_result_argument_to_an_export() {
    // Result<s32, s32> flattens to (i32 disc, i32 payload).
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (core module $m
            (func (export "decode") (param i32 i32) (result i32)
              (if (result i32)
                (i32.eqz (local.get 0))
                (then (local.get 1))
                (else (i32.sub (i32.const 0) (local.get 1))))))
          (core instance $i (instantiate $m))
          (func (export "decode") (param "r" (result s32 (error s32))) (result s32)
            (canon lift (core func $i "decode"))))
        "#
    );
    let (mut store, instance) = instantiate(COMPONENT);
    assert_eq!(
        call(
            &instance,
            &mut store,
            "decode",
            &[Val::Result(Ok(Some(Box::new(Val::S32(11)))))]
        )
        .as_ref(),
        &[Val::S32(11)]
    );
    assert_eq!(
        call(
            &instance,
            &mut store,
            "decode",
            &[Val::Result(Err(Some(Box::new(Val::S32(11)))))]
        )
        .as_ref(),
        &[Val::S32(-11)]
    );
}

#[wcmp_macros::test]
async fn it_passes_an_enum_argument_to_a_host_function() {
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (type $iface (instance
            (type $colour' (enum "red" "green" "blue"))
            (export "colour" (type $colour (eq $colour')))
            (type $decode-ty (func (param "c" $colour) (result s32)))
            (export "decode" (func (type $decode-ty)))))
          (import "pdd-tests:host/probe@0.1.0" (instance $imports (type $iface)))
          (alias export $imports "decode" (func $decode))
          (core func $core-decode (canon lower (func $decode)))
          (core module $m
            (func (import "host" "decode") (param i32) (result i32))
            (func (export "go") (param i32) (result i32)
              local.get 0
              call 0))
          (core instance $i (instantiate $m
            (with "host" (instance
              (export "decode" (func $core-decode))))))
          (func (export "go") (param "n" s32) (result s32)
            (canon lift (core func $i "go"))))
        "#
    );
    use wasm_component_model_polyfill::{
        EnumType, FunctionParameter, FunctionType, InterfaceIdentifier, PrimitiveType, ValueType,
    };
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, COMPONENT).expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd-tests:host/probe@0.1.0".parse().expect("identifier");
    linker.instance(&iface).func_new(
        "decode",
        FunctionType {
            parameters: vec![FunctionParameter {
                name: "c".to_owned(),
                ty: ValueType::Enum(EnumType::new(["red".into(), "green".into(), "blue".into()])),
            }],
            result: Some(ValueType::Primitive(PrimitiveType::S32)),
        },
        |_: &mut (), args, results| {
            let Val::Enum(case) = &args[0] else {
                panic!("expected enum");
            };
            let v = match case.as_str() {
                "red" => 100,
                "green" => 101,
                "blue" => 102,
                _ => panic!("unknown enum case"),
            };
            results[0] = Val::S32(v);
            Ok(())
        },
    );
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let inst = linker
        .instantiate(&mut store, &component)
        .expect("instantiate");
    let go = inst.get_func(&mut store, "go").expect("go export");
    assert_eq!(
        go.call(&mut store, &[Val::S32(1)]).expect("call").as_ref(),
        &[Val::S32(101)]
    );
    assert_eq!(
        go.call(&mut store, &[Val::S32(2)]).expect("call").as_ref(),
        &[Val::S32(102)]
    );
}

#[wcmp_macros::test]
async fn it_passes_a_flags_argument_to_a_host_function() {
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (type $iface (instance
            (type $perm' (flags "read" "write" "execute"))
            (export "perm" (type $perm (eq $perm')))
            (type $popcount-ty (func (param "p" $perm) (result u32)))
            (export "popcount" (func (type $popcount-ty)))))
          (import "pdd-tests:host/probe@0.1.0" (instance $imports (type $iface)))
          (alias export $imports "popcount" (func $popcount))
          (core func $core-popcount (canon lower (func $popcount)))
          (core module $m
            (func (import "host" "popcount") (param i32) (result i32))
            (func (export "go") (param i32) (result i32)
              local.get 0
              call 0))
          (core instance $i (instantiate $m
            (with "host" (instance
              (export "popcount" (func $core-popcount))))))
          (func (export "go") (param "bits" u32) (result u32)
            (canon lift (core func $i "go"))))
        "#
    );
    use wasm_component_model_polyfill::{
        FlagsType, FunctionParameter, FunctionType, InterfaceIdentifier, PrimitiveType, ValueType,
    };
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, COMPONENT).expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd-tests:host/probe@0.1.0".parse().expect("identifier");
    linker.instance(&iface).func_new(
        "popcount",
        FunctionType {
            parameters: vec![FunctionParameter {
                name: "p".to_owned(),
                ty: ValueType::Flags(FlagsType::new([
                    "read".into(),
                    "write".into(),
                    "execute".into(),
                ])),
            }],
            result: Some(ValueType::Primitive(PrimitiveType::U32)),
        },
        |_: &mut (), args, results| {
            let Val::Flags(active) = &args[0] else {
                panic!("expected flags");
            };
            results[0] = Val::U32(active.len() as u32);
            Ok(())
        },
    );
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let inst = linker
        .instantiate(&mut store, &component)
        .expect("instantiate");
    let go = inst.get_func(&mut store, "go").expect("go export");
    // Bits 0b101 = read + execute.
    assert_eq!(
        go.call(&mut store, &[Val::U32(0b101)])
            .expect("call")
            .as_ref(),
        &[Val::U32(2)]
    );
}

#[wcmp_macros::test]
async fn it_round_trips_list_of_signed_integers_through_an_export() {
    // The component takes a list<s32> and returns the third element.
    // The host-side exercise asserts the lift+lower of a non-zero
    // length list along its full memory path: cabi_realloc allocates
    // 4 * 4 bytes, the polyfill writes the element values, lowers
    // (ptr, len), the guest reads element 2, and lifts the i32.
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
            (func (export "third") (param i32 i32) (result i32)
              local.get 0
              i32.const 8
              i32.add
              i32.load))
          (core instance $i (instantiate $m))
          (func (export "third") (param "xs" (list s32)) (result s32)
            (canon lift (core func $i "third")
                       (memory (core memory $i "memory"))
                       (realloc (core func $i "cabi_realloc")))))
        "#
    );
    let (mut store, instance) = instantiate(COMPONENT);
    let result = call(
        &instance,
        &mut store,
        "third",
        &[Val::List(
            vec![Val::S32(10), Val::S32(20), Val::S32(30), Val::S32(40)].into_boxed_slice(),
        )],
    );
    assert_eq!(result.as_ref(), &[Val::S32(30)]);
}

#[wcmp_macros::test]
async fn it_round_trips_list_of_bytes_through_an_export() {
    // list<u8> exercises the byte-slab encoding the polyfill is
    // free to specialise; observable behaviour must match.
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
            (func (export "byte-at") (param i32 i32 i32) (result i32)
              local.get 0
              local.get 2
              i32.add
              i32.load8_u))
          (core instance $i (instantiate $m))
          (func (export "byte-at") (param "xs" (list u8)) (param "i" u32) (result u8)
            (canon lift (core func $i "byte-at")
                       (memory (core memory $i "memory"))
                       (realloc (core func $i "cabi_realloc")))))
        "#
    );
    let (mut store, instance) = instantiate(COMPONENT);
    let bytes: Vec<Val> = (0u8..=4).map(Val::U8).collect();
    let result = call(
        &instance,
        &mut store,
        "byte-at",
        &[Val::List(bytes.into_boxed_slice()), Val::U32(2)],
    );
    assert_eq!(result.as_ref(), &[Val::U8(2)]);
}

#[wcmp_macros::test]
async fn it_observes_cabi_realloc_during_string_lower() {
    // The component imports a host counter the realloc call
    // increments. Lowering a string into guest memory has to call
    // realloc; the host observes the call by reading the counter
    // afterwards.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (type $iface (instance
            (export "bump" (func (param "n" s32)))))
          (import "pdd-tests:host/probe@0.1.0" (instance $imports (type $iface)))
          (alias export $imports "bump" (func $bump))
          (core func $core-bump (canon lower (func $bump)))
          (core module $m
            (func (import "host" "bump") (param i32))
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
              local.get 3
              call 0
              local.get $ptr)
            (func (export "len") (param i32 i32) (result i32)
              local.get 1))
          (core instance $i (instantiate $m
            (with "host" (instance
              (export "bump" (func $core-bump))))))
          (func (export "len") (param "s" string) (result s32)
            (canon lift (core func $i "len")
                       (memory (core memory $i "memory"))
                       (realloc (core func $i "cabi_realloc")))))
        "#
    );
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, COMPONENT).expect("component parses");
    let mut linker: Linker<i32> = Linker::new(&engine);
    let iface: wasm_component_model_polyfill::InterfaceIdentifier =
        "pdd-tests:host/probe@0.1.0".parse().expect("identifier");
    linker.instance(&iface).func_wrap(
        "bump",
        |data: &mut i32, (n,): (i32,)| -> wasm_component_model_polyfill::Result<()> {
            *data += n;
            Ok(())
        },
    );
    let mut store: Store<i32> = Store::new(&engine, 0).expect("store");
    let inst = linker
        .instantiate(&mut store, &component)
        .expect("instantiate");
    let len = inst
        .get_func(&mut store, "len")
        .expect("len export present");
    let results = len
        .call(&mut store, &[Val::String("hello".to_owned())])
        .expect("call");
    assert_eq!(results.as_ref(), &[Val::S32(5)]);
    // The realloc fired once for the 5-byte UTF-8 payload.
    assert_eq!(*store.data(), 5);
}

#[wcmp_macros::test]
async fn it_invokes_post_return_after_a_sync_lift() {
    // The component's `post-return` calls a host counter once. The
    // export's body is the "string-length" pattern; the host
    // observes the post-return having fired by reading the counter
    // after the call returns.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (type $iface (instance
            (export "tick" (func))))
          (import "pdd-tests:host/probe@0.1.0" (instance $imports (type $iface)))
          (alias export $imports "tick" (func $tick))
          (core func $core-tick (canon lower (func $tick)))
          (core module $m
            (func (import "host" "tick"))
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
            (func (export "len") (param i32 i32) (result i32)
              local.get 1)
            (func (export "after") (param i32)
              call 0))
          (core instance $i (instantiate $m
            (with "host" (instance
              (export "tick" (func $core-tick))))))
          (func (export "len") (param "s" string) (result s32)
            (canon lift (core func $i "len")
                       (memory (core memory $i "memory"))
                       (realloc (core func $i "cabi_realloc"))
                       (post-return (core func $i "after")))))
        "#
    );
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, COMPONENT).expect("component parses");
    let mut linker: Linker<u32> = Linker::new(&engine);
    let iface: wasm_component_model_polyfill::InterfaceIdentifier =
        "pdd-tests:host/probe@0.1.0".parse().expect("identifier");
    linker.instance(&iface).func_wrap(
        "tick",
        |data: &mut u32, (): ()| -> wasm_component_model_polyfill::Result<()> {
            *data += 1;
            Ok(())
        },
    );
    let mut store: Store<u32> = Store::new(&engine, 0).expect("store");
    let inst = linker
        .instantiate(&mut store, &component)
        .expect("instantiate");
    let len = inst.get_func(&mut store, "len").expect("len export");
    assert_eq!(*store.data(), 0, "post-return has not run yet");
    let results = len
        .call(&mut store, &[Val::String("ab".to_owned())])
        .expect("call");
    assert_eq!(results.as_ref(), &[Val::S32(2)]);
    // After the call returns, post-return has been invoked exactly
    // once.
    assert_eq!(*store.data(), 1, "post-return fired exactly once");
}

#[wcmp_macros::test]
async fn it_tracks_resource_handles_in_a_handle_table() {
    // Drive the per-store handle table directly: mint three
    // handles, drop the middle, mint a fourth, and assert the
    // canonical-ABI's index-allocation rules hold — the freed slot
    // is reused, never aliased while live.
    use wasm_component_model_polyfill::ResourceTypeId;
    let engine = Engine::new().expect("engine");
    let store: Store<()> = Store::new(&engine, ()).expect("store");

    // Two distinct registered resource types live in the same
    // store; their indices must not collide.
    let type_a = ResourceTypeId::fresh();
    let type_b = ResourceTypeId::fresh();

    let h0 = store.resource_new(type_a, 100).expect("mint a0");
    let h1 = store.resource_new(type_a, 101).expect("mint a1");
    let h2 = store.resource_new(type_a, 102).expect("mint a2");
    assert_ne!(h0.index, h1.index, "indices are non-aliasing while live");
    assert_ne!(h1.index, h2.index);
    assert_ne!(h0.index, h2.index);

    // A different resource type's table is independent.
    let b0 = store.resource_new(type_b, 200).expect("mint b0");
    assert_eq!(b0.index, h0.index, "tables are keyed by resource type");

    // Free the middle slot through the public surface: drop the
    // entry by removing it via the per-type table guard. We do
    // this by lowering through the lift/lower paths in production;
    // here, a structural assertion via the registered type id is
    // enough.
    let mut tables = store.tables.lock().expect("tables");
    assert_eq!(tables.for_type_mut(type_a).remove(h1.index), Some(101));
    drop(tables);

    let h3 = store.resource_new(type_a, 103).expect("mint a3 reuses h1");
    assert_eq!(
        h3.index, h1.index,
        "freed indices are reused deterministically (LIFO free list)"
    );
}

// --------------------------------------------------------------
// Stubs: capabilities the polyfill does not yet realise. Each
// names a specific shortcoming so the next PDD that lands the
// capability has a clear test to un-stub.
// --------------------------------------------------------------

#[wcmp_macros::test]
async fn it_supports_the_utf16_string_encoding() {
    // The export takes a string and returns its length in UTF-16
    // code units. Both lift and lower walk the
    // `StringEncoding::Utf16` arms; non-ASCII content (`café`) makes
    // the encoding choice observable — UTF-8 length is 5 bytes,
    // UTF-16 length is 4 code units.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (core module $m
            (memory (export "memory") 1)
            (global $bump (mut i32) (i32.const 16))
            (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32)
              (local $ptr i32)
              global.get $bump local.set $ptr
              global.get $bump local.get 3 i32.add global.set $bump
              local.get $ptr)
            (func (export "len") (param i32 i32) (result i32)
              local.get 1))
          (core instance $i (instantiate $m))
          (func (export "len") (param "s" string) (result s32)
            (canon lift (core func $i "len")
                       string-encoding=utf16
                       (memory (core memory $i "memory"))
                       (realloc (core func $i "cabi_realloc")))))
        "#
    );

    let (mut store, instance) = instantiate(COMPONENT);
    let result = call(
        &instance,
        &mut store,
        "len",
        &[Val::String("café".to_owned())],
    );
    assert_eq!(result.as_ref(), &[Val::S32(4)]);
}

#[wcmp_macros::test]
async fn it_supports_typed_export_calls() {
    // Narrower complement to
    // `baseline_linking::it_supports_a_typed_export_call_surface`:
    // the typed-conversion entry point rejects a mistyped call at
    // *acquisition* — before any guest code runs — with a
    // structured `Error::TypeMismatch`.
    use wasm_component_model_polyfill::Error;
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
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, COMPONENT).expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .expect("instantiate");

    // Happy path: `(i32) -> i32` matches the export's `(s32) -> s32`.
    let typed = instance
        .get_func(&mut store, "id")
        .expect("`id` export")
        .typed::<(i32,), i32>()
        .expect("typed conversion succeeds");
    assert_eq!(typed.call(&mut store, (42,)).expect("typed call"), 42);

    // Mismatch path: requesting `(i32) -> i64` against `(s32) -> s32`
    // surfaces a structured `Error::TypeMismatch` before any call.
    let mismatch = instance
        .get_func(&mut store, "id")
        .expect("`id` export")
        .typed::<(i32,), i64>()
        .expect_err("typed conversion rejects a mismatched return type");
    assert!(
        matches!(mismatch, Error::TypeMismatch(_)),
        "expected Error::TypeMismatch, got {mismatch:?}",
    );
}

// `it_returns_multiple_results_from_an_export` was removed once we
// confirmed the post-MVP canonical ABI restricts each function to at
// most one result value; multi-return is the historical MVP shape.
// The modern WIT idiom uses `tuple<…>` for "many returns at once",
// which is exercised by `it_returns_a_tuple_from_an_export`. The
// polyfill's `FunctionType.result: Option<ValueType>` matches the
// current spec and intentionally does not model the obsolete
// multi-result shape.

#[wcmp_macros::test]
async fn it_observes_cabi_realloc_alignment_for_record_allocations() {
    // The component returns a wide record whose canonical-ABI
    // alignment is 4 (two `s32` fields). The wide-result path drives
    // the core function's `cabi_realloc` with `(0, 0, 4, 8)` — the
    // third argument is the requested alignment. We bridge the
    // alignment slot out through a host import the realloc
    // immediately calls, then assert the host observed the
    // alignment the polyfill computed.
    use wasm_component_model_polyfill::{
        FunctionParameter, FunctionType, InterfaceIdentifier, PrimitiveType, ValueType,
    };
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (type $iface (instance
            (export "record-align" (func (param "alignment" u32)))))
          (import "pdd-tests:host/probe@0.1.0" (instance $imports (type $iface)))
          (alias export $imports "record-align" (func $record-align))
          (core func $core-record-align (canon lower (func $record-align)))
          (import "test:host/shapes@0.1.0" (instance $shapes
            (type $rec' (record (field "a" s32) (field "b" s32)))
            (export "rec" (type $rec (eq $rec')))))
          (alias export $shapes "rec" (type $rec))
          (core module $m
            (func $record-align (import "host" "record-align") (param i32))
            (memory (export "memory") 1)
            (global $bump (mut i32) (i32.const 16))
            (func $cabi-realloc (export "cabi_realloc") (param i32 i32 i32 i32) (result i32)
              (local $ptr i32)
              local.get 2
              call $record-align
              global.get $bump local.set $ptr
              global.get $bump local.get 3 i32.add global.set $bump
              local.get $ptr)
            (func (export "make") (result i32)
              (local $ptr i32)
              i32.const 0 i32.const 0 i32.const 4 i32.const 8
              call $cabi-realloc
              local.set $ptr
              local.get $ptr i32.const 7 i32.store
              local.get $ptr i32.const 11 i32.store offset=4
              local.get $ptr))
          (core instance $i (instantiate $m
            (with "host" (instance
              (export "record-align" (func $core-record-align))))))
          (type $make-ty (func (result $rec)))
          (func $make (type $make-ty)
            (canon lift (core func $i "make") (memory (core memory $i "memory")) (realloc (core func $i "cabi_realloc"))))
          (export "make" (func $make)))
        "#
    );

    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, COMPONENT).expect("component parses");
    let mut linker: Linker<Arc<Mutex<Vec<u32>>>> = Linker::new(&engine);
    let probe: InterfaceIdentifier = "pdd-tests:host/probe@0.1.0".parse().expect("identifier");
    let shapes: InterfaceIdentifier = "test:host/shapes@0.1.0".parse().expect("identifier");
    let _ = linker.instance(&shapes);
    linker.instance(&probe).func_new(
        "record-align",
        FunctionType {
            parameters: vec![FunctionParameter {
                name: "alignment".to_owned(),
                ty: ValueType::Primitive(PrimitiveType::U32),
            }],
            result: None,
        },
        |observed: &mut Arc<Mutex<Vec<u32>>>, args, _| {
            let Val::U32(alignment) = args[0] else {
                panic!("expected u32 alignment");
            };
            observed.lock().expect("lock").push(alignment);
            Ok(())
        },
    );
    let observed = Arc::new(Mutex::new(Vec::<u32>::new()));
    let mut store: Store<Arc<Mutex<Vec<u32>>>> =
        Store::new(&engine, observed.clone()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .expect("instantiate");
    let make = instance
        .get_func(&mut store, "make")
        .expect("`make` export");
    let _ = make.call(&mut store, &[]).expect("call");

    let observed = observed.lock().expect("lock").clone();
    assert!(
        observed.contains(&4),
        "host should observe a cabi_realloc with alignment=4 for a two-s32 record; observed: {observed:?}",
    );
}

// --------------------------------------------------------------
// Helpers
// --------------------------------------------------------------

fn instantiate(component: &[u8]) -> (Store<()>, wasm_component_model_polyfill::Instance) {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, component).expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .expect("instantiate");
    (store, instance)
}

fn call(
    instance: &wasm_component_model_polyfill::Instance,
    store: &mut Store<()>,
    name: &str,
    args: &[Val],
) -> Box<[Val]> {
    let func = instance
        .get_func(store, name)
        .unwrap_or_else(|| panic!("`{name}` export present"));
    func.call(store, args).expect("call succeeds")
}

// Defeat dead-code on the helper constants; rust-analyzer otherwise
// flags them when no test in this file references them.
#[allow(dead_code)]
fn _unused_fixtures() {
    let _ = REALLOC_AND_MEMORY;
    let _: ValField = ValField {
        name: String::new(),
        value: Val::Bool(false),
    };
}
