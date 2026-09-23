//! Baseline tests for the `stream<T>` and `future<T>` value types.
//!
//! The type projection accepts both in a function type, on an import
//! and on an export, and names each with its payload type. A value of
//! either type is the readable end of a stream or a future: one `i32`,
//! the index of the end in the handle table of the instance that holds
//! it. Such a function type is ordinary in every other respect, so a
//! synchronous export, a synchronous host function, an `async` export,
//! and an `async` import can each carry one.
//!
//! A readable end cannot cross a boundary yet. A component whose
//! function type carries a stream or a future translates, links, and
//! instantiates, and a call that lifts or lowers such a value fails
//! with `Error::Unsupported` at the call.

#![cfg(test)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use wasm_component_model_polyfill::{
    Component, Engine, Error, ExternType, ExternalName, FunctionParameter, FunctionType,
    FutureType, Instance, Linker, ListType, PrimitiveType, Store, StreamType, Val, ValueType,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// A component that carries a stream or a future on a parameter and
/// on a result of every kind of function: a synchronous import and
/// export, and an `async` import and export. The payloads cover a
/// number, a string, a list, and no payload at all.
const STREAMS_AND_FUTURES: &[u8] = component!(
    r#"
    (component
      (import "pull" (func (param "s" (stream u8)) (result (future string))))
      (import "pull-async" (func async (param "f" (future)) (result (stream (list u8)))))
      (core module $m
        (func (export "take") (param i32) (result i32) local.get 0)
        (func (export "take-async") (param i32) (result i32) i32.const 0)
        (func (export "callback") (param i32 i32 i32) (result i32) i32.const 0))
      (core instance $i (instantiate $m))
      (func (export "take") (param "s" (stream u8)) (result (future string))
        (canon lift (core func $i "take")))
      (func (export "take-async") async (param "f" (future)) (result (stream))
        (canon lift (core func $i "take-async") async (callback (core func $i "callback")))))
    "#
);

/// A component whose one export returns a future, and whose guest
/// hands back an index the host would have to lift.
const RETURNS_A_FUTURE: &[u8] = component!(
    r#"
    (component
      (core module $m
        (func (export "make") (result i32) i32.const 1))
      (core instance $i (instantiate $m))
      (func (export "make") (result (future u32))
        (canon lift (core func $i "make"))))
    "#
);

/// A component whose export calls a synchronous host function that
/// takes a stream, so the host would have to lift the guest's end.
const PASSES_A_STREAM_TO_THE_HOST: &[u8] = component!(
    r#"
    (component
      (import "pull" (func $pull (param "s" (stream u8))))
      (core func $pull (canon lower (func $pull)))
      (core module $m
        (import "" "pull" (func $pull (param i32)))
        (func (export "run") (call $pull (i32.const 1))))
      (core instance $i (instantiate $m
        (with "" (instance (export "pull" (func $pull))))))
      (func (export "run") (canon lift (core func $i "run"))))
    "#
);

/// The type of `stream<u8>`.
fn stream_of_bytes() -> ValueType {
    ValueType::Stream(StreamType::new(Some(ValueType::Primitive(
        PrimitiveType::U8,
    ))))
}

/// The type of `future<string>`.
fn future_of_a_string() -> ValueType {
    ValueType::Future(FutureType::new(Some(ValueType::Primitive(
        PrimitiveType::String,
    ))))
}

/// The function type of the named root import or export of
/// [`STREAMS_AND_FUTURES`].
async fn function_type(wire_name: &str, import: bool) -> FunctionType {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, STREAMS_AND_FUTURES)
        .await
        .expect("a component whose function types carry streams and futures translates");
    let is_named = |name: &ExternalName| matches!(name, ExternalName::Plain(n) if n == wire_name);
    let ty = if import {
        component
            .imports
            .iter()
            .find(|item| is_named(&item.name))
            .map(|item| item.ty.clone())
    } else {
        component
            .exports
            .iter()
            .find(|item| is_named(&item.name))
            .map(|item| item.ty.clone())
    };
    match ty {
        Some(ExternType::Function(ty)) => ty,
        other => panic!("`{wire_name}` is not a function: {other:?}"),
    }
}

/// The feature an `Error::Unsupported` names, or a panic naming what
/// the error turned out to be.
fn unsupported_feature(err: Error) -> String {
    match err {
        Error::Unsupported { feature } => feature,
        other => panic!("expected an unsupported feature, got {other:?}"),
    }
}

