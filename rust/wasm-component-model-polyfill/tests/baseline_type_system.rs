//! Baseline tests for the Component Model type system. Each test
//! parses a component declaring a value type in an export signature
//! and asserts that the polyfill's `Component::exports` projects the
//! type into the expected [`ValueType`] shape. The round-trip
//! lift/lower behaviour is exercised in `baseline_canonical_abi.rs`;
//! the tests here are the structural complement.
//!
//! The `stream` and `future` valtypes are exercised in
//! `baseline_stream_future_types.rs`. Subtyping lives in a separate,
//! forthcoming test file.

#![cfg(test)]

use wasm_component_model_polyfill::{
    Component, Engine, ExternType, ExternalName, FunctionType, InstanceType, PrimitiveType,
    ValueType,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// Resolve the [`FunctionType`] for the export named `wire_name` on
/// the component the bytes parse to. Panics if the export is absent
/// or not a function.
async fn export_signature(bytes: &[u8], wire_name: &str) -> FunctionType {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, bytes)
        .await
        .expect("component parses");
    let export = component
        .exports
        .iter()
        .find(|export| match &export.name {
            ExternalName::Plain(name) => name == wire_name,
            ExternalName::Interface(id) => id.to_string() == wire_name,
        })
        .unwrap_or_else(|| panic!("export `{wire_name}` not found"));
    match &export.ty {
        ExternType::Function(ty) => ty.clone(),
        other => panic!("export `{wire_name}` is not a function: {other:?}"),
    }
}

/// Resolve the result [`ValueType`] of the named export.
async fn export_result(bytes: &[u8], wire_name: &str) -> ValueType {
    export_signature(bytes, wire_name)
        .await
        .result
        .unwrap_or_else(|| panic!("export `{wire_name}` declares no result"))
}

/// Resolve the [`InstanceType`] of an interface-typed import.
async fn import_instance(bytes: &[u8], wire_name: &str) -> InstanceType {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, bytes)
        .await
        .expect("component parses");
    let import = component
        .imports
        .iter()
        .find(|import| match &import.name {
            ExternalName::Plain(name) => name == wire_name,
            ExternalName::Interface(id) => id.to_string() == wire_name,
        })
        .unwrap_or_else(|| panic!("import `{wire_name}` not found"));
    match &import.ty {
        ExternType::Instance(instance) => instance.clone(),
        other => panic!("import `{wire_name}` is not an instance: {other:?}"),
    }
}

/// Resolve the [`FunctionType`] of an item inside an interface-typed
/// import.
async fn import_function(bytes: &[u8], iface: &str, item: &str) -> FunctionType {
    let instance = import_instance(bytes, iface).await;
    let entry = instance
        .items
        .iter()
        .find(|i| i.name == item)
        .unwrap_or_else(|| panic!("instance import `{iface}` has no item `{item}`"));
    match &entry.ty {
        ExternType::Function(ty) => ty.clone(),
        other => panic!("import item `{iface}#{item}` is not a function: {other:?}"),
    }
}

