//! Baseline tests for the async-typed import and for the core
//! signature an asynchronous lower presents to the guest.
//!
//! The type projection accepts an async-typed import. A host reads
//! `async_` on an imported function's type exactly as it reads it on
//! an export: the flag is the callee's half of the call, and it says
//! nothing about how a caller reaches the function.
//!
//! How a caller reaches it is the other axis, the `canon lower`. A
//! lower without the `async` option keeps the core signature of the
//! synchronous canonical ABI. A lower with it presents a different
//! one — at most four flat parameters, the result always through a
//! return-area pointer, and one `i32` result, the status word — and
//! validation requires the `memory` option on it. The two axes move
//! separately: an async-typed import may be lowered either way, and
//! only the `async` option itself requires the effect on the type.
//!
//! A component that lowers asynchronously translates, links, and
//! instantiates. Only a guest that makes such a call meets the
//! refusal, because the call path behind it is not built yet.

#![cfg(test)]

use wasm_component_model_polyfill::{
    Component, ComponentImport, Engine, Error, ExternType, ExternalName, FunctionType, HostCall,
    Linker, Store,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// A component that imports an `async` function at the root beside a
/// synchronous one of the same shape.
const ASYNC_IMPORT: &[u8] = component!(
    r#"
    (component
      (import "answer" (func $answer async (param "x" u32) (result u32)))
      (import "double" (func $double (param "x" u32) (result u32))))
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

/// A component that lowers an async-typed import with the `async`
/// option. The lowered core function takes the one flat parameter
/// and the return-area pointer, and returns the status word, which
/// is the signature `flatten_functype` gives an asynchronous lower.
const ASYNCHRONOUS_LOWER: &[u8] = component!(
    r#"
    (component
      (import "answer" (func $answer async (param "x" u32) (result u32)))
      (core module $libc
        (memory (export "memory") 1))
      (core instance $libc (instantiate $libc))
      (core func $lowered
        (canon lower (func $answer) async (memory (core memory $libc "memory"))))
      (core module $m
        (import "" "answer" (func $answer (param i32 i32) (result i32)))
        (func (export "run") (result i32)
          (call $answer (i32.const 7) (i32.const 0))))
      (core instance $i (instantiate $m
        (with "" (instance (export "answer" (func $lowered))))))
      (func (export "run") (result u32) (canon lift (core func $i "run"))))
    "#
);

/// The same lower without the `memory` option. Its parameter fits a
/// flat slot and it has no result, so nothing in the signature needs
/// a pointer — and validation requires the memory all the same.
const ASYNCHRONOUS_LOWER_WITHOUT_MEMORY: &[u8] = component!(
    r#"
    (component
      (import "answer" (func $answer async (param "x" u32)))
      (core func $lowered (canon lower (func $answer) async))
      (core module $m
        (import "" "answer" (func $answer (param i32) (result i32)))
        (func (export "run") (result i32)
          (call $answer (i32.const 7))))
      (core instance $i (instantiate $m
        (with "" (instance (export "answer" (func $lowered))))))
      (func (export "run") (result u32) (canon lift (core func $i "run"))))
    "#
);

/// Parse `bytes` with the default engine configuration.
async fn parse(bytes: &[u8]) -> Result<Component, Error> {
    let engine = Engine::new().expect("engine");
    Component::new(&engine, bytes).await
}

/// The [`FunctionType`] of the named root-level function import.
fn import_signature(component: &Component, wire_name: &str) -> FunctionType {
    match &import(component, wire_name).ty {
        ExternType::Function(ty) => ty.clone(),
        other => panic!("import `{wire_name}` is not a function: {other:?}"),
    }
}

/// The named root-level import.
fn import<'a>(component: &'a Component, wire_name: &str) -> &'a ComponentImport {
    component
        .imports
        .iter()
        .find(|import| match &import.name {
            ExternalName::Plain(name) => name == wire_name,
            ExternalName::Interface(id) => id.to_string() == wire_name,
        })
        .unwrap_or_else(|| panic!("import `{wire_name}` not found"))
}

/// Every message in an error's source chain, joined so that a cause
/// the substrate wrapped can be matched wherever it put it.
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
    out
}

#[wcmp_macros::test]
async fn it_reports_async_on_an_imported_function_type() {
    // The `async` effect of the import's type reaches the public
    // shape, and the translation that used to refuse the import now
    // carries it through. The two imports below differ in the flag
    // alone, which is what makes it the fact a host reads.
    let component = parse(ASYNC_IMPORT).await.expect("component parses");

    let asynchronous = import_signature(&component, "answer");
    assert!(
        asynchronous.async_,
        "the async-typed import's type is `async`: {asynchronous:?}"
    );

    let synchronous = import_signature(&component, "double");
    assert!(
        !synchronous.async_,
        "the sync-typed import's type is not `async`: {synchronous:?}"
    );

    assert_eq!(asynchronous.parameters, synchronous.parameters);
    assert_eq!(asynchronous.result, synchronous.result);
}

#[wcmp_macros::test]
async fn it_reports_async_on_an_imported_function_inside_an_interface() {
    // An import may be an instance that holds the function at any
    // depth, and the flag reaches the item's type there too.
    let component = parse(ASYNC_IMPORT_IN_AN_INTERFACE)
        .await
        .expect("component parses");

    let instance = match &import(&component, "pdd-tests:host/answers@0.1.0").ty {
        ExternType::Instance(instance) => instance.clone(),
        other => panic!("the import is not an instance: {other:?}"),
    };
    let item = instance
        .items
        .iter()
        .find(|item| item.name == "answer")
        .expect("the interface exports `answer`");
    match &item.ty {
        ExternType::Function(ty) => assert!(ty.async_, "the item's type is `async`: {ty:?}"),
        other => panic!("`answer` is not a function: {other:?}"),
    }
}

#[wcmp_macros::test]
async fn it_instantiates_a_component_whose_import_is_lowered_asynchronously() {
    // The lower's core signature is the one the guest module
    // imports, so the component only instantiates if the trampoline
    // was built with the asynchronous flattening.
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, ASYNCHRONOUS_LOWER)
        .await
        .expect("component parses");

    let mut linker: Linker<()> = Linker::new(&engine);
    linker.root().func_wrap(
        "answer",
        |_call: HostCall<'_, ()>, (x,): (u32,)| -> wasm_component_model_polyfill::Result<u32> {
            Ok(x * 2)
        },
    );

    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the component instantiates");

    // The call path behind an asynchronous lower is not built, so
    // the guest's call into it fails where it is made.
    let err = instance
        .get_func("run")
        .expect("the component exports `run`")
        .call(&mut store, &[])
        .await
        .expect_err("the asynchronous host call is refused");
    assert!(
        chain(&err).contains("asynchronous host calls"),
        "expected the asynchronous host call to be named, got {err:?}"
    );
}

#[wcmp_macros::test]
async fn it_refuses_an_asynchronous_lower_without_the_memory_option() {
    // Validation requires the `memory` option on an asynchronous
    // lower whatever the lowered type is, so a component that
    // declares one without a memory is invalid rather than
    // unsupported.
    let err = parse(ASYNCHRONOUS_LOWER_WITHOUT_MEMORY)
        .await
        .expect_err("the lower without a memory is refused");
    assert!(
        matches!(&err, Error::InvalidComponentBinary { message, .. } if message.contains("memory")),
        "expected Error::InvalidComponentBinary naming the memory option, got {err:?}"
    );
}
