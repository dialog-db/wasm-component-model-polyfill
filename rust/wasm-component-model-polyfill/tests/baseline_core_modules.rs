//! Baseline tests for core modules at the component boundary: a
//! component that exports a core module, and a host that loads,
//! inspects, and instantiates a core module itself.

#![cfg(test)]

use wasm_component_model_polyfill::{
    Component, CoreExternType, CoreValueType, Engine, Error, ExternType, ExternalName,
    InstantiationError, Linker, Module, Store,
};
use wcmp_macros::{component, wasm};

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// A provider module: one immutable global and one function.
const PROVIDER: &str = r#"
    (module
      (global (export "g") i32 i32.const 5)
      (func (export "f") (param i32) (result i32) local.get 0))
"#;

/// A consumer module whose start function traps unless the imported
/// global holds 5.
const CONSUMER: &str = r#"
    (module
      (import "" "g" (global $g i32))
      (func $start
        global.get $g
        i32.const 5
        i32.ne
        if unreachable end)
      (start $start))
"#;

#[wcmp_macros::test]
async fn it_exposes_a_module_typed_export_as_a_handle() {
    // The component exports a core module at the root and another
    // inside an instance-typed export. Neither is instantiated by
    // the component; the host takes the handle.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (core module $provider
            (global (export "g") i32 i32.const 5)
            (func (export "f") (param i32) (result i32) local.get 0))
          (core module $consumer
            (import "" "g" (global $g i32))
            (func $start
              global.get $g
              i32.const 5
              i32.ne
              if unreachable end)
            (start $start))
          (export "provider" (core module $provider))
          (instance $i (export "consumer" (core module $consumer)))
          (export "i" (instance $i)))
        "#
    );

    let engine = Engine::new().expect("engine construction succeeds");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("a component with module-typed exports parses");

    // The type projection describes the module's shape.
    let provider = component
        .exports
        .iter()
        .find(|export| export.name == ExternalName::Plain("provider".to_owned()))
        .expect("`provider` is a declared export");
    let ExternType::Module(module_type) = &provider.ty else {
        panic!("expected a module export, got {:?}", provider.ty);
    };
    assert!(module_type.imports.is_empty());
    assert_eq!(module_type.exports.len(), 2);
    assert_eq!(module_type.exports[0].name, "g");
    assert_eq!(
        module_type.exports[0].ty,
        CoreExternType::Global {
            content: CoreValueType::I32,
            mutable: false,
        }
    );
    assert_eq!(module_type.exports[1].name, "f");
    assert_eq!(
        module_type.exports[1].ty,
        CoreExternType::Func {
            params: vec![CoreValueType::I32],
            results: vec![CoreValueType::I32],
        }
    );

    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiation succeeds");

    // The handle at the root, through both the shorthand and the
    // navigator, describes the same module.
    let provider = instance
        .get_module("provider")
        .expect("`provider` is a module export");
    assert_eq!(provider.imports().len(), 0);
    assert_eq!(provider.exports().len(), 2);
    assert_eq!(provider.exports()[0].name, "g");
    assert_eq!(
        provider.exports()[0].ty,
        CoreExternType::Global {
            content: CoreValueType::I32,
            mutable: false,
        }
    );
    assert_eq!(provider.exports()[1].name, "f");
    assert!(instance.exports().module("provider").is_some());
    assert!(instance.get_module("i").is_none());
    assert!(instance.get_func("provider").is_none());

    // The nested handle is reached through the instance view.
    let consumer = instance
        .exports()
        .instance("i")
        .expect("`i` is an instance export")
        .module("consumer")
        .expect("`consumer` is a module export inside `i`");
    assert_eq!(consumer.imports().len(), 1);
    assert_eq!(consumer.imports()[0].module, "");
    assert_eq!(consumer.imports()[0].name, "g");
    assert_eq!(
        consumer.imports()[0].ty,
        CoreExternType::Global {
            content: CoreValueType::I32,
            mutable: false,
        }
    );
    assert!(instance.get_module("consumer").is_none());

    // The host instantiates the provider, takes its global, and
    // feeds it to the consumer, whose start function checks it.
    let provided = provider
        .instantiate(&mut store, &[])
        .await
        .expect("a module without imports instantiates");
    let g = provided
        .get_export(&store, "g")
        .expect("`g` is exported by the provider");
    assert_eq!(
        g.ty(&store),
        CoreExternType::Global {
            content: CoreValueType::I32,
            mutable: false,
        }
    );
    assert!(provided.get_export(&store, "missing").is_none());
    consumer
        .instantiate(&mut store, &[g])
        .await
        .expect("the consumer instantiates against the provider's global");
}

