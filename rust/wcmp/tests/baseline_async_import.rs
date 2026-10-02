// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

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
//! validation requires the `memory` option on it only where the
//! canonical ABI loads or stores: when a parameter carries a
//! pointer, when the parameter tuple spills, or when the type has a
//! result. A lower of at most four flat parameters with no result is
//! valid without a memory, and the call through it never reads the
//! empty slot. The two axes move separately: an async-typed import
//! may be lowered either way, and only the `async` option itself
//! requires the effect on the type.
//!
//! A component that lowers asynchronously translates, links,
//! instantiates, and calls: through that lower the guest reaches a
//! host `async` function and is answered with the status word. The
//! call path itself is proved in `baseline_async_lower.rs`; what is
//! proved here is that the shape of the lower reaches it.
//!
//! Linking against such an import is held to the *form* of the host's
//! registration. An async-typed import wants a concurrent
//! registration, `func_new_concurrent` or `func_wrap_concurrent`, and
//! a sync-typed import wants a synchronous one, `func_new` or
//! `func_wrap`; either pairing the wrong way round fails to link with
//! the message Wasmtime gives it. A synchronous host function would
//! serve an async-typed import correctly, since it resolves at once,
//! so the rule is Wasmtime's choice rather than the reference's, and
//! the polyfill follows it so that a host's registrations move
//! between the two runtimes unchanged.
//!
//! The rule reads the registration's form and not the signature it
//! declares, because a typed concurrent registration derives a
//! signature whose `async_` is false: the closure's argument tuple
//! and return type say nothing about the effect.

#![cfg(test)]