#[wcmp_macros::test]
async fn it_supports_primitive_value_types() {
    // The polyfill projects every Component Model primitive into a
    // `ValueType::Primitive` variant. The component below declares
    // one export per primitive, taking it as its sole parameter so
    // the parse path's `lower_type` arm is exercised for each.
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
            (func (export "noop") nop)
            (func (export "noop1") (param i32) nop)
            (func (export "noop2") (param i32 i32) nop)
            (func (export "noop1l") (param i64) nop)
            (func (export "noop1f") (param f32) nop)
            (func (export "noop1d") (param f64) nop))
          (core instance $i (instantiate $m))
          (func (export "take-bool")   (param "v" bool)   (canon lift (core func $i "noop1")))
          (func (export "take-s8")     (param "v" s8)     (canon lift (core func $i "noop1")))
          (func (export "take-u8")     (param "v" u8)     (canon lift (core func $i "noop1")))
          (func (export "take-s16")    (param "v" s16)    (canon lift (core func $i "noop1")))
          (func (export "take-u16")    (param "v" u16)    (canon lift (core func $i "noop1")))
          (func (export "take-s32")    (param "v" s32)    (canon lift (core func $i "noop1")))
          (func (export "take-u32")    (param "v" u32)    (canon lift (core func $i "noop1")))
          (func (export "take-s64")    (param "v" s64)    (canon lift (core func $i "noop1l")))
          (func (export "take-u64")    (param "v" u64)    (canon lift (core func $i "noop1l")))
          (func (export "take-f32")    (param "v" f32)    (canon lift (core func $i "noop1f")))
          (func (export "take-f64")    (param "v" f64)    (canon lift (core func $i "noop1d")))
          (func (export "take-char")   (param "v" char)   (canon lift (core func $i "noop1")))
          (func (export "take-string") (param "v" string)
            (canon lift (core func $i "noop2") (memory (core memory $i "memory")) (realloc (core func $i "cabi_realloc")))))
        "#
    );

    let cases: &[(&str, PrimitiveType)] = &[
        ("take-bool", PrimitiveType::Bool),
        ("take-s8", PrimitiveType::S8),
        ("take-u8", PrimitiveType::U8),
        ("take-s16", PrimitiveType::S16),
        ("take-u16", PrimitiveType::U16),
        ("take-s32", PrimitiveType::S32),
        ("take-u32", PrimitiveType::U32),
        ("take-s64", PrimitiveType::S64),
        ("take-u64", PrimitiveType::U64),
        ("take-f32", PrimitiveType::F32),
        ("take-f64", PrimitiveType::F64),
        ("take-char", PrimitiveType::Char),
        ("take-string", PrimitiveType::String),
    ];

    for (name, expected) in cases {
        let signature = export_signature(COMPONENT, name).await;
        assert_eq!(
            signature.parameters.len(),
            1,
            "export `{name}` should take exactly one parameter",
        );
        assert_eq!(
            signature.parameters[0].ty,
            ValueType::Primitive(*expected),
            "export `{name}` projected to the wrong primitive",
        );
    }
}

#[wcmp_macros::test]
async fn it_supports_record_types() {
    // The component imports an interface declaring a record type
    // and a function returning that record. The polyfill projects
    // the record's fields in declaration order.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (type $iface (instance
            (type $rec' (record (field "x" s32) (field "y" s32)))
            (export "rec" (type $rec (eq $rec')))
            (type $make-ty (func (result $rec)))
            (export "make" (func (type $make-ty)))))
          (import "test:host/shapes@0.1.0" (instance (type $iface))))
        "#
    );

    let signature = import_function(COMPONENT, "test:host/shapes@0.1.0", "make").await;
    let result = signature.result.expect("`make` declares a result");
    let ValueType::Record(record) = result else {
        panic!("expected record result, got {result:?}");
    };
    let fields = record.fields();
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].name(), "x");
    assert_eq!(fields[1].name(), "y");
    assert_eq!(*fields[0].ty(), ValueType::Primitive(PrimitiveType::S32));
    assert_eq!(*fields[1].ty(), ValueType::Primitive(PrimitiveType::S32));
}

#[wcmp_macros::test]
async fn it_supports_variant_types() {
    // A variant with a no-payload arm and a payloaded arm; the
    // projection preserves both.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (type $iface (instance
            (type $v' (variant (case "none") (case "value" s32)))
            (export "v" (type $v (eq $v')))
            (type $make-ty (func (result $v)))
            (export "make" (func (type $make-ty)))))
          (import "test:host/shapes@0.1.0" (instance (type $iface))))
        "#
    );

    let signature = import_function(COMPONENT, "test:host/shapes@0.1.0", "make").await;
    let result = signature.result.expect("`make` declares a result");
    let ValueType::Variant(variant) = result else {
        panic!("expected variant result, got {result:?}");
    };
    let cases = variant.cases();
    assert_eq!(cases.len(), 2);
    assert_eq!(cases[0].name(), "none");
    assert!(cases[0].payload().is_none());
    assert_eq!(cases[1].name(), "value");
    assert_eq!(
        cases[1].payload().cloned(),
        Some(ValueType::Primitive(PrimitiveType::S32)),
    );
}

