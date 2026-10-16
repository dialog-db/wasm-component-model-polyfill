// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Baseline tests for the Rust forms of the component value types that
//! the typed value traits give beyond the scalars, `String`, `Vec<T>`,
//! and `Option<T>`: `Result<T, E>`, tuples as values, `Box<str>` and
//! `Arc<str>`, `Box<[T]>` and `Arc<[T]>`, the lower-only `str`, `[T]`,
//! and the forwarding `&T`, `Box<T>`, and `Arc<T>`, and the untyped
//! readers `StreamAny` and `FutureAny`.
//!
//! Each form crosses through an export that returns what it receives,
//! so one call lowers it into the guest and lifts it back out. A form
//! that lowers only crosses in, and the echo comes back as the owning
//! form of the same type.

#![cfg(test)]

use std::sync::Arc;

use wcmp::{
    Component, Engine, Error, FutureAny, FutureReader, Instance, Linker, Store, StreamAny,
    StreamReader, TypeMismatchPosition, TypePathStep, TypedFunc,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// A guest whose exports return what they receive.
///
/// `echo-pair` serves every type that flattens to a pointer and a
/// length, and `echo-three` every type that flattens to three `i32`s and
/// spills its result: a discriminant and two joined payload slots. Both
/// write their result into memory the guest's allocator gives.
/// `echo-handle` returns the readable end it receives.
const FORMS: &[u8] = component!(
    r#"
    (component
      (core module $m
        (memory (export "memory") 1)
        (global $bump (mut i32) (i32.const 16))
        (func $realloc (export "cabi_realloc")
              (param i32 i32) (param $align i32) (param $size i32) (result i32)
          (local $ptr i32)
          global.get $bump local.get $align i32.add i32.const 1 i32.sub
          local.get $align i32.const 1 i32.sub i32.const -1 i32.xor i32.and
          local.tee $ptr local.get $size i32.add global.set $bump
          local.get $ptr)
        (func (export "echo-pair") (param $ptr i32) (param $len i32) (result i32)
          (local $ret i32)
          (local.set $ret (call $realloc (i32.const 0) (i32.const 0) (i32.const 4) (i32.const 8)))
          (i32.store (local.get $ret) (local.get $ptr))
          (i32.store offset=4 (local.get $ret) (local.get $len))
          (local.get $ret))
        (func (export "echo-three") (param $case i32) (param $a i32) (param $b i32) (result i32)
          (local $ret i32)
          (local.set $ret (call $realloc (i32.const 0) (i32.const 0) (i32.const 4) (i32.const 12)))
          (i32.store8 (local.get $ret) (local.get $case))
          (i32.store offset=4 (local.get $ret) (local.get $a))
          (i32.store offset=8 (local.get $ret) (local.get $b))
          (local.get $ret))
        (func (export "echo-handle") (param i32) (result i32) local.get 0)
        (func (export "ignore") (param i32 i32 i32 i32)))
      (core instance $i (instantiate $m))
      (func (export "echo-string") (param "s" string) (result string)
        (canon lift (core func $i "echo-pair")
          (memory (core memory $i "memory")) (realloc (core func $i "cabi_realloc"))))
      (func (export "echo-numbers") (param "xs" (list u32)) (result (list u32))
        (canon lift (core func $i "echo-pair")
          (memory (core memory $i "memory")) (realloc (core func $i "cabi_realloc"))))
      (func (export "echo-tuples") (param "xs" (list (tuple u32 string)))
        (result (list (tuple u32 string)))
        (canon lift (core func $i "echo-pair")
          (memory (core memory $i "memory")) (realloc (core func $i "cabi_realloc"))))
      (func (export "echo-result") (param "r" (result u32 (error string)))
        (result (result u32 (error string)))
        (canon lift (core func $i "echo-three")
          (memory (core memory $i "memory")) (realloc (core func $i "cabi_realloc"))))
      (func (export "echo-unit-result") (param "r" (result (error string)))
        (result (result (error string)))
        (canon lift (core func $i "echo-three")
          (memory (core memory $i "memory")) (realloc (core func $i "cabi_realloc"))))
      (func (export "echo-tuple-result") (param "r" (result (tuple u32 u32) (error u32)))
        (result (result (tuple u32 u32) (error u32)))
        (canon lift (core func $i "echo-three")
          (memory (core memory $i "memory")) (realloc (core func $i "cabi_realloc"))))
      (func (export "take-fixed") (param "xs" (list u32 4))
        (canon lift (core func $i "ignore")))
      (func (export "echo-stream") (param "s" (stream u8)) (result (stream u8))
        (canon lift (core func $i "echo-handle")))
      (func (export "echo-future") (param "f" (future u32)) (result (future u32))
        (canon lift (core func $i "echo-handle"))))
    "#
);

/// Instantiate [`FORMS`] into a store of its own.
async fn forms() -> (Store<()>, Instance) {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, FORMS)
        .await
        .expect("the component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiation succeeds");
    (store, instance)
}