use wcmp::{
    Accessor, Component, ComponentImport, Engine, Error, ExternType, ExternalName,
    FunctionParameter, FunctionType, HostCall, LinkError, Linker, PrimitiveType, Store, Val,
    ValueType,
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

/// The same lower without the `memory` option. Its one parameter
/// fits a flat slot and it has no result, so nothing in the flattened
/// signature is a pointer and validation asks for no memory.
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

/// An asynchronous lower of a type that has a result, without the
/// `memory` option. Such a lower never returns the result, which
/// travels through a return-area pointer the guest passes, so the
/// canonical ABI stores through the memory and validation requires
/// the option.
const ASYNCHRONOUS_LOWER_OF_A_RESULT_WITHOUT_MEMORY: &[u8] = component!(
    r#"
    (component
      (import "answer" (func $answer async (param "x" u32) (result u32)))
      (core func $lowered (canon lower (func $answer) async))
      (core module $m
        (import "" "answer" (func $answer (param i32 i32) (result i32)))
        (func (export "run") (result i32)
          (call $answer (i32.const 7) (i32.const 0))))
      (core instance $i (instantiate $m
        (with "" (instance (export "answer" (func $lowered))))))
      (func (export "run") (result u32) (canon lift (core func $i "run"))))
    "#
);

/// An asynchronous lower whose five flat parameters exceed the four
/// slots such a lower has, without the `memory` option. The tuple
/// spills through one pointer, so the canonical ABI loads it out of
/// the memory and validation requires the option.
const ASYNCHRONOUS_LOWER_OF_FIVE_PARAMETERS_WITHOUT_MEMORY: &[u8] = component!(
    r#"
    (component
      (import "answer" (func $answer async
        (param "a" u32) (param "b" u32) (param "c" u32)
        (param "d" u32) (param "e" u32)))
      (core func $lowered (canon lower (func $answer) async))
      (core module $m
        (import "" "answer" (func $answer (param i32) (result i32)))
        (func (export "run") (result i32)
          (call $answer (i32.const 0))))
      (core instance $i (instantiate $m
        (with "" (instance (export "answer" (func $lowered))))))
      (func (export "run") (result u32) (canon lift (core func $i "run"))))
    "#
);

/// A component that lowers an async-typed import *without* the
/// `async` option, which the two axes allow: the lowered core
/// function keeps the synchronous flattening, one flat parameter and
/// one flat result, and the guest reaches the host on its own stack.
const SYNCHRONOUS_LOWER_OF_AN_ASYNC_IMPORT: &[u8] = component!(
    r#"
    (component
      (import "answer" (func $answer async (param "x" u32) (result u32)))
      (core func $lowered (canon lower (func $answer)))
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
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
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
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, ASYNCHRONOUS_LOWER)
        .await
        .expect("component parses");

    // The import's type is `async func`, so the link rule wants a
    // concurrent registration for it.
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap_concurrent(
            "answer",
            |_accessor: &Accessor<()>, (x,): (u32,)| async move { Ok(x * 2) },
        )
        .expect("the registration");

    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the component instantiates");

    // The guest's `run` hands the status word straight back. The
    // registration's future resolves on its first poll, so the call
    // returned before the lower did and the word is the returned
    // state alone, with no subtask index above it.
    let status = instance
        .get_func("run")
        .expect("the component exports `run`")
        .call(&mut store, &[])
        .await
        .expect("the asynchronous host call runs");
    assert_eq!(
        status.first(),
        Some(&Val::U32(2)),
        "the guest saw the returned state with no index"
    );
}

#[wcmp_macros::test]
async fn it_translates_an_asynchronous_lower_without_memory_when_nothing_spills() {
    // The reference requires the `memory` option where the canonical
    // ABI loads or stores. This lower's one parameter travels in a
    // flat slot and its type has no result, so the flattened
    // signature holds no pointer at all and the lower is valid with
    // no memory named.
    parse(ASYNCHRONOUS_LOWER_WITHOUT_MEMORY)
        .await
        .expect("the lower with nothing to spill needs no memory");
}

#[wcmp_macros::test]
async fn it_calls_through_an_asynchronous_lower_without_memory() {
    // The trampoline of such a lower holds an empty memory slot, and
    // the call path never reads it: the parameters are lifted from
    // the flat slots, and the host's future produces no value to
    // lower. What the guest is answered with is the status word.
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, ASYNCHRONOUS_LOWER_WITHOUT_MEMORY)
        .await
        .expect("component parses");

    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap_concurrent(
            "answer",
            |_accessor: &Accessor<()>, (_x,): (u32,)| async move { Ok(()) },
        )
        .expect("the registration");

    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the component instantiates");

    // The registration's future resolves on its first poll, so the
    // call returned before the lower did: the word is the returned
    // state alone, with no subtask index above it.
    let status = instance
        .get_func("run")
        .expect("the component exports `run`")
        .call(&mut store, &[])
        .await
        .expect("the asynchronous host call runs");
    assert_eq!(
        status.first(),
        Some(&Val::U32(2)),
        "the guest saw the returned state with no index"
    );
}

#[wcmp_macros::test]
async fn it_refuses_an_asynchronous_lower_without_memory_when_the_type_has_a_result() {
    // An asynchronous lower never returns the result: it travels
    // through a return-area pointer, which is a store through the
    // memory, so validation requires the option.
    let err = parse(ASYNCHRONOUS_LOWER_OF_A_RESULT_WITHOUT_MEMORY)
        .await
        .expect_err("the lower of a result without a memory is refused");
    assert_memory_option_required(&err);
}

#[wcmp_macros::test]
async fn it_refuses_an_asynchronous_lower_without_memory_when_the_parameters_spill() {
    // Five flat parameters exceed the four slots an asynchronous
    // lower has, so the tuple spills and the lower loads it out of
    // the memory, which validation therefore requires.
    let err = parse(ASYNCHRONOUS_LOWER_OF_FIVE_PARAMETERS_WITHOUT_MEMORY)
        .await
        .expect_err("the lower of a spilled tuple without a memory is refused");
    assert_memory_option_required(&err);
}