#[wcmp_macros::test]
async fn it_supports_list_types() {
    use wasm_component_model_polyfill::ListType;
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
            (func (export "make") (result i32) i32.const 0))
          (core instance $i (instantiate $m))
          (func (export "make") (result (list u8))
            (canon lift (core func $i "make") (memory (core memory $i "memory")) (realloc (core func $i "cabi_realloc")))))
        "#
    );

    let result = export_result(COMPONENT, "make").await;
    let ValueType::List(list) = result else {
        panic!("expected list result, got {result:?}");
    };
    assert_eq!(list, ListType::new(ValueType::Primitive(PrimitiveType::U8)),);
}

#[wcmp_macros::test]
async fn it_supports_option_types() {
    use wasm_component_model_polyfill::OptionType;
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
            (func (export "make") (result i32) i32.const 0))
          (core instance $i (instantiate $m))
          (func (export "make") (result (option s32))
            (canon lift (core func $i "make") (memory (core memory $i "memory")) (realloc (core func $i "cabi_realloc")))))
        "#
    );

    let result = export_result(COMPONENT, "make").await;
    let ValueType::Option(option) = result else {
        panic!("expected option result, got {result:?}");
    };
    assert_eq!(
        option,
        OptionType::new(ValueType::Primitive(PrimitiveType::S32)),
    );
}

#[wcmp_macros::test]
async fn it_supports_result_types() {
    use wasm_component_model_polyfill::ResultType;
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
            (func (export "make") (result i32) i32.const 0))
          (core instance $i (instantiate $m))
          (func (export "make") (result (result s32 (error string)))
            (canon lift (core func $i "make") (memory (core memory $i "memory")) (realloc (core func $i "cabi_realloc")))))
        "#
    );

    let result = export_result(COMPONENT, "make").await;
    let ValueType::Result(result_ty) = result else {
        panic!("expected result-type result, got {result:?}");
    };
    assert_eq!(
        result_ty,
        ResultType::new(
            Some(ValueType::Primitive(PrimitiveType::S32)),
            Some(ValueType::Primitive(PrimitiveType::String)),
        ),
    );
}

#[wcmp_macros::test]
async fn it_supports_tuple_types() {
    use wasm_component_model_polyfill::TupleType;
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
            (func (export "make") (result i32) i32.const 0))
          (core instance $i (instantiate $m))
          (func (export "make") (result (tuple s32 string))
            (canon lift (core func $i "make") (memory (core memory $i "memory")) (realloc (core func $i "cabi_realloc")))))
        "#
    );

    let result = export_result(COMPONENT, "make").await;
    let ValueType::Tuple(tuple) = result else {
        panic!("expected tuple result, got {result:?}");
    };
    assert_eq!(
        tuple,
        TupleType::new([
            ValueType::Primitive(PrimitiveType::S32),
            ValueType::Primitive(PrimitiveType::String),
        ]),
    );
}

