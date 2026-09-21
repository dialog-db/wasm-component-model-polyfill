//! Baseline tests for core modules at the component boundary: a
//! component that exports a core module, and a host that loads,
//! inspects, and instantiates a core module itself.

#![cfg(test)]

use wasm_component_model_polyfill::{
    Component, CoreExternType, CoreValueType, Engine, Error, ExternType, ExternalName,
    InstantiationError, LinkError, Linker, Module, Store,
};
use wcmp_macros::{component, wasm};

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// A provider module: one immutable global and one function.
const PROVIDER: &[u8] = wasm!(
    r#"
    (module
      (global (export "g") i32 i32.const 5)
      (func (export "f") (param i32) (result i32) local.get 0))
    "#
);

/// A consumer module whose start function traps unless the imported
/// global holds 5.
const CONSUMER: &[u8] = wasm!(
    r#"
    (module
      (import "" "g" (global $g i32))
      (func $start
        global.get $g
        i32.const 5
        i32.ne
        if unreachable end)
      (start $start))
    "#
);

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

    let provider = Module::new(&engine, PROVIDER)
        .await
        .expect("a core module compiles from bytes");
    let consumer = Module::new(&engine, CONSUMER)
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
        .instantiate(&mut store, std::slice::from_ref(&g))
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
    let mut other_store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
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
    const WRONG_PROVIDER: &[u8] = wasm!(
        r#"
        (module (global (export "g") i32 i32.const 6))
        "#
    );
    let engine = Engine::new().expect("engine construction succeeds");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    let provider = Module::new(&engine, WRONG_PROVIDER)
        .await
        .expect("compiles");
    let consumer = Module::new(&engine, CONSUMER).await.expect("compiles");
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

/// A component that imports a core module under a plain name,
/// instantiates it, and lifts the module's `f`.
const IMPORTS_MODULE: &[u8] = component!(
    r#"
    (component
      (import "m" (core module $m
        (export "f" (func (result i32)))))
      (core instance $i (instantiate $m))
      (func (export "f") (result u32) (canon lift (core func $i "f"))))
    "#
);

#[wcmp_macros::test]
async fn it_instantiates_a_core_module_the_host_registered() {
    const PROVIDES_F: &[u8] = wasm!(
        r#"
        (module (func (export "f") (result i32) i32.const 101))
        "#
    );
    let engine = Engine::new().expect("engine construction succeeds");
    let component = Component::new(&engine, IMPORTS_MODULE)
        .await
        .expect("a component importing a core module parses");
    let module = Module::new(&engine, PROVIDES_F)
        .await
        .expect("the module compiles");

    // The import is described with its module type.
    assert_eq!(component.imports.len(), 1);
    assert_eq!(
        component.imports[0].name,
        ExternalName::Plain("m".to_owned())
    );
    let ExternType::Module(module_type) = &component.imports[0].ty else {
        panic!(
            "expected a module import, got {:?}",
            component.imports[0].ty
        );
    };
    assert_eq!(module_type.exports.len(), 1);
    assert_eq!(module_type.exports[0].name, "f");

    let mut linker: Linker<()> = Linker::new(&engine);
    linker.root().module("m", &module);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the component instantiates the registered module");
    let f = instance
        .get_func("f")
        .expect("`f` is exported")
        .typed::<(), u32>()
        .expect("typed conversion succeeds");
    assert_eq!(f.call(&mut store, ()).await.expect("call succeeds"), 101);
}

#[wcmp_macros::test]
async fn it_reports_a_missing_module_import_as_a_link_error() {
    let engine = Engine::new().expect("engine construction succeeds");
    let component = Component::new(&engine, IMPORTS_MODULE)
        .await
        .expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    let err = match linker.instantiate(&mut store, &component).await {
        Ok(_) => panic!("an unregistered module import must not link"),
        Err(err) => err,
    };
    match err {
        Error::Link(inner) => match *inner {
            LinkError::UnresolvedImport { import, .. } => {
                assert_eq!(import, ExternalName::Plain("m".to_owned()));
            }
            other => panic!("expected an unresolved import, got {other:?}"),
        },
        other => panic!("expected a link error, got {other:?}"),
    }
}