/// Assert that `err` is the refusal of an asynchronous lower that
/// needs the `memory` option and does not declare one.
///
/// The refusal is validation's, and the message is the one
/// `wasmparser` gives it: `ComponentFuncType::lower` in
/// `src/validator/component_types.rs` at `0.258.0` requires the
/// option for a parameter that transitively contains a pointer, for
/// a parameter tuple that spills, and for a result. The polyfill
/// adds no check of its own, so what a host reads is that text with
/// the offset of the `canon` definition.
fn assert_memory_option_required(err: &Error) {
    assert!(
        matches!(
            err,
            Error::InvalidComponentBinary { message, .. }
                if message.starts_with("canonical option `memory` is required")
        ),
        "expected Error::InvalidComponentBinary naming the memory option, got {err:?}"
    );
}

/// Wasmtime 49's message for an async-typed import satisfied by a
/// synchronous registration, verbatim from `typecheck_async` in
/// `crates/wasmtime/src/runtime/component/func/host.rs` at
/// `v49.0.0-rc.1`. Pinned here so the polyfill's rendering stays the
/// text a host reads from either runtime.
const SYNC_REGISTRATION_MESSAGE: &str = "type mismatch with async: this import is declared \
     `async func` in WIT, but was satisfied with a sync-style host function \
     (`func_new`/`func_wrap`, or `func_new_async`/`func_wrap_async` — despite the name, these \
     implement a *sync*-WIT-typed function via blocking host code, not an `async func` import); \
     use `func_new_concurrent`/`func_wrap_concurrent` instead";

/// Wasmtime 49's message for the other mismatch, a sync-typed import
/// satisfied by a concurrent registration, from the same function.
const CONCURRENT_REGISTRATION_MESSAGE: &str = "type mismatch with async: this import's WIT type \
     is a plain (non-`async`) function, but was satisfied with \
     `func_new_concurrent`/`func_wrap_concurrent`, which is only for `async func`-typed imports; \
     use `func_new`/`func_wrap` (or `func_new_async`/`func_wrap_async` for blocking host code) \
     instead";

/// The declared type of the two `answer`/`double` imports, with the
/// `async` effect the caller names. The untyped registration entries
/// take the type explicitly, so a test can declare either flag and
/// see that the rule does not read it.
fn declared(async_: bool) -> FunctionType {
    FunctionType {
        parameters: vec![FunctionParameter {
            name: "x".to_owned(),
            ty: ValueType::Primitive(PrimitiveType::U32),
        }],
        result: Some(ValueType::Primitive(PrimitiveType::U32)),
        async_,
    }
}

/// The [`LinkError`] behind a link failure, or a panic naming what the
/// failure turned out to be.
fn link_error(err: Error) -> LinkError {
    match err {
        Error::Link(inner) => *inner,
        other => panic!("expected a link error, got {other:?}"),
    }
}

/// Link `bytes` against `linker` and give back the failure.
async fn link_failure<T: Send + 'static>(
    engine: &Engine,
    linker: &Linker<T>,
    data: T,
    bytes: &[u8],
) -> Error {
    let component = Component::new(engine, bytes)
        .await
        .expect("component parses");
    let mut store: Store<T> = Store::new(engine, data).expect("store");
    match linker.instantiate(&mut store, &component).await {
        Ok(_) => panic!("the mismatched registration kind must not link"),
        Err(err) => err,
    }
}