/// The typed handle of the export `name`.
fn typed<P, R>(instance: &Instance, name: &str) -> TypedFunc<P, R>
where
    P: wcmp::ComponentNamedList + wcmp::Lower,
    R: wcmp::ComponentNamedList + wcmp::Lift,
{
    instance
        .get_func(name)
        .expect("the export")
        .typed::<P, R>()
        .expect("the export has the typed signature")
}

#[wcmp_macros::test]
async fn it_crosses_a_result_in_each_case() {
    let (mut store, instance) = forms().await;
    let echo = typed::<(Result<u32, String>,), (Result<u32, String>,)>(&instance, "echo-result");
    for value in [Ok(7), Err("refused".to_owned())] {
        let (echoed,) = echo
            .call(&mut store, (value.clone(),))
            .await
            .expect("the result crosses");
        assert_eq!(echoed, value);
    }

    // `()` stands for a case with no payload.
    let unit = typed::<(Result<(), String>,), (Result<(), String>,)>(&instance, "echo-unit-result");
    for value in [Ok(()), Err("refused".to_owned())] {
        let (echoed,) = unit
            .call(&mut store, (value.clone(),))
            .await
            .expect("the result crosses");
        assert_eq!(echoed, value);
    }
}

#[wcmp_macros::test]
async fn it_crosses_a_tuple_inside_a_list_and_a_result() {
    let (mut store, instance) = forms().await;
    let pairs = vec![(1, "one".to_owned()), (2, "two".to_owned())];
    let (echoed,) = typed::<(Vec<(u32, String)>,), (Vec<(u32, String)>,)>(&instance, "echo-tuples")
        .call(&mut store, (pairs.clone(),))
        .await
        .expect("the list of tuples crosses");
    assert_eq!(echoed, pairs);

    let echo = typed::<(Result<(u32, u32), u32>,), (Result<(u32, u32), u32>,)>(
        &instance,
        "echo-tuple-result",
    );
    for value in [Ok((3, 4)), Err(5)] {
        let (echoed,) = echo
            .call(&mut store, (value,))
            .await
            .expect("the result of a tuple crosses");
        assert_eq!(echoed, value);
    }
}

