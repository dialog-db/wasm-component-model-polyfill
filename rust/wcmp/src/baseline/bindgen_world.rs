// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! When the type check of a world that `bindgen!` writes runs.
//!
//! The world's `new` acquires each export as a typed function, so the
//! check runs once for each there. A `call_*` method calls the typed
//! function it keeps and runs no check. The macro is invoked inside the
//! polyfill with `wcmp_crate: crate`, which is how the test reads the
//! crate's own count of type checks.

#![cfg(test)]

use crate::typed::type_check_runs;
use crate::{Component, Engine, Linker, Store};
use wcmp_macros::component;

mod bindings {
    crate::bindgen!({
        inline: "
            package test:count;

            world counted {
                export id: func(x: u32) -> u32;
            }
        ",
        wcmp_crate: crate,
    });
}

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
async fn it_runs_the_type_check_of_a_world_once_in_new_and_never_in_a_call() {
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

    let before = type_check_runs();
    let world = bindings::Counted::new(&mut store, &instance).expect("the world's exports");
    let acquired = type_check_runs();
    assert_eq!(
        acquired,
        before + 1,
        "`new` ran the check of its one export once"
    );

    for value in 0..1000_u32 {
        let echoed = world.call_id(&mut store, value).await.expect("the call");
        assert_eq!(echoed, value);
    }
    assert_eq!(
        type_check_runs(),
        acquired,
        "a thousand calls ran the type check no more"
    );
}
