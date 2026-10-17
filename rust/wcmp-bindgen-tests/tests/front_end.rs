// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Tests of the front-end of `wcmp::bindgen!`: the invocation forms, the
//! WIT they read, the modules, and the Rust types.
//!
//! A generated record, variant, enum, and flags type crosses as the
//! hand-derived equivalent does: the same layout, the same type check,
//! and the same values, through a guest whose exports return what they
//! are given.

#![cfg(test)]
#![allow(dead_code)]

use wcmp::{Component, ComponentType, Engine, Instance, Lift, Linker, Lower, Store, flags};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// `bindgen!()`: the one world of `wit/`.
mod no_arguments {
    wcmp::bindgen!();
}

/// `bindgen!("world")`: a named world of `wit/`.
mod world_only {
    wcmp::bindgen!("forms");
}

/// `bindgen!(in "path")`: the one world of another directory.
mod path_only {
    wcmp::bindgen!(in "wit-other");
}

/// `bindgen!("world" in "path")`: a named world of another directory.
mod world_and_path {
    wcmp::bindgen!("other" in "wit-other");
}

/// `bindgen!({ … })`: the braced form.
mod braced {
    wcmp::bindgen!({
        world: "other",
        path: "wit-other",
    });
}

/// `inline`: WIT written in the invocation.
mod inline {
    wcmp::bindgen!({
        inline: "
            package test:inline;

            interface colors {
                enum color { red, green }
            }

            world inline {
                import colors;
            }
        ",
    });
}

/// `interfaces`: the interfaces named, with no world of their own.
mod interfaces {
    wcmp::bindgen!({
        interfaces: "import test:other/sizes;",
        path: "wit-other",
    });
}

/// The types of `wit-types/`, which the type tests compare.
mod generated {
    wcmp::bindgen!({
        path: "wit-types",
        additional_derives: [PartialEq, Eq, Hash],
    });
}

/// Two versions of one package, which generate two modules whose names
/// carry the versions.
mod versions {
    wcmp::bindgen!(in "wit-versions");
}

/// The re-export a crate makes of the polyfill, which `wcmp_crate` names.
mod polyfill {
    pub use wcmp::*;
}

/// The types of `wit-types/` through a re-export of the polyfill.
mod through_a_reexport {
    wcmp::bindgen!({
        path: "wit-types",
        wcmp_crate: crate::polyfill,
    });
}

/// The generated code of `wit/`'s world, as a string.
const STRINGIFIED: &str = wcmp::bindgen!({ stringify: true });

/// The generated code of `wit-types/` through a re-export, as a string.
const STRINGIFIED_THROUGH_A_REEXPORT: &str = wcmp::bindgen!({
    path: "wit-types",
    wcmp_crate: crate::polyfill,
    stringify: true,
});

/// The generated code of `wit/`, built from the file the macro wrote.
mod from_a_file {
    wcmp::bindgen!({ include_generated_code_from_file: true });
}

/// `record point { x: u32, y: u32 }`, derived by hand.
#[derive(ComponentType, Lift, Lower, Clone, Copy, Debug, PartialEq)]
#[component(record)]
struct Point {
    x: u32,
    y: u32,
}

/// `variant shape { circle(u32), none }`, derived by hand.
#[derive(ComponentType, Lift, Lower, Clone, Copy, Debug, PartialEq)]
#[component(variant)]
enum Shape {
    #[component(name = "circle")]
    Circle(u32),
    #[component(name = "none")]
    None,
}

/// `enum level { info, error }`, derived by hand.
#[derive(ComponentType, Lift, Lower, Clone, Copy, Debug, PartialEq)]
#[component(enum)]
#[repr(u8)]
enum Level {
    #[component(name = "info")]
    Info,
    #[component(name = "error")]
    Error,
}

flags! {
    /// `flags permissions { read, write }`, by hand.
    Permissions {
        #[component(name = "read")]
        const READ;
        #[component(name = "write")]
        const WRITE;
    }
}