#[wcmp_macros::test]
async fn it_refuses_an_async_typed_import_satisfied_by_a_synchronous_registration() {
    // `func_wrap` is a synchronous registration, and Wasmtime refuses
    // it for an `async func` import however well its signature fits.
    // The polyfill refuses it with the same text.
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap("answer", |_call: HostCall<'_, ()>, (x,): (u32,)| Ok(x * 2))
        .expect("the registration");
    linker
        .root()
        .func_wrap("double", |_call: HostCall<'_, ()>, (x,): (u32,)| Ok(x * 2))
        .expect("the registration");

    let err = link_failure(&engine, &linker, (), ASYNC_IMPORT).await;
    let cause = link_error(err);
    assert!(
        matches!(
            &cause,
            LinkError::SynchronousRegistrationForAsyncImport { import, item }
                if *import == ExternalName::Plain("answer".to_owned()) && item.is_none()
        ),
        "expected the synchronous-registration cause naming `answer`, got {cause:?}"
    );
    assert_eq!(
        cause.to_string(),
        format!("import `answer`: {SYNC_REGISTRATION_MESSAGE}")
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_sync_typed_import_satisfied_by_a_concurrent_registration() {
    // The other half of the rule. `answer` is satisfied the way it
    // wants, so the failure the component reaches is `double`'s:
    // `func_wrap_concurrent` is only for an `async func` import.
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap_concurrent(
            "answer",
            |_accessor: &Accessor<()>, (x,): (u32,)| async move { Ok(x * 2) },
        )
        .expect("the registration");
    linker
        .root()
        .func_wrap_concurrent(
            "double",
            |_accessor: &Accessor<()>, (x,): (u32,)| async move { Ok(x * 2) },
        )
        .expect("the registration");

    let err = link_failure(&engine, &linker, (), ASYNC_IMPORT).await;
    let cause = link_error(err);
    assert!(
        matches!(
            &cause,
            LinkError::ConcurrentRegistrationForSyncImport { import, item }
                if *import == ExternalName::Plain("double".to_owned()) && item.is_none()
        ),
        "expected the concurrent-registration cause naming `double`, got {cause:?}"
    );
    assert_eq!(
        cause.to_string(),
        format!("import `double`: {CONCURRENT_REGISTRATION_MESSAGE}")
    );
}

#[wcmp_macros::test]
async fn it_names_the_item_of_an_interface_import_the_rule_refuses() {
    // The rule runs at every position a function item sits in, and
    // the cause carries the item's name inside an interface import.
    // The message names it too, in the clause Wasmtime's error chain
    // puts between the import and the reason: an interface import
    // with several function items says which item failed.
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .instance(&"pdd-tests:host/answers@0.1.0".parse().expect("identifier"))
        .func_wrap("answer", |_call: HostCall<'_, ()>, (x,): (u32,)| Ok(x * 2))
        .expect("the registration");

    let err = link_failure(&engine, &linker, (), ASYNC_IMPORT_IN_AN_INTERFACE).await;
    let cause = link_error(err);
    match &cause {
        LinkError::SynchronousRegistrationForAsyncImport { import, item } => {
            assert_eq!(import.to_string(), "pdd-tests:host/answers@0.1.0");
            assert_eq!(item.as_deref(), Some("answer"));
        }
        other => panic!("expected the synchronous-registration cause, got {other:?}"),
    }
    assert_eq!(
        cause.to_string(),
        format!(
            "import `pdd-tests:host/answers@0.1.0`: instance export `answer` has the wrong type: \
             {SYNC_REGISTRATION_MESSAGE}"
        )
    );
}

#[wcmp_macros::test]
async fn it_links_both_concurrent_entries_for_an_async_typed_import() {
    // Both concurrent entries register and link for the async-typed
    // import, beside a synchronous entry for the sync-typed one. The
    // typed entry derives a signature whose `async_` is false, so a
    // link that reads the signature rather than the form would refuse
    // the first of the two.
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, ASYNC_IMPORT)
        .await
        .expect("component parses");

    let mut typed: Linker<()> = Linker::new(&engine);
    typed
        .root()
        .func_wrap_concurrent(
            "answer",
            |_accessor: &Accessor<()>, (x,): (u32,)| async move { Ok(x * 2) },
        )
        .expect("the registration");
    typed
        .root()
        .func_wrap("double", |_call: HostCall<'_, ()>, (x,): (u32,)| Ok(x * 2))
        .expect("the registration");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    typed
        .instantiate(&mut store, &component)
        .await
        .expect("`func_wrap_concurrent` links for an async-typed import");

    let mut untyped: Linker<()> = Linker::new(&engine);
    untyped
        .root()
        .func_new_concurrent(
            "answer",
            declared(true),
            |_accessor: &Accessor<()>, args: Vec<Val>| async move { Ok(args) },
        )
        .expect("the registration");
    untyped
        .root()
        .func_new("double", declared(false), |_call, _args, _results| Ok(()))
        .expect("the registration");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    untyped
        .instantiate(&mut store, &component)
        .await
        .expect("`func_new_concurrent` links for an async-typed import");
}

#[wcmp_macros::test]
async fn it_reads_the_registration_form_rather_than_its_declared_async_flag() {
    // The untyped entries take the declared type whole, so a host can
    // name the `async` effect on either form. The rule reads the form
    // all the same: a synchronous registration that declares the
    // effect still cannot serve the async-typed import, and a
    // concurrent registration that omits it still cannot serve the
    // sync-typed one.
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");

    let mut claims_async: Linker<()> = Linker::new(&engine);
    claims_async
        .root()
        .func_new("answer", declared(true), |_call, _args, _results| Ok(()))
        .expect("the registration");
    claims_async
        .root()
        .func_new("double", declared(false), |_call, _args, _results| Ok(()))
        .expect("the registration");
    let cause = link_error(link_failure(&engine, &claims_async, (), ASYNC_IMPORT).await);
    assert!(
        matches!(
            &cause,
            LinkError::SynchronousRegistrationForAsyncImport { import, .. }
                if *import == ExternalName::Plain("answer".to_owned())
        ),
        "a declared `async` effect does not make a synchronous registration concurrent: {cause:?}"
    );

    let mut claims_sync: Linker<()> = Linker::new(&engine);
    claims_sync
        .root()
        .func_new_concurrent(
            "answer",
            declared(true),
            |_accessor: &Accessor<()>, args: Vec<Val>| async move { Ok(args) },
        )
        .expect("the registration");
    claims_sync
        .root()
        .func_new_concurrent(
            "double",
            declared(false),
            |_accessor: &Accessor<()>, args: Vec<Val>| async move { Ok(args) },
        )
        .expect("the registration");
    let cause = link_error(link_failure(&engine, &claims_sync, (), ASYNC_IMPORT).await);
    assert!(
        matches!(
            &cause,
            LinkError::ConcurrentRegistrationForSyncImport { import, .. }
                if *import == ExternalName::Plain("double".to_owned())
        ),
        "an omitted `async` effect does not make a concurrent registration synchronous: {cause:?}"
    );
}

#[wcmp_macros::test]
async fn it_calls_a_concurrent_registration_through_a_synchronous_lower() {
    // The rule holds a concurrent registration to an async-typed
    // import, and the two axes still move separately: this component
    // lowers the async-typed import without the `async` option, so the
    // guest reaches the host synchronously and expects the result when
    // the call returns. A future ready on its first poll gives it
    // there and then; the block that a future which is not yet ready
    // needs is proved in `baseline_sync_lower.rs`.
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, SYNCHRONOUS_LOWER_OF_AN_ASYNC_IMPORT)
        .await
        .expect("component parses");

    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap_concurrent(
            "answer",
            |_accessor: &Accessor<()>, (x,): (u32,)| async move { Ok(x * 2) },
        )
        .expect("the registration");

    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the component instantiates");

    let result = instance
        .get_func("run")
        .expect("the component exports `run`")
        .call(&mut store, &[])
        .await
        .expect("the synchronous call of a host `async` function returns");
    assert_eq!(
        result.first(),
        Some(&Val::U32(14)),
        "the host's result crossed as the synchronous lower returned"
    );
}

/// A component that imports an `async` function, and a synchronous
/// one of the same shape, under *interface* names rather than plain
/// ones. Neither import is an instance, so both resolve through the
/// root namespace under the whole name — and the rule that holds a
/// registration's form to the import's `async` effect reads them
/// there as it reads a plain-named import.
const ASYNC_IMPORT_UNDER_AN_INTERFACE_NAME: &[u8] = component!(
    r#"
    (component
      (import "pdd-tests:host/answers@0.1.0"
        (func $answer async (param "x" u32) (result u32)))
      (import "pdd-tests:host/doubles@0.1.0"
        (func $double (param "x" u32) (result u32))))
    "#
);

#[wcmp_macros::test]
async fn it_holds_the_registration_form_of_an_import_under_an_interface_name() {
    // A function import that carries an interface name is a function
    // import: it reaches the same registration-kind check as one
    // named plainly, and the cause names the import rather than an
    // item inside it.
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");

    let mut synchronous: Linker<()> = Linker::new(&engine);
    synchronous
        .root()
        .func_wrap(
            "pdd-tests:host/answers@0.1.0",
            |_call: HostCall<'_, ()>, (x,): (u32,)| Ok(x * 2),
        )
        .expect("the registration");
    synchronous
        .root()
        .func_wrap(
            "pdd-tests:host/doubles@0.1.0",
            |_call: HostCall<'_, ()>, (x,): (u32,)| Ok(x * 2),
        )
        .expect("the registration");
    let cause = link_error(
        link_failure(
            &engine,
            &synchronous,
            (),
            ASYNC_IMPORT_UNDER_AN_INTERFACE_NAME,
        )
        .await,
    );
    match &cause {
        LinkError::SynchronousRegistrationForAsyncImport { import, item } => {
            assert!(
                matches!(import, ExternalName::Interface(_)),
                "the cause carries the import's interface name: {import:?}"
            );
            assert_eq!(import.to_string(), "pdd-tests:host/answers@0.1.0");
            assert_eq!(item.as_deref(), None);
        }
        other => panic!("expected the synchronous-registration cause, got {other:?}"),
    }

    // The other half of the rule, on the sync-typed import beside it.
    let mut concurrent: Linker<()> = Linker::new(&engine);
    concurrent
        .root()
        .func_new_concurrent(
            "pdd-tests:host/answers@0.1.0",
            declared(true),
            |_accessor: &Accessor<()>, args: Vec<Val>| async move { Ok(args) },
        )
        .expect("the registration");
    concurrent
        .root()
        .func_new_concurrent(
            "pdd-tests:host/doubles@0.1.0",
            declared(false),
            |_accessor: &Accessor<()>, args: Vec<Val>| async move { Ok(args) },
        )
        .expect("the registration");
    let cause = link_error(
        link_failure(
            &engine,
            &concurrent,
            (),
            ASYNC_IMPORT_UNDER_AN_INTERFACE_NAME,
        )
        .await,
    );
    match &cause {
        LinkError::ConcurrentRegistrationForSyncImport { import, item } => {
            assert_eq!(import.to_string(), "pdd-tests:host/doubles@0.1.0");
            assert_eq!(item.as_deref(), None);
        }
        other => panic!("expected the concurrent-registration cause, got {other:?}"),
    }
}

#[wcmp_macros::test]
async fn it_links_a_concurrent_registration_for_an_async_import_under_an_interface_name() {
    // The pairing the rule wants links: the async-typed import takes
    // the concurrent entry and the sync-typed one takes the
    // synchronous entry, both registered on the root view under the
    // interface names the component writes.
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, ASYNC_IMPORT_UNDER_AN_INTERFACE_NAME)
        .await
        .expect("component parses");

    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap_concurrent(
            "pdd-tests:host/answers@0.1.0",
            |_accessor: &Accessor<()>, (x,): (u32,)| async move { Ok(x * 2) },
        )
        .expect("the registration");
    linker
        .root()
        .func_wrap(
            "pdd-tests:host/doubles@0.1.0",
            |_call: HostCall<'_, ()>, (x,): (u32,)| Ok(x * 2),
        )
        .expect("the registration");

    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    linker
        .instantiate(&mut store, &component)
        .await
        .expect("the root registrations under the interface names satisfy both imports");
}

#[path = "support/backend.rs"]
mod test_backend;
