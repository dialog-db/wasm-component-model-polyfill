//! Baseline tests for the export lifted `canon lift async` with a
//! callback.
//!
//! The translator accepts such an export: its callback is extracted
//! from the core instance into the instance's runtime state beside
//! the reallocs and the post-returns, and the public function type
//! says the function is `async`, under the name Wasmtime gives the
//! fact. A host acquires a typed handle to a callback export with
//! the code a synchronous export takes, because the parameters and
//! the result of the two forms are the same.
//!
//! The runtime that reads the status word a callback export returns
//! is not built yet, so a host call into one is refused rather than
//! lifting the word as though it were the result. The stackful form
//! of the lift, the one with no callback, is refused at translation,
//! and so is an `async` function type on an import: no host function
//! the polyfill registers can satisfy one.

#![cfg(test)]

use wasm_component_model_polyfill::{
    Component, Engine, EngineConfig, Error, ExternType, ExternalName, FunctionType, Linker, Store,
    Val,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// One core instance behind two exports: `answer` is lifted `async`
/// with a callback, `double` is lifted synchronously. The core
/// functions of the asynchronous pair trap if they are ever entered,
/// which nothing in these tests does.
const CALLBACK_EXPORT: &[u8] = component!(
    r#"
    (component
      (core module $m
        (func (export "answer-callback") (param i32 i32 i32) (result i32) unreachable)
        (func (export "answer") (param i32) (result i32) unreachable)
        (func (export "double") (param i32) (result i32)
          local.get 0 i32.const 2 i32.mul))
      (core instance $i (instantiate $m))
      (func (export "answer") async (param "x" u32) (result u32)
        (canon lift (core func $i "answer") async
          (callback (core func $i "answer-callback"))))
      (func (export "double") (param "x" u32) (result u32)
        (canon lift (core func $i "double"))))
    "#
);

/// An export lifted `async` with no callback: the stackful form.
const STACKFUL_EXPORT: &[u8] = component!(
    r#"
    (component
      (core module $m
        (func (export "answer") (param i32) unreachable))
      (core instance $i (instantiate $m))
      (func (export "answer") async (param "x" u32) (result u32)
        (canon lift (core func $i "answer") async)))
    "#
);

/// A component that imports an `async` function at the root.
const ASYNC_IMPORT: &[u8] = component!(
    r#"
    (component
      (import "answer" (func $answer async (param "x" u32) (result u32))))
    "#
);

/// A component that imports an `async` function inside an interface.
const ASYNC_IMPORT_IN_AN_INTERFACE: &[u8] = component!(
    r#"
    (component
      (import "pdd-tests:host/answers@0.1.0" (instance
        (export "answer" (func async (param "x" u32) (result u32))))))
    "#
);

/// The [`FunctionType`] of the named root-level function export.
fn export_signature(component: &Component, wire_name: &str) -> FunctionType {
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

/// Parse `bytes` with the default engine configuration.
async fn parse(bytes: &[u8]) -> Result<Component, Error> {
    let engine = Engine::new().expect("engine");
    Component::new(&engine, bytes).await
}

/// Parse `bytes` with the stackful asynchronous lift accepted by the
/// validator, so that the polyfill's own refusal is what the test
/// observes rather than the validator's.
async fn parse_with_stackful_lifts(bytes: &[u8]) -> Result<Component, Error> {
    let mut config = EngineConfig::new();
    config.wasm_component_model_async_stackful(true);
    let engine = Engine::with_config(&config).expect("engine");
    Component::new(&engine, bytes).await
}

#[wcmp_macros::test]
async fn it_reports_async_on_a_callback_export_and_not_on_a_synchronous_one() {
    // The `async` effect of the function type reaches the public
    // shape. The two exports below come from one core instance, so
    // the flag is the only thing that tells them apart.
    let component = parse(CALLBACK_EXPORT).await.expect("component parses");

    let asynchronous = export_signature(&component, "answer");
    assert!(
        asynchronous.async_,
        "the callback export's type is `async`: {asynchronous:?}"
    );

    let synchronous = export_signature(&component, "double");
    assert!(
        !synchronous.async_,
        "the synchronous export's type is not `async`: {synchronous:?}"
    );

    // Everything else about the two types is the same, which is why
    // the flag is the fact a host reads.
    assert_eq!(asynchronous.parameters, synchronous.parameters);
    assert_eq!(asynchronous.result, synchronous.result);
}

#[wcmp_macros::test]
async fn it_acquires_a_typed_handle_to_a_callback_export() {
    // The typed conversion compares parameters and result and
    // ignores the `async` flag, so the host writes the same call it
    // writes for a synchronous export of the same shape.
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, CALLBACK_EXPORT)
        .await
        .expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the instantiation extracts the callback");

    let typed = instance
        .get_func("answer")
        .expect("answer export")
        .typed::<(u32,), u32>();
    assert!(
        typed.is_ok(),
        "a typed handle to a callback export is acquired: {:?}",
        typed.err()
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_host_call_into_a_callback_export() {
    // Nothing reads the status word a callback export returns yet,
    // so both entry points refuse the call rather than lifting the
    // word as the export's result.
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, CALLBACK_EXPORT)
        .await
        .expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the instantiation extracts the callback");

    let untyped = instance.get_func("answer").expect("answer export");
    let err = untyped
        .call(&mut store, &[Val::U32(7)])
        .await
        .expect_err("the untyped call is refused");
    assert!(
        matches!(&err, Error::Unsupported { feature } if feature.contains("asynchronous export")),
        "expected Error::Unsupported, got {err:?}"
    );

    let typed = instance
        .get_func("answer")
        .expect("answer export")
        .typed::<(u32,), u32>()
        .expect("typed handle");
    let err = typed
        .call(&mut store, (7,))
        .await
        .expect_err("the typed call is refused");
    assert!(
        matches!(&err, Error::Unsupported { feature } if feature.contains("asynchronous export")),
        "expected Error::Unsupported, got {err:?}"
    );

    // The synchronous export of the same component still runs, so
    // the refusal is the export's and not the instance's.
    let double = instance.get_func("double").expect("double export");
    let result = double
        .call(&mut store, &[Val::U32(21)])
        .await
        .expect("the synchronous call runs");
    assert_eq!(result.as_ref(), &[Val::U32(42)]);
}

#[wcmp_macros::test]
async fn it_refuses_a_stackful_asynchronous_lift() {
    // The stackful form has no callback, so resuming it would mean
    // suspending the export's core function mid-call. The polyfill
    // runs the guest on the one real stack and refuses the lift.
    let err = parse_with_stackful_lifts(STACKFUL_EXPORT)
        .await
        .expect_err("the stackful lift is refused");
    assert!(
        matches!(&err, Error::Unsupported { feature } if feature.contains("stackful")),
        "expected Error::Unsupported, got {err:?}"
    );
}

#[wcmp_macros::test]
async fn it_refuses_an_asynchronous_function_type_on_an_import() {
    // No host function of this design returns its result through a
    // task, so an `async` import is refused where it is listed —
    // whether the import is the function or an interface holding it.
    let err = parse(ASYNC_IMPORT)
        .await
        .expect_err("the asynchronous import is refused");
    assert!(
        matches!(&err, Error::Unsupported { feature } if feature.contains("on imports")),
        "expected Error::Unsupported, got {err:?}"
    );

    let err = parse(ASYNC_IMPORT_IN_AN_INTERFACE)
        .await
        .expect_err("the asynchronous import is refused inside an interface");
    assert!(
        matches!(&err, Error::Unsupported { feature } if feature.contains("on imports")),
        "expected Error::Unsupported, got {err:?}"
    );
}