/// A guest whose exports return what they are given: a `point`, a
/// `shape`, a `level`, and a `permissions`.
const ECHOES: &[u8] = component!(
    r#"
    (component
      (core module $m
        (memory (export "memory") 1)
        (func (export "echo-two") (param $a i32) (param $b i32) (result i32)
          (i32.store (i32.const 16) (local.get $a))
          (i32.store offset=4 (i32.const 16) (local.get $b))
          (i32.const 16))
        (func (export "echo-one") (param i32) (result i32) (local.get 0)))
      (core instance $i (instantiate $m))
      (type $point' (record (field "x" u32) (field "y" u32)))
      (export $point "point" (type $point'))
      (type $shape' (variant (case "circle" u32) (case "none")))
      (export $shape "shape" (type $shape'))
      (type $level' (enum "info" "error"))
      (export $level "level" (type $level'))
      (type $permissions' (flags "read" "write"))
      (export $permissions "permissions" (type $permissions'))
      (func (export "echo-point") (param "p" $point) (result $point)
        (canon lift (core func $i "echo-two") (memory (core memory $i "memory"))))
      (func (export "echo-shape") (param "s" $shape) (result $shape)
        (canon lift (core func $i "echo-two") (memory (core memory $i "memory"))))
      (func (export "echo-level") (param "l" $level) (result $level)
        (canon lift (core func $i "echo-one")))
      (func (export "echo-permissions") (param "p" $permissions) (result $permissions)
        (canon lift (core func $i "echo-one"))))
    "#
);

/// Instantiate [`ECHOES`].
async fn echoes() -> (Store<()>, Instance) {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, ECHOES)
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

/// Call the echo export `name` with `value` through a typed handle of
/// `T`, and answer what it returned.
async fn echo<T>(store: &mut Store<()>, instance: &Instance, name: &str, value: T) -> T
where
    T: ComponentType + Lift + Lower + Send + Sync + 'static,
{
    let (echoed,) = instance
        .get_func(name)
        .unwrap_or_else(|| panic!("the guest exports `{name}`"))
        .typed::<(T,), (T,)>()
        .unwrap_or_else(|error| panic!("the type check of `{name}` passes: {error}"))
        .call(store, (value,))
        .await
        .unwrap_or_else(|error| panic!("the call of `{name}` succeeds: {error}"));
    echoed
}

/// The layout of `T`.
fn layout<T: ComponentType>() -> (usize, u32) {
    (T::SIZE32, T::ALIGN32)
}

use generated::test::types::shapes as types;

#[wcmp_macros::test]
fn it_generates_the_modules_of_each_invocation_form() {
    let _: no_arguments::test::forms::shapes::Point =
        no_arguments::test::forms::shapes::Point { x: 1, y: 2 };
    let _: world_only::test::forms::shapes::Point =
        world_only::test::forms::shapes::Point { x: 1, y: 2 };
    let _ = path_only::test::other::sizes::Size::Small;
    let _ = world_and_path::test::other::sizes::Size::Large;
    let _ = braced::test::other::sizes::Size::Small;
    let _ = inline::test::inline::colors::Color::Green;
    let _ = interfaces::test::other::sizes::Size::Large;
    let _ = from_a_file::test::forms::shapes::Point { x: 3, y: 4 };
}

#[wcmp_macros::test]
fn it_lays_out_each_generated_type_as_the_hand_derived_one() {
    assert_eq!(layout::<types::Point>(), layout::<Point>(), "record");
    assert_eq!(layout::<types::Shape>(), layout::<Shape>(), "variant");
    assert_eq!(layout::<types::Level>(), layout::<Level>(), "enum");
    assert_eq!(
        layout::<types::Permissions>(),
        layout::<Permissions>(),
        "flags"
    );
}

#[wcmp_macros::test]
async fn it_crosses_each_generated_type_as_the_hand_derived_one_does() {
    let (mut store, instance) = echoes().await;

    let point = types::Point { x: 3, y: 4 };
    assert_eq!(
        echo(&mut store, &instance, "echo-point", point).await,
        point
    );
    assert_eq!(
        echo(&mut store, &instance, "echo-point", Point { x: 3, y: 4 }).await,
        Point { x: 3, y: 4 }
    );

    let shape = types::Shape::Circle(9);
    assert_eq!(
        echo(&mut store, &instance, "echo-shape", shape).await,
        shape
    );
    assert_eq!(
        echo(&mut store, &instance, "echo-shape", types::Shape::None).await,
        types::Shape::None
    );
    assert_eq!(
        echo(&mut store, &instance, "echo-shape", Shape::Circle(9)).await,
        Shape::Circle(9)
    );

    assert_eq!(
        echo(&mut store, &instance, "echo-level", types::Level::Error).await,
        types::Level::Error
    );
    assert_eq!(
        echo(&mut store, &instance, "echo-level", Level::Error).await,
        Level::Error
    );

    let both = types::Permissions::READ | types::Permissions::WRITE;
    assert_eq!(
        echo(&mut store, &instance, "echo-permissions", both).await,
        both
    );
    assert_eq!(
        echo(
            &mut store,
            &instance,
            "echo-permissions",
            Permissions::READ | Permissions::WRITE
        )
        .await,
        Permissions::READ | Permissions::WRITE
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_generated_type_where_the_hand_derived_one_is_refused() {
    // The type check compares names and types, so a generated record
    // checks against a variant export as the hand-derived one does.
    let (_store, instance) = echoes().await;
    let shape = || {
        instance
            .get_func("echo-shape")
            .expect("the guest exports `echo-shape`")
    };
    let generated = shape()
        .typed::<(types::Point,), (types::Point,)>()
        .expect_err("a record is not a variant");
    let derived = shape()
        .typed::<(Point,), (Point,)>()
        .expect_err("a record is not a variant");
    // The messages differ only in the Rust type each names.
    let expected = |error: wcmp::Error| {
        let message = error.to_string();
        message[..message.find("found Rust type").unwrap_or(message.len())].to_owned()
    };
    assert_eq!(
        expected(generated),
        expected(derived),
        "the two type checks refuse it alike"
    );
}

#[wcmp_macros::test]
fn it_generates_a_module_for_each_version_of_a_package() {
    let _ = versions::my::dep0_1_0::a::Version1 { value: 1 };
    let _ = versions::my::dep0_2_0::a::Version2 { value: 2 };
}

#[wcmp_macros::test]
fn it_gives_the_generated_code_as_a_string() {
    let words = STRINGIFIED.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        words.contains("pub struct Point"),
        "the string holds the generated record: {STRINGIFIED}"
    );
}

/// The code `wit/`'s world generates, as committed. The test runs on
/// both targets, so the same snapshot holding on each is the macro
/// writing the same tokens for each.
const SNAPSHOT: &str = include_str!("snapshots/forms.txt");

#[wcmp_macros::test]
fn it_writes_the_same_tokens_for_every_target() {
    if STRINGIFIED != SNAPSHOT {
        println!("--- generated ---\n{STRINGIFIED}\n--- end ---");
    }
    assert_eq!(
        STRINGIFIED, SNAPSHOT,
        "the generated code matches the snapshot"
    );
}

#[wcmp_macros::test]
fn it_adds_the_additional_derives_to_each_generated_type() {
    use std::collections::HashSet;
    let points: HashSet<types::Point> = [types::Point { x: 1, y: 2 }].into_iter().collect();
    let shapes: HashSet<types::Shape> = [types::Shape::None].into_iter().collect();
    let levels: HashSet<types::Level> = [types::Level::Info].into_iter().collect();
    assert_eq!((points.len(), shapes.len(), levels.len()), (1, 1, 1));
}

#[wcmp_macros::test]
fn it_moves_every_emitted_path_under_wcmp_crate() {
    let _ = through_a_reexport::test::types::shapes::Point { x: 1, y: 2 };
    let _ = through_a_reexport::test::types::shapes::Permissions::READ;
    let code = STRINGIFIED_THROUGH_A_REEXPORT.replace(' ', "");
    assert!(
        !code.contains("::wcmp::"),
        "no emitted path names `::wcmp`: {STRINGIFIED_THROUGH_A_REEXPORT}"
    );
    assert!(
        code.contains("crate::polyfill::"),
        "the paths name the re-export"
    );
}

#[wcmp_macros::test]
fn it_generates_an_item_behind_a_feature_gate() {
    let _ = types::Gated { value: 1 };
}

#[path = "../../wcmp/tests/support/backend.rs"]
mod test_backend;