#[wcmp_macros::test]
async fn it_loads_a_core_module_from_bytes_and_instantiates_it() {
    let engine = Engine::new().expect("engine construction succeeds");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");

    let provider = Module::new(&engine, wasm!(PROVIDER))
        .await
        .expect("a core module compiles from bytes");
    let consumer = Module::new(&engine, wasm!(CONSUMER))
        .await
        .expect("a core module compiles from bytes");
    assert_eq!(consumer.imports().len(), 1);
    assert_eq!(consumer.exports().len(), 0);

    let provided = provider
        .instantiate(&mut store, &[])
        .await
        .expect("the provider instantiates");
    let g = provided.get_export(&store, "g").expect("`g` is exported");
    let f = provided.get_export(&store, "f").expect("`f` is exported");
    assert_eq!(
        f.ty(&store),
        CoreExternType::Func {
            params: vec![CoreValueType::I32],
            results: vec![CoreValueType::I32],
        }
    );

    // The right import satisfies the consumer's start function.
    consumer
        .instantiate(&mut store, &[g.clone()])
        .await
        .expect("the consumer instantiates");

    // Too few imports is a structured error before the substrate
    // runs.
    let err = consumer
        .instantiate(&mut store, &[])
        .await
        .expect_err("a missing import is refused");
    match err {
        Error::Instantiation(inner) => assert!(matches!(
            *inner,
            InstantiationError::ImportCount {
                expected: 1,
                found: 0
            }
        )),
        other => panic!("expected an instantiation error, got {other:?}"),
    }

    // An item of the wrong kind is refused by the substrate.
    let err = consumer
        .instantiate(&mut store, &[f])
        .await
        .expect_err("a function where a global is imported is refused");
    assert!(matches!(err, Error::Instantiation(_)));

    // A value from another store is refused before the substrate
    // sees it.
    let mut other_store: Store<()> =
        Store::new(&engine, ()).expect("store construction succeeds");
    let err = consumer
        .instantiate(&mut other_store, &[g])
        .await
        .expect_err("an import from another store is refused");
    match err {
        Error::Instantiation(inner) => {
            assert!(matches!(*inner, InstantiationError::WrongStore));
        }
        other => panic!("expected an instantiation error, got {other:?}"),
    }
}

#[wcmp_macros::test]
async fn it_reports_a_trapping_start_function_as_an_instantiation_error() {
    // A provider whose global holds the wrong value makes the
    // consumer's start function trap, so the import really flowed
    // through.
    const WRONG_PROVIDER: &str = r#"
        (module (global (export "g") i32 i32.const 6))
    "#;
    let engine = Engine::new().expect("engine construction succeeds");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    let provider = Module::new(&engine, wasm!(WRONG_PROVIDER))
        .await
        .expect("compiles");
    let consumer = Module::new(&engine, wasm!(CONSUMER)).await.expect("compiles");
    let provided = provider
        .instantiate(&mut store, &[])
        .await
        .expect("the provider instantiates");
    let g = provided.get_export(&store, "g").expect("`g` is exported");
    let err = consumer
        .instantiate(&mut store, &[g])
        .await
        .expect_err("the start function traps");
    match err {
        Error::Instantiation(inner) => {
            assert!(matches!(*inner, InstantiationError::SubstrateFailure(_)));
        }
        other => panic!("expected an instantiation error, got {other:?}"),
    }
}

#[wcmp_macros::test]
async fn it_refuses_bytes_that_are_not_a_core_module() {
    const COMPONENT: &[u8] = component!("(component)");
    let engine = Engine::new().expect("engine construction succeeds");
    assert!(Module::new(&engine, COMPONENT).await.is_err());
    assert!(Module::new(&engine, b"not wasm").await.is_err());
}
