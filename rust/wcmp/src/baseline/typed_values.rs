// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! When the type check of the typed value traits runs.
//!
//! The check compares a typed function's Rust types with the types the
//! component declares once, when the host acquires the function. A
//! call trusts it: the check never runs on a call.

#![cfg(test)]

use crate::typed::type_check_runs;
use crate::{Component, Engine, Linker, Store};
use wcmp_macros::component;

/// A component whose export returns the `u32` it receives.
const IDENTITY: &[u8] = component!(
    r#"
    (component
      (core module $m (func (export "id") (param i32) (result i32) local.get 0))
      (core instance $i (instantiate $m))
      (func (export "id") (param "x" u32) (result u32) (canon lift (core func $i "id"))))
    "#
);

#[wcmp_macros::test]
async fn it_runs_the_type_check_when_a_typed_function_is_acquired_and_never_on_a_call() {
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, IDENTITY)
        .await
        .expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiation succeeds");
    let func = instance.get_func("id").expect("the export");

    let before = type_check_runs();
    let id = func.typed::<(u32,), (u32,)>().expect("the typed signature");
    let acquired = type_check_runs();
    assert_eq!(
        acquired,
        before + 1,
        "acquiring the typed function ran the check once"
    );

    for value in 0..1000_u32 {
        let (echoed,) = id.call(&mut store, (value,)).await.expect("the call");
        assert_eq!(echoed, value);
    }
    assert_eq!(
        type_check_runs(),
        acquired,
        "a thousand calls ran the type check no more"
    );
}