#[wcmp_macros::test]
async fn it_rejects_a_registered_module_that_does_not_satisfy_the_declared_type() {
    let engine = Engine::new().expect("engine construction succeeds");
    let component = Component::new(&engine, IMPORTS_MODULE)
        .await
        .expect("component parses");

    /// Link the component against `module` and return the reason
    /// the resolver rejected it.
    async fn reason(engine: &Engine, component: &Component, module: &Module) -> String {
        let mut linker: Linker<()> = Linker::new(engine);
        linker.root().module("m", module);
        let mut store: Store<()> = Store::new(engine, ()).expect("store");
        match linker.instantiate(&mut store, component).await {
            Err(Error::Link(inner)) => match *inner {
                LinkError::IncompatibleModule {
                    import,
                    item,
                    reason,
                } => {
                    assert_eq!(import, ExternalName::Plain("m".to_owned()));
                    assert_eq!(item, "m");
                    reason
                }
                other => panic!("expected an incompatible module, got {other:?}"),
            },
            Err(other) => panic!("expected a link error, got {other:?}"),
            Ok(_) => panic!("a module that does not satisfy the type must not link"),
        }
    }

    // A missing export.
    let empty = Module::new(&engine, wasm!("(module)"))
        .await
        .expect("compiles");
    assert_eq!(
        reason(&engine, &component, &empty).await,
        "module export `f` not defined"
    );

    // An export of the wrong type.
    let wrong_type = Module::new(
        &engine,
        wasm!(r#"(module (func (export "f") (param i32) (result i32) local.get 0))"#),
    )
    .await
    .expect("compiles");
    assert_eq!(
        reason(&engine, &component, &wrong_type).await,
        "module export `f` has the wrong type: expected type `(func (result i32))`, \
         found type `(func (param i32) (result i32))`"
    );

    // An export of the wrong kind.
    let wrong_kind = Module::new(
        &engine,
        wasm!(r#"(module (global (export "f") i32 i32.const 0))"#),
    )
    .await
    .expect("compiles");
    assert_eq!(
        reason(&engine, &component, &wrong_kind).await,
        "module export `f` has the wrong type: expected func found global"
    );

    // An import the declared type does not list.
    let extra_import = Module::new(
        &engine,
        wasm!(
            r#"
            (module
              (import "env" "something" (func))
              (func (export "f") (result i32) i32.const 1))
            "#
        ),
    )
    .await
    .expect("compiles");
    assert_eq!(
        reason(&engine, &component, &extra_import).await,
        "module import `env::something` not defined"
    );
}

#[wcmp_macros::test]
async fn it_instantiates_a_module_registered_inside_an_instance_import() {
    // Wasmtime's wast runner shape: the host registers a module as
    // an item of the `host` instance, and the component reaches it
    // through the instance.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (import "host" (instance $host
            (export "simple-module" (core module
              (export "f" (func (result i32)))
              (export "g" (global i32))))))
          (core instance $i (instantiate (module $host "simple-module")))
          (core module $verify
            (import "host" "f" (func $f (result i32)))
            (import "host" "g" (global $g i32))
            (func (export "sum") (result i32)
              call $f
              global.get $g
              i32.add))
          (core instance $v (instantiate $verify (with "host" (instance $i))))
          (func (export "sum") (result u32) (canon lift (core func $v "sum"))))
        "#
    );
    const SIMPLE: &[u8] = wasm!(
        r#"
        (module
          (global (export "g") i32 i32.const 100)
          (func (export "f") (result i32) i32.const 101))
        "#
    );
    let engine = Engine::new().expect("engine construction succeeds");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let simple = Module::new(&engine, SIMPLE).await.expect("compiles");
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .instance("host")
        .module("simple-module", &simple);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the component instantiates the module through the instance import");
    let sum = instance
        .get_func("sum")
        .expect("`sum` is exported")
        .typed::<(), u32>()
        .expect("typed");
    assert_eq!(sum.call(&mut store, ()).await.expect("call succeeds"), 201);
}

#[wcmp_macros::test]
async fn it_re_exports_an_imported_module() {
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (import "m" (core module $m (export "f" (func (result i32)))))
          (export "m2" (core module $m)))
        "#
    );
    const PROVIDES_F: &[u8] = wasm!(
        r#"
        (module (func (export "f") (result i32) i32.const 7))
        "#
    );
    let engine = Engine::new().expect("engine construction succeeds");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let module = Module::new(&engine, PROVIDES_F).await.expect("compiles");
    let mut linker: Linker<()> = Linker::new(&engine);
    linker.root().module("m", &module);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store construction succeeds");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiation succeeds");
    let re_exported = instance
        .get_module("m2")
        .expect("`m2` is the re-exported module");
    assert_eq!(re_exported.exports().len(), 1);
    assert_eq!(re_exported.exports()[0].name, "f");
    re_exported
        .instantiate(&mut store, &[])
        .await
        .expect("the re-exported module instantiates");
}
