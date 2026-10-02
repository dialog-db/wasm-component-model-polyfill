// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The downstream crate at run time, natively: the engine it makes runs a
//! component, so the polyfill and the backend link and work together outside
//! the polyfill's workspace, not only build.

#![cfg(not(target_arch = "wasm32"))]

use wcmp::{Component, Linker, Store, Val};

/// A component whose one export adds two numbers in a core module, so a call
/// reaches the backend.
const ADD: &str = r#"
    (component
      (core module $m
        (func (export "add") (param i32 i32) (result i32)
          local.get 0
          local.get 1
          i32.add))
      (core instance $i (instantiate $m))
      (func (export "add") (param "a" u32) (param "b" u32) (result u32)
        (canon lift (core func $i "add"))))
"#;

#[test]
fn it_runs_a_component_through_the_engine() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a runtime");
    runtime.block_on(async {
        let engine = wcmp_downstream::engine().expect("the engine");
        let bytes = wat::parse_str(ADD).expect("the component parses");
        let component = Component::new(&engine, &bytes)
            .await
            .expect("the component compiles");
        let linker: Linker<()> = Linker::new(&engine);
        let mut store: Store<()> = Store::new(&engine, ()).expect("a store");
        let instance = linker
            .instantiate(&mut store, &component)
            .await
            .expect("the component instantiates");
        let returned = instance
            .get_func("add")
            .expect("the export")
            .call(&mut store, &[Val::U32(2), Val::U32(3)])
            .await
            .expect("the call returns");
        assert!(matches!(&returned[..], [Val::U32(5)]));
    });
}