/// Instantiate `bytes` against `linker` in a fresh store.
async fn instantiate(engine: &Engine, linker: &Linker<()>, bytes: &[u8]) -> (Store<()>, Instance) {
    let component = Component::new(engine, bytes)
        .await
        .expect("component parses");
    let mut store: Store<()> = Store::new(engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the component instantiates");
    (store, instance)
}

#[wcmp_macros::test]
async fn it_projects_a_stream_and_a_future_on_a_synchronous_export() {
    let ty = function_type("take", false).await;
    assert!(!ty.async_, "`take` is synchronous: {ty:?}");
    assert_eq!(ty.parameters.len(), 1);
    assert_eq!(ty.parameters[0].ty, stream_of_bytes());
    assert_eq!(ty.result, Some(future_of_a_string()));
}

#[wcmp_macros::test]
async fn it_projects_a_stream_and_a_future_on_a_synchronous_import() {
    let ty = function_type("pull", true).await;
    assert!(!ty.async_, "`pull` is synchronous: {ty:?}");
    assert_eq!(ty.parameters.len(), 1);
    assert_eq!(ty.parameters[0].ty, stream_of_bytes());
    assert_eq!(ty.result, Some(future_of_a_string()));
}

#[wcmp_macros::test]
async fn it_projects_a_stream_and_a_future_on_an_async_export() {
    // Neither end carries a payload here: the projection reads the
    // absence as it reads a payload.
    let ty = function_type("take-async", false).await;
    assert!(ty.async_, "`take-async` is `async`: {ty:?}");
    assert_eq!(ty.parameters.len(), 1);
    let ValueType::Future(future) = &ty.parameters[0].ty else {
        panic!("expected a future parameter, got {:?}", ty.parameters[0].ty);
    };
    assert_eq!(future.payload(), None);
    let Some(ValueType::Stream(stream)) = &ty.result else {
        panic!("expected a stream result, got {:?}", ty.result);
    };
    assert_eq!(stream.payload(), None);
}

#[wcmp_macros::test]
async fn it_projects_a_stream_and_a_future_on_an_async_import() {
    let ty = function_type("pull-async", true).await;
    assert!(ty.async_, "`pull-async` is `async`: {ty:?}");
    assert_eq!(ty.parameters.len(), 1);
    assert_eq!(
        ty.parameters[0].ty,
        ValueType::Future(FutureType::new(None))
    );
    let Some(ValueType::Stream(stream)) = &ty.result else {
        panic!("expected a stream result, got {:?}", ty.result);
    };
    assert_eq!(
        stream.payload(),
        Some(&ValueType::List(ListType::new(ValueType::Primitive(
            PrimitiveType::U8
        )))),
        "the payload projects through its own compound type"
    );
}

#[wcmp_macros::test]
async fn it_distinguishes_a_stream_from_a_future_of_the_same_payload() {
    let payload = || Some(ValueType::Primitive(PrimitiveType::U8));
    assert_ne!(
        ValueType::Stream(StreamType::new(payload())),
        ValueType::Future(FutureType::new(payload())),
    );
    assert_ne!(
        ValueType::Stream(StreamType::new(payload())),
        ValueType::Stream(StreamType::new(None)),
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_stream_of_char_at_validation() {
    const STREAM_OF_CHAR: &[u8] = component!(
        r#"
        (component
          (type (stream char)))
        "#
    );
    let engine = Engine::new().expect("engine");
    let err = match Component::new(&engine, STREAM_OF_CHAR).await {
        Ok(_) => panic!("`stream<char>` must not translate"),
        Err(err) => err,
    };
    assert!(
        err.to_string().contains("`stream<char>` is not valid"),
        "validation names the refused payload, got {err}"
    );
}

/// A linker that satisfies both imports of [`STREAMS_AND_FUTURES`]:
/// a synchronous host function and a host `async` function, each
/// declaring the stream and future types of its import. `ran` is set
/// if either body runs.
fn linker_for_every_kind(engine: &Engine, ran: Arc<AtomicBool>) -> Linker<()> {
    let mut linker: Linker<()> = Linker::new(engine);
    let mut root = linker.root();
    let sync_ran = ran.clone();
    root.func_new(
        "pull",
        FunctionType {
            parameters: vec![FunctionParameter {
                name: "s".to_owned(),
                ty: stream_of_bytes(),
            }],
            result: Some(future_of_a_string()),
            async_: false,
        },
        move |_call, _args, _results| {
            sync_ran.store(true, Ordering::SeqCst);
            Ok(())
        },
    )
    .expect("a synchronous host function can carry a stream and a future");
    root.func_new_concurrent(
        "pull-async",
        FunctionType {
            parameters: vec![FunctionParameter {
                name: "f".to_owned(),
                ty: ValueType::Future(FutureType::new(None)),
            }],
            result: Some(ValueType::Stream(StreamType::new(Some(ValueType::List(
                ListType::new(ValueType::Primitive(PrimitiveType::U8)),
            ))))),
            async_: true,
        },
        move |_accessor, _args| {
            ran.store(true, Ordering::SeqCst);
            async { Ok(Vec::new()) }
        },
    )
    .expect("a host `async` function can carry a stream and a future");
    linker
}

#[wcmp_macros::test]
async fn it_links_host_functions_that_carry_streams_and_futures() {
    // The host's declared types match the projection of each import,
    // so both link and the component instantiates.
    let engine = Engine::new().expect("engine");
    let ran = Arc::new(AtomicBool::new(false));
    let linker = linker_for_every_kind(&engine, ran.clone());
    instantiate(&engine, &linker, STREAMS_AND_FUTURES).await;
    assert!(!ran.load(Ordering::SeqCst), "no host body ran");
}

#[wcmp_macros::test]
async fn it_refuses_to_lower_a_stream_into_a_synchronous_export_at_the_call() {
    // The host has no readable end to pass, so whatever value it
    // hands over, the lower that reaches the stream fails before the
    // guest runs.
    let engine = Engine::new().expect("engine");
    let linker = linker_for_every_kind(&engine, Arc::new(AtomicBool::new(false)));
    let (mut store, instance) = instantiate(&engine, &linker, STREAMS_AND_FUTURES).await;

    let err = instance
        .get_func("take")
        .expect("the component exports `take`")
        .call(&mut store, &[Val::U32(0)])
        .await
        .expect_err("a stream cannot cross the boundary yet");
    assert_eq!(
        unsupported_feature(err),
        "the transfer of a `stream<T>` readable end"
    );
}

#[wcmp_macros::test]
async fn it_refuses_to_lower_a_future_into_an_async_export_at_the_call() {
    let engine = Engine::new().expect("engine");
    let linker = linker_for_every_kind(&engine, Arc::new(AtomicBool::new(false)));
    let (mut store, instance) = instantiate(&engine, &linker, STREAMS_AND_FUTURES).await;

    let err = instance
        .get_func("take-async")
        .expect("the component exports `take-async`")
        .call(&mut store, &[Val::U32(0)])
        .await
        .expect_err("a future cannot cross the boundary yet");
    assert_eq!(
        unsupported_feature(err),
        "the transfer of a `future<T>` readable end"
    );
}

#[wcmp_macros::test]
async fn it_refuses_to_lift_a_future_from_an_export_result_at_the_call() {
    // The guest returns an index; the lift that would take the end it
    // names out of the guest's table fails instead.
    let engine = Engine::new().expect("engine");
    let linker: Linker<()> = Linker::new(&engine);
    let (mut store, instance) = instantiate(&engine, &linker, RETURNS_A_FUTURE).await;

    let err = instance
        .get_func("make")
        .expect("the component exports `make`")
        .call(&mut store, &[])
        .await
        .expect_err("a future cannot cross the boundary yet");
    assert_eq!(
        unsupported_feature(err),
        "the transfer of a `future<T>` readable end"
    );
}

#[wcmp_macros::test]
async fn it_refuses_to_lift_a_stream_into_a_host_function_at_the_call() {
    // The guest passes an index to the host; the lift of the
    // parameter fails before the host body runs.
    let engine = Engine::new().expect("engine");
    let ran = Arc::new(AtomicBool::new(false));
    let body_ran = ran.clone();
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_new(
            "pull",
            FunctionType {
                parameters: vec![FunctionParameter {
                    name: "s".to_owned(),
                    ty: stream_of_bytes(),
                }],
                result: None,
                async_: false,
            },
            move |_call, _args, _results| {
                body_ran.store(true, Ordering::SeqCst);
                Ok(())
            },
        )
        .expect("the registration");
    let (mut store, instance) = instantiate(&engine, &linker, PASSES_A_STREAM_TO_THE_HOST).await;

    let err = instance
        .get_func("run")
        .expect("the component exports `run`")
        .call(&mut store, &[])
        .await
        .expect_err("a stream cannot cross the boundary yet");
    // The lift runs inside the trampoline the guest called, and the
    // runtime layer carries a trampoline's failure back to the host as
    // a substrate failure whose chain holds the polyfill's message.
    let chain = chain(&err);
    assert!(
        chain.contains("unsupported component feature: the transfer of a `stream<T>` readable end"),
        "expected the unsupported transfer, got {chain}"
    );
    assert!(!ran.load(Ordering::SeqCst), "the host body never ran");
}

/// Every message in the source chain of `error`, joined.
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
