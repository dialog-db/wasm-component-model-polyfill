// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Tests of the exports `wcmp::bindgen!` writes: the world struct, its
//! `new`, the interface getters, and the `call_*` methods.
//!
//! `new` acquires every export with its type check, so a component
//! whose export has another type, or that lacks one, fails there and
//! never in a call.

#![cfg(test)]

use wcmp::{Component, Engine, Error, Instance, Linker, Store, TypeMismatchPosition, TypePathStep};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

mod bindings {
    wcmp::bindgen!({
        path: "wit-exports",
        additional_derives: [PartialEq],
    });
}

/// The world's code with no `exports` flag, as a string.
const PLAIN: &str = wcmp::bindgen!({ path: "wit-exports", stringify: true });

/// The world's code with `exports: { default: async }`, as a string.
const ASYNC: &str = wcmp::bindgen!({
    path: "wit-exports",
    exports: { default: async },
    stringify: true,
});

/// The interfaces of the world alone, as a string.
const INTERFACES: &str = wcmp::bindgen!({
    interfaces: "export test:exports/shapes;",
    path: "wit-exports",
    stringify: true,
});

use bindings::Exports;
use bindings::exports::test::exports::shapes::{Point, Shape};

/// The same guest, but its `point` has a field `z` where the world's
/// has `y`.
const OTHER_RECORD: &[u8] = component!(
    r#"
    (component
      (core module $m
        (memory (export "memory") 1)
        (func (export "shape-of") (param i32 i32) (result i32) (i32.const 16))
        (func (export "echo") (param i32 i32) (result i32) (i32.const 32))
        (func (export "realloc") (param i32 i32 i32 i32) (result i32) (i32.const 64))
        (func (export "add") (param i32 i32) (result i32) (local.get 0)))
      (core instance $i (instantiate $m))
      (type $point (record (field "x" u32) (field "z" u32)))
      (type $shape (variant (case "circle" u32) (case "none")))
      (func $shape-of (param "point" $point) (result $shape)
        (canon lift (core func $i "shape-of") (memory (core memory $i "memory"))))
      (func $echo (param "text" string) (result string)
        (canon lift (core func $i "echo") (memory (core memory $i "memory"))
          (realloc (core func $i "realloc"))))
      (instance $shapes
        (export "point" (type $point))
        (export "shape" (type $shape))
        (export "shape-of" (func $shape-of))
        (export "echo" (func $echo)))
      (export "test:exports/shapes" (instance $shapes))
      (func (export "add") (param "a" u32) (param "b" u32) (result u32)
        (canon lift (core func $i "add"))))
    "#
);

/// The same guest without its `add`.
const WITHOUT_ADD: &[u8] = component!(
    r#"
    (component
      (core module $m
        (memory (export "memory") 1)
        (func (export "shape-of") (param i32 i32) (result i32) (i32.const 16))
        (func (export "echo") (param i32 i32) (result i32) (i32.const 32))
        (func (export "realloc") (param i32 i32 i32 i32) (result i32) (i32.const 64)))
      (core instance $i (instantiate $m))
      (type $point (record (field "x" u32) (field "y" u32)))
      (type $shape (variant (case "circle" u32) (case "none")))
      (func $shape-of (param "point" $point) (result $shape)
        (canon lift (core func $i "shape-of") (memory (core memory $i "memory"))))
      (func $echo (param "text" string) (result string)
        (canon lift (core func $i "echo") (memory (core memory $i "memory"))
          (realloc (core func $i "realloc"))))
      (instance $shapes
        (export "point" (type $point))
        (export "shape" (type $shape))
        (export "shape-of" (func $shape-of))
        (export "echo" (func $echo)))
      (export "test:exports/shapes" (instance $shapes)))
    "#
);

/// Instantiate `bytes`, which imports nothing.
async fn instantiate(bytes: &[u8]) -> (Store<()>, Instance) {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, bytes)
        .await
        .expect("the component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the component instantiates");
    (store, instance)
}

#[wcmp_macros::test]
async fn it_calls_a_generated_export_with_a_record_and_returns_a_variant() {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, guests::IMPLEMENTS_EXPORTS)
        .await
        .expect("the component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    Exports::add_to_linker::<(), wcmp::HasSelf<()>>(&mut linker, |state| state)
        .expect("a world with no imports registers nothing");
    let mut store = Store::new(&engine, ()).expect("store");
    let world = Exports::instantiate_async(&mut store, &component, &linker)
        .await
        .expect("the world instantiates");

    let shapes = world.test_exports_shapes();
    let shape = shapes
        .call_shape_of(&mut store, Point { x: 3, y: 4 })
        .await
        .expect("call shape-of");
    assert_eq!(shape, Shape::Circle(7), "the guest computed the variant");

    let echoed = shapes
        .call_echo(&mut store, "borrowed")
        .await
        .expect("call echo");
    assert_eq!(
        echoed, "borrowed",
        "the borrowed string crossed and came back"
    );

    assert_eq!(
        world.call_add(&mut store, 2, 5).await.expect("call add"),
        7,
        "the world's own export runs"
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_component_whose_export_has_another_record() {
    let (mut store, instance) = instantiate(OTHER_RECORD).await;
    let error = Exports::new(&mut store, &instance)
        .err()
        .expect("the record's second field is another");
    let Error::TypeMismatch(mismatch) = &error else {
        panic!("the type check refuses it, got {error:?}");
    };
    let TypeMismatchPosition::TypedConversion { export, path } = &mismatch.position else {
        panic!("the mismatch is the typed conversion's, got {mismatch:?}");
    };
    assert_eq!(export, "shape-of", "the mismatch names the export");
    assert_eq!(
        path.first(),
        Some(&TypePathStep::Parameter(0)),
        "the path starts at the parameter"
    );
    assert!(
        path.iter()
            .any(|step| matches!(step, TypePathStep::Field(name) if name == "z" || name == "y")),
        "the path names the field that differs: {path:?}"
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_component_without_one_of_the_worlds_exports() {
    let (mut store, instance) = instantiate(WITHOUT_ADD).await;
    let error = Exports::new(&mut store, &instance)
        .err()
        .expect("the component does not export `add`");
    assert!(
        matches!(
            &error,
            Error::MissingExport { name, instance: None } if name == "add"
        ),
        "the error names the missing export, got {error:?}"
    );
    assert_eq!(error.to_string(), "no export `add` found");
}

#[wcmp_macros::test]
fn it_emits_the_same_calls_under_the_async_flag() {
    assert_eq!(PLAIN, ASYNC, "`async` on an export changes nothing");
}

#[wcmp_macros::test]
fn it_emits_no_world_struct_with_interfaces() {
    let words = INTERFACES.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        words.contains("pub struct Guest") && !words.contains("instantiate_async"),
        "the interface is generated, and no world struct: {INTERFACES}"
    );
}

#[path = "support/guests.rs"]
mod guests;
#[path = "../../wcmp/tests/support/backend.rs"]
mod test_backend;