#[wcmp_macros::test]
async fn it_supports_flags_types() {
    use wasm_component_model_polyfill::FlagsType;
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (type $iface (instance
            (type $f' (flags "read" "write" "execute"))
            (export "perms" (type $f (eq $f')))
            (type $take-ty (func (param "v" $f)))
            (export "take" (func (type $take-ty)))))
          (import "test:host/perms@0.1.0" (instance (type $iface))))
        "#
    );

    let signature = import_function(COMPONENT, "test:host/perms@0.1.0", "take").await;
    assert_eq!(signature.parameters.len(), 1);
    let ValueType::Flags(flags) = &signature.parameters[0].ty else {
        panic!(
            "expected flags parameter, got {:?}",
            signature.parameters[0].ty,
        );
    };
    assert_eq!(
        flags.clone(),
        FlagsType::new(["read".to_owned(), "write".to_owned(), "execute".to_owned()]),
    );
}

#[wcmp_macros::test]
async fn it_supports_enum_types() {
    use wasm_component_model_polyfill::EnumType;
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (type $iface (instance
            (type $e' (enum "low" "medium" "high"))
            (export "level" (type $e (eq $e')))
            (type $take-ty (func (param "v" $e)))
            (export "take" (func (type $take-ty)))))
          (import "test:host/levels@0.1.0" (instance (type $iface))))
        "#
    );

    let signature = import_function(COMPONENT, "test:host/levels@0.1.0", "take").await;
    assert_eq!(signature.parameters.len(), 1);
    let ValueType::Enum(en) = &signature.parameters[0].ty else {
        panic!(
            "expected enum parameter, got {:?}",
            signature.parameters[0].ty,
        );
    };
    assert_eq!(
        en.clone(),
        EnumType::new(["low".to_owned(), "medium".to_owned(), "high".to_owned()]),
    );
}

#[wcmp_macros::test]
async fn it_compares_types_structurally() {
    // Two components declare an identically-shaped record type
    // under different type names. Their projected `ValueType::Record`
    // values must compare equal — equality on `ValueType` is
    // structural, not nominal.
    const FIRST: &[u8] = component!(
        r#"
        (component
          (type $iface (instance
            (type $point' (record (field "x" s32) (field "y" s32)))
            (export "point" (type $point (eq $point')))
            (type $make-ty (func (result $point)))
            (export "make" (func (type $make-ty)))))
          (import "test:host/shapes@0.1.0" (instance (type $iface))))
        "#
    );
    const SECOND: &[u8] = component!(
        r#"
        (component
          (type $iface (instance
            (type $vector' (record (field "x" s32) (field "y" s32)))
            (export "vector" (type $vector (eq $vector')))
            (type $make-ty (func (result $vector)))
            (export "make" (func (type $make-ty)))))
          (import "test:host/vectors@0.1.0" (instance (type $iface))))
        "#
    );

    let first = import_function(FIRST, "test:host/shapes@0.1.0", "make")
        .await
        .result
        .expect("first declares result");
    let second = import_function(SECOND, "test:host/vectors@0.1.0", "make")
        .await
        .result
        .expect("second declares result");
    assert_eq!(first, second);
}

#[wcmp_macros::test]
async fn it_supports_own_resource_handles() {
    // The component imports a resource type and declares an export
    // that consumes an owning handle of it. The polyfill projects
    // the parameter to `ValueType::Own(ResourceType { label, .. })`.
    use wasm_component_model_polyfill::ResourceType;
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (import "test:guest/host@0.1.0" (instance $h
            (export "thing" (type (sub resource)))))
          (alias export $h "thing" (type $thing))
          (core func $thing-drop (canon resource.drop $thing))
          (core module $m
            (func (import "host" "drop") (param i32))
            (func (export "consume") (param i32) local.get 0 call 0))
          (core instance $core (instantiate $m
            (with "host" (instance (export "drop" (func $thing-drop))))))
          (func (export "consume") (param "h" (own $thing))
            (canon lift (core func $core "consume"))))
        "#
    );

    let signature = export_signature(COMPONENT, "consume").await;
    assert_eq!(signature.parameters.len(), 1);
    let ValueType::Own(rt) = &signature.parameters[0].ty else {
        panic!(
            "expected own<thing> parameter, got {:?}",
            signature.parameters[0].ty,
        );
    };
    assert_eq!(*rt, ResourceType::new("thing"));
}

#[wcmp_macros::test]
async fn it_supports_borrow_resource_handles() {
    // Same shape as the `own<T>` test but with a borrow handle.
    use wasm_component_model_polyfill::ResourceType;
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (import "test:guest/host@0.1.0" (instance $h
            (export "thing" (type (sub resource)))))
          (alias export $h "thing" (type $thing))
          (core module $m
            (func (export "inspect") (param i32) nop))
          (core instance $core (instantiate $m))
          (func (export "inspect") (param "h" (borrow $thing))
            (canon lift (core func $core "inspect"))))
        "#
    );

    let signature = export_signature(COMPONENT, "inspect").await;
    assert_eq!(signature.parameters.len(), 1);
    let ValueType::Borrow(rt) = &signature.parameters[0].ty else {
        panic!(
            "expected borrow<thing> parameter, got {:?}",
            signature.parameters[0].ty,
        );
    };
    assert_eq!(*rt, ResourceType::new("thing"));
}

#[wcmp_macros::test]
async fn it_runs_sync_resource_destructors() {
    // The polyfill drives a host destructor synchronously when the
    // guest drops the last handle to a resource. The structural
    // analogue here parses a component declaring an export that
    // takes an `own<T>` (so the canonical-ABI semantics demand the
    // destructor fire on a guest `resource.drop`) and asserts the
    // signature projected as expected. The full execution-time
    // assertion lives in
    // `baseline_resources::it_defines_a_host_resource_with_a_sync_destructor`.
    use wasm_component_model_polyfill::ResourceType;
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (import "test:guest/host@0.1.0" (instance $h
            (export "thing" (type (sub resource)))))
          (alias export $h "thing" (type $thing))
          (core func $thing-drop (canon resource.drop $thing))
          (core module $m
            (func (import "host" "drop") (param i32))
            (func (export "consume") (param i32) local.get 0 call 0))
          (core instance $core (instantiate $m
            (with "host" (instance (export "drop" (func $thing-drop))))))
          (func (export "consume") (param "h" (own $thing))
            (canon lift (core func $core "consume"))))
        "#
    );

    let signature = export_signature(COMPONENT, "consume").await;
    let ValueType::Own(rt) = &signature.parameters[0].ty else {
        panic!("expected own<thing> parameter");
    };
    assert_eq!(*rt, ResourceType::new("thing"));
}

#[wcmp_macros::test]
async fn it_projects_an_error_context_wherever_a_value_appears() {
    use wasm_component_model_polyfill::OptionType;

    // `error-context` projects to `ValueType::ErrorContext` as a
    // parameter, as a result, and inside a compound type. Validation
    // admits the type only with its feature enabled, so the engine
    // enables it.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (core module $m
            (memory (export "memory") 1)
            (func (export "take") (param i32))
            (func (export "give") (result i32) (i32.const 0)))
          (core instance $i (instantiate $m))
          (func (export "take") (param "e" error-context)
            (canon lift (core func $i "take")))
          (func (export "give") (result (option error-context))
            (canon lift (core func $i "give") (memory (core memory $i "memory")))))
        "#
    );
    let mut config = wasm_component_model_polyfill::EngineConfig::default();
    config.wasm_component_model_error_context(true);
    let engine = Engine::with_config(&config).expect("engine");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let signature = |name: &str| {
        let export = component
            .exports
            .iter()
            .find(|export| matches!(&export.name, ExternalName::Plain(n) if n == name))
            .unwrap_or_else(|| panic!("export `{name}` not found"));
        match &export.ty {
            ExternType::Function(ty) => ty.clone(),
            other => panic!("export `{name}` is not a function: {other:?}"),
        }
    };

    assert_eq!(signature("take").parameters[0].ty, ValueType::ErrorContext);
    assert_eq!(
        signature("give").result,
        Some(ValueType::Option(OptionType::new(ValueType::ErrorContext)))
    );
}