#[wcmp_macros::test]
async fn it_crosses_each_form_of_a_string() {
    let (mut store, instance) = forms().await;
    let (boxed,) = typed::<(Box<str>,), (Box<str>,)>(&instance, "echo-string")
        .call(&mut store, ("boxed".into(),))
        .await
        .expect("a boxed string crosses");
    assert_eq!(&*boxed, "boxed");
    let (shared,) = typed::<(Arc<str>,), (Arc<str>,)>(&instance, "echo-string")
        .call(&mut store, (Arc::from("shared"),))
        .await
        .expect("a shared string crosses");
    assert_eq!(&*shared, "shared");

    // `str` lowers only, through a reference; the echo comes back as a
    // `String`.
    let (owned,) = typed::<(&'static str,), (String,)>(&instance, "echo-string")
        .call(&mut store, ("borrowed",))
        .await
        .expect("a string slice lowers");
    assert_eq!(owned, "borrowed");
}

#[wcmp_macros::test]
async fn it_crosses_each_form_of_a_list() {
    let (mut store, instance) = forms().await;
    let (boxed,) = typed::<(Box<[u32]>,), (Box<[u32]>,)>(&instance, "echo-numbers")
        .call(&mut store, (vec![1, 2, 3].into_boxed_slice(),))
        .await
        .expect("a boxed slice crosses");
    assert_eq!(&*boxed, &[1, 2, 3]);
    let (shared,) = typed::<(Arc<[u32]>,), (Arc<[u32]>,)>(&instance, "echo-numbers")
        .call(&mut store, (Arc::from(vec![4, 5]),))
        .await
        .expect("a shared slice crosses");
    assert_eq!(&*shared, &[4, 5]);

    // `[T]` lowers only, through a reference; so do `Box<T>` and
    // `Arc<T>` of a type that lowers.
    static NUMBERS: [u32; 2] = [6, 7];
    let (owned,) = typed::<(&'static [u32],), (Vec<u32>,)>(&instance, "echo-numbers")
        .call(&mut store, (&NUMBERS[..],))
        .await
        .expect("a slice lowers");
    assert_eq!(owned, vec![6, 7]);
    let (owned,) = typed::<(Box<Vec<u32>>,), (Vec<u32>,)>(&instance, "echo-numbers")
        .call(&mut store, (Box::new(vec![8]),))
        .await
        .expect("a box of a vector lowers as the vector");
    assert_eq!(owned, vec![8]);
    let (owned,) = typed::<(Arc<String>,), (String,)>(&instance, "echo-string")
        .call(&mut store, (Arc::new("arc".to_owned()),))
        .await
        .expect("an arc of a string lowers as the string");
    assert_eq!(owned, "arc");
}

#[wcmp_macros::test]
async fn it_crosses_an_untyped_stream_and_future() {
    let (mut store, instance) = forms().await;
    let stream = StreamReader::<u8>::new(&mut store.as_context_mut(), vec![1_u8, 2, 3])
        .expect("a stream the host creates");
    let stream = stream
        .try_into_stream_any(&mut store.as_context_mut())
        .expect("the untyped reader of the stream");
    let (echoed,) = typed::<(StreamAny,), (StreamAny,)>(&instance, "echo-stream")
        .call(&mut store, (stream,))
        .await
        .expect("an untyped stream crosses");
    echoed
        .try_into_stream_reader::<u8>()
        .expect("the stream still carries bytes")
        .close(&mut store.as_context_mut())
        .expect("the host closes it");

    let future = FutureReader::<u32>::new(&mut store.as_context_mut(), async {
        Ok::<u32, wcmp::Error>(9)
    })
    .expect("a future the host creates")
    .try_into_future_any(&mut store.as_context_mut())
    .expect("the untyped reader of the future");
    let (echoed,) = typed::<(FutureAny,), (FutureAny,)>(&instance, "echo-future")
        .call(&mut store, (future,))
        .await
        .expect("an untyped future crosses");
    echoed
        .try_into_future_reader::<u32>()
        .expect("the future still carries a u32")
        .close(&mut store.as_context_mut())
        .expect("the host closes it");
}

#[wcmp_macros::test]
async fn it_refuses_a_fixed_length_list_of_another_length() {
    let (_store, instance) = forms().await;
    let func = || instance.get_func("take-fixed").expect("the export");
    func()
        .typed::<([u32; 4],), ()>()
        .expect("four elements match `list<u32, 4>`");
    let error = func()
        .typed::<([u32; 3],), ()>()
        .expect_err("three elements do not match `list<u32, 4>`");
    assert!(
        matches!(&error, Error::TypeMismatch(mismatch) if matches!(
            &mismatch.position,
            TypeMismatchPosition::TypedConversion { path, .. } if *path == [TypePathStep::Parameter(0)]
        )),
        "{error}"
    );
}

#[wcmp_macros::test]
async fn it_names_the_payload_of_a_result_whose_types_differ() {
    let (_store, instance) = forms().await;
    let func = || instance.get_func("echo-result").expect("the export");
    let path = |error: Error| match error {
        Error::TypeMismatch(mismatch) => match mismatch.position {
            TypeMismatchPosition::TypedConversion { path, .. } => path,
            other => panic!("expected a typed conversion, got {other:?}"),
        },
        other => panic!("expected a type mismatch, got {other:?}"),
    };
    let wrong_error = func()
        .typed::<(Result<u32, u32>,), (Result<u32, String>,)>()
        .expect_err("a `u32` error is not a `string` one");
    assert_eq!(
        path(wrong_error),
        [TypePathStep::Parameter(0), TypePathStep::ResultErr]
    );
    let unit_ok = func()
        .typed::<(Result<(), String>,), (Result<u32, String>,)>()
        .expect_err("a case with no payload is not a `u32` one");
    assert_eq!(
        path(unit_ok),
        [TypePathStep::Parameter(0), TypePathStep::ResultOk]
    );
}

#[path = "support/backend.rs"]
mod test_backend;
