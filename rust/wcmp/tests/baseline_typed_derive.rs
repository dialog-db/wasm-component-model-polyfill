// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Baseline tests for the derive macros `ComponentType`, `Lift`, and
//! `Lower`, and for `flags!`.
//!
//! A derived record, variant, or enum crosses as a Rust type, field by
//! field, with no `Val` at any depth, and its type check compares field
//! and case names as well as types. The file installs a counting global
//! allocator, so a test can assert that a list of derived records costs
//! the host nothing per record.

#![cfg(test)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use wcmp::{
    Component, ComponentType, Engine, Error, HostCall, Instance, InterfaceIdentifier, Lift, Linker,
    Lower, Store, TypeMismatchPosition, TypePathStep, TypedFunc, flags,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

thread_local! {
    /// Whether this thread's allocations are being counted.
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    /// How many allocations this thread has made while counting.
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

/// A global allocator that counts the allocations of the thread under
/// measurement and passes everything through to the system's.
struct CountingAllocator;

/// Count one allocation, when this thread is counting.
fn tally() {
    if COUNTING.try_with(Cell::get).unwrap_or(false) {
        let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
    }
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        tally();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        tally();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        tally();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Count the allocations `future` makes on this thread until it
/// completes, and answer them with what it produced.
async fn counted<O>(future: impl core::future::Future<Output = O>) -> (usize, O) {
    ALLOCATIONS.with(|count| count.set(0));
    COUNTING.with(|counting| counting.set(true));
    let output = future.await;
    COUNTING.with(|counting| counting.set(false));
    (ALLOCATIONS.with(Cell::get), output)
}

/// `record point { x: u32, y: u32 }`.
#[derive(ComponentType, Lift, Lower, Clone, Copy, Debug, PartialEq)]
#[component(record)]
struct Point {
    x: u32,
    y: u32,
}

/// `variant shape { circle(u32), none }`, with the cases renamed.
#[derive(ComponentType, Lift, Lower, Clone, Copy, Debug, PartialEq)]
#[component(variant)]
enum Shape {
    #[component(name = "circle")]
    Circle(u32),
    #[component(name = "none")]
    Nothing,
}

/// `enum level { info, error }`.
#[derive(ComponentType, Lift, Lower, Clone, Copy, Debug, PartialEq)]
#[component(enum)]
#[repr(u8)]
enum Level {
    #[component(name = "info")]
    Info,
    #[component(name = "error")]
    Error,
}

/// `record renamed { first-field: u32, level: level }`, with its first
/// field renamed.
#[derive(ComponentType, Lift, Lower, Clone, Copy, Debug, PartialEq)]
#[component(record)]
struct Renamed {
    #[component(name = "first-field")]
    first: u32,
    level: Level,
}

/// A record whose second field has another name than `point`'s.
#[derive(ComponentType, Lift, Lower, Debug)]
#[component(record)]
struct PointZ {
    x: u32,
    z: u32,
}

/// A record whose first field has another type than `point`'s.
#[derive(ComponentType, Lift, Lower, Debug)]
#[component(record)]
struct PointWide {
    x: u64,
    y: u32,
}

/// A record with one field more than `point`.
#[derive(ComponentType, Lift, Lower, Debug)]
#[component(record)]
struct Point3 {
    x: u32,
    y: u32,
    z: u32,
}

flags! {
    One {
        const A;
    }
}

flags! {
    Eight {
        const A; const B; const C; const D; const E; const F; const G; const H;
    }
}

flags! {
    Nine {
        const A; const B; const C; const D; const E; const F; const G; const H; const I;
    }
}

flags! {
    ThirtyTwo {
        const F0; const F1; const F2; const F3; const F4; const F5; const F6; const F7;
        const F8; const F9; const F10; const F11; const F12; const F13; const F14; const F15;
        const F16; const F17; const F18; const F19; const F20; const F21; const F22; const F23;
        const F24; const F25; const F26; const F27; const F28; const F29; const F30; const F31;
    }
}

flags! {
    ThirtyThree {
        const F0; const F1; const F2; const F3; const F4; const F5; const F6; const F7;
        const F8; const F9; const F10; const F11; const F12; const F13; const F14; const F15;
        const F16; const F17; const F18; const F19; const F20; const F21; const F22; const F23;
        const F24; const F25; const F26; const F27; const F28; const F29; const F30; const F31;
        const F32;
    }
}

flags! {
    Permissions {
        #[component(name = "read")]
        const READ;
        #[component(name = "write")]
        const WRITE;
    }
}

// Two flags, as many as `permissions` has, named otherwise.
flags! {
    ReadExec {
        #[component(name = "read")]
        const READ;
        #[component(name = "exec")]
        const EXEC;
    }
}

flags! {
    Three {
        const A; const B; const C;
    }
}

/// `record flagged { perms: permissions, nine: nine, wide: thirty-two,
/// rest: list<u32, 14> }`: three flags words of each width, and
/// seventeen flat values in all, one more than a call's parameters
/// take, so the record crosses into the guest in memory.
#[derive(ComponentType, Lift, Lower, Clone, Copy, Debug, PartialEq)]
#[component(record)]
struct Flagged {
    perms: Permissions,
    nine: Nine,
    wide: ThirtyTwo,
    rest: [u32; 14],
}

/// The interface the guest of [`DERIVED`] imports.
const HOST: &str = "wcmp-tests:derive/host@0.1.0";

/// A guest whose exports and imports take derived types.
///
/// - `shape-of` takes a `point` and returns `circle(x + y)`.
/// - `call-describe` hands the host's `describe` `circle(r)` and
///   returns the `point` the host answers.
/// - `echo-points` and `count-points` return the list of points they
///   receive, or its length.
/// - `echo-renamed`, `echo-shape`, and the `echo-…` exports of each
///   flags type of 1, 8, 9, and 32 flags and of `permissions`, return
///   what they receive.
/// - `echo-flagged` returns the `flagged` it receives, at the address
///   the host lowered it to.
/// - `padded-one` returns the flags word `0xff` for a type of one
///   flag, and `padded-pair` returns a `tuple<one, nine>` whose words
///   are all ones, in memory.
/// - `peek` reads a word of memory.
const DERIVED: &[u8] = component!(
    r#"
    (component
      (core module $libc
        (memory (export "memory") 1)
        (global $bump (mut i32) (i32.const 16))
        (func (export "cabi_realloc")
              (param $old i32) (param $old-size i32) (param $align i32) (param $size i32)
              (result i32)
          (local $ptr i32) (local $end i32) (local $have i32)
          global.get $bump local.get $align i32.add i32.const 1 i32.sub
          local.get $align i32.const 1 i32.sub i32.const -1 i32.xor i32.and
          local.set $ptr
          local.get $ptr local.get $size i32.add local.set $end
          memory.size i32.const 16 i32.shl local.set $have
          (if (i32.gt_u (local.get $end) (local.get $have))
            (then
              (if (i32.eq
                    (memory.grow
                      (i32.shr_u
                        (i32.add (i32.sub (local.get $end) (local.get $have)) (i32.const 65535))
                        (i32.const 16)))
                    (i32.const -1))
                (then unreachable))))
          local.get $end global.set $bump
          local.get $ptr)
        (func (export "reset") i32.const 16 global.set $bump)
        (func (export "peek") (param i32) (result i32) (i32.load (local.get 0))))
      (core instance $libc (instantiate $libc))
      (import "wcmp-tests:derive/host@0.1.0" (instance $host
        (type $point' (record (field "x" u32) (field "y" u32)))
        (export "point" (type $point (eq $point')))
        (type $shape' (variant (case "circle" u32) (case "none")))
        (export "shape" (type $shape (eq $shape')))
        (export "describe" (func (param "s" $shape) (result $point)))))
      (alias export $host "point" (type $point))
      (alias export $host "shape" (type $shape))
      (alias export $host "describe" (func $describe))
      (core func $describe-lower (canon lower (func $describe)
        (memory (core memory $libc "memory"))))
      (type $level' (enum "info" "error"))
      (export $level "level" (type $level'))
      (type $renamed' (record (field "first-field" u32) (field "level" $level)))
      (export $renamed "renamed" (type $renamed'))
      (type $one' (flags "A"))
      (export $one "one" (type $one'))
      (type $eight' (flags "A" "B" "C" "D" "E" "F" "G" "H"))
      (export $eight "eight" (type $eight'))
      (type $nine' (flags "A" "B" "C" "D" "E" "F" "G" "H" "I"))
      (export $nine "nine" (type $nine'))
      (type $thirty-two' (flags
        "F0" "F1" "F2" "F3" "F4" "F5" "F6" "F7" "F8" "F9" "F10" "F11" "F12" "F13" "F14" "F15"
        "F16" "F17" "F18" "F19" "F20" "F21" "F22" "F23" "F24" "F25" "F26" "F27" "F28" "F29"
        "F30" "F31"))
      (export $thirty-two "thirty-two" (type $thirty-two'))
      (type $permissions' (flags "read" "write"))
      (export $permissions "permissions" (type $permissions'))
      (type $flagged' (record
        (field "perms" $permissions) (field "nine" $nine) (field "wide" $thirty-two)
        (field "rest" (list u32 14))))
      (export $flagged "flagged" (type $flagged'))
      (core module $m
        (import "libc" "memory" (memory 1))
        (import "libc" "cabi_realloc" (func $realloc (param i32 i32 i32 i32) (result i32)))
        (import "host" "describe" (func $describe (param i32 i32 i32)))
        (func $two (param $a i32) (param $b i32) (result i32)
          (local $ret i32)
          (local.set $ret (call $realloc (i32.const 0) (i32.const 0) (i32.const 4) (i32.const 8)))
          (i32.store (local.get $ret) (local.get $a))
          (i32.store offset=4 (local.get $ret) (local.get $b))
          (local.get $ret))
        (func (export "shape-of") (param $x i32) (param $y i32) (result i32)
          (call $two (i32.const 0) (i32.add (local.get $x) (local.get $y))))
        (func (export "call-describe") (param $r i32) (result i32)
          (local $ret i32)
          (local.set $ret (call $realloc (i32.const 0) (i32.const 0) (i32.const 4) (i32.const 8)))
          (call $describe (i32.const 0) (local.get $r) (local.get $ret))
          (local.get $ret))
        (func (export "echo-pair") (param $a i32) (param $b i32) (result i32)
          (call $two (local.get $a) (local.get $b)))
        (func (export "count") (param $ptr i32) (param $len i32) (result i32)
          (local.get $len))
        (func (export "echo-one") (param i32) (result i32) local.get 0)
        (func (export "padded-one") (result i32) i32.const 0xff)
        (func (export "padded-pair") (result i32)
          (i32.store (i32.const 8) (i32.const -1))
          (i32.const 8)))
      (core instance $i (instantiate $m
        (with "libc" (instance $libc))
        (with "host" (instance (export "describe" (func $describe-lower))))))
      (func (export "shape-of") (param "p" $point) (result $shape)
        (canon lift (core func $i "shape-of")
          (memory (core memory $libc "memory")) (realloc (core func $libc "cabi_realloc"))))
      (func (export "call-describe") (param "r" u32) (result $point)
        (canon lift (core func $i "call-describe")
          (memory (core memory $libc "memory")) (realloc (core func $libc "cabi_realloc"))))
      (func (export "echo-points") (param "ps" (list $point)) (result (list $point))
        (canon lift (core func $i "echo-pair")
          (memory (core memory $libc "memory")) (realloc (core func $libc "cabi_realloc"))))
      (func (export "count-points") (param "ps" (list $point)) (result u32)
        (canon lift (core func $i "count")
          (memory (core memory $libc "memory")) (realloc (core func $libc "cabi_realloc"))))
      (func (export "echo-renamed") (param "r" $renamed) (result $renamed)
        (canon lift (core func $i "echo-pair")
          (memory (core memory $libc "memory")) (realloc (core func $libc "cabi_realloc"))))
      (func (export "echo-one") (param "f" $one) (result $one)
        (canon lift (core func $i "echo-one")))
      (func (export "echo-eight") (param "f" $eight) (result $eight)
        (canon lift (core func $i "echo-one")))
      (func (export "echo-nine") (param "f" $nine) (result $nine)
        (canon lift (core func $i "echo-one")))
      (func (export "echo-thirty-two") (param "f" $thirty-two) (result $thirty-two)
        (canon lift (core func $i "echo-one")))
      (func (export "echo-permissions") (param "f" $permissions) (result $permissions)
        (canon lift (core func $i "echo-one")))
      (func (export "echo-shape") (param "s" $shape) (result $shape)
        (canon lift (core func $i "echo-pair")
          (memory (core memory $libc "memory")) (realloc (core func $libc "cabi_realloc"))))
      (func (export "echo-flagged") (param "f" $flagged) (result $flagged)
        (canon lift (core func $i "echo-one")
          (memory (core memory $libc "memory")) (realloc (core func $libc "cabi_realloc"))))
      (func (export "padded-one") (result $one) (canon lift (core func $i "padded-one")))
      (func (export "padded-pair") (result (tuple $one $nine))
        (canon lift (core func $i "padded-pair") (memory (core memory $libc "memory"))))
      (func (export "peek") (param "p" u32) (result u32) (canon lift (core func $libc "peek")))
      (func (export "reset") (canon lift (core func $libc "reset"))))
    "#
);

/// Instantiate [`DERIVED`] with a typed host function over derived
/// types for its import.
async fn derived() -> (Store<()>, Instance) {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, DERIVED)
        .await
        .expect("the component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let host: InterfaceIdentifier = HOST.parse().expect("identifier");
    linker
        .instance(&host)
        .func_wrap("describe", |_call: HostCall<'_, ()>, (shape,): (Shape,)| {
            Ok((match shape {
                Shape::Circle(r) => Point { x: r, y: r + 1 },
                Shape::Nothing => Point { x: 0, y: 0 },
            },))
        })
        .expect("the registration of `describe`");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the component links to the typed host function");
    (store, instance)
}

/// The typed handle of the export `name`.
fn typed<P, R>(instance: &Instance, name: &str) -> TypedFunc<P, R>
where
    P: wcmp::ComponentNamedList + Lower,
    R: wcmp::ComponentNamedList + Lift,
{
    instance
        .get_func(name)
        .expect("the export")
        .typed::<P, R>()
        .expect("the export has the typed signature")
}

/// The path of the type mismatch `error` names.
fn path(error: Error) -> Vec<TypePathStep> {
    match error {
        Error::TypeMismatch(mismatch) => match mismatch.position {
            TypeMismatchPosition::TypedConversion { path, .. }
            | TypeMismatchPosition::HostFunctionRegistration { path, .. } => path,
            other => panic!("expected a typed position, got {other:?}"),
        },
        other => panic!("expected a type mismatch, got {other:?}"),
    }
}

#[wcmp_macros::test]
async fn it_crosses_a_derived_record_and_variant_both_ways() {
    let (mut store, instance) = derived().await;
    // A typed export takes a derived record and returns a derived
    // variant.
    let (shape,) = typed::<(Point,), (Shape,)>(&instance, "shape-of")
        .call(&mut store, (Point { x: 3, y: 4 },))
        .await
        .expect("the call returns");
    assert_eq!(shape, Shape::Circle(7));

    // A typed host function takes a derived variant and returns a
    // derived record.
    let (point,) = typed::<(u32,), (Point,)>(&instance, "call-describe")
        .call(&mut store, (5,))
        .await
        .expect("the call returns");
    assert_eq!(point, Point { x: 5, y: 6 });
}

#[wcmp_macros::test]
async fn it_crosses_renamed_fields_and_cases_by_their_wit_names() {
    let (mut store, instance) = derived().await;
    for value in [
        Renamed {
            first: 1,
            level: Level::Info,
        },
        Renamed {
            first: 2,
            level: Level::Error,
        },
    ] {
        let (echoed,) = typed::<(Renamed,), (Renamed,)>(&instance, "echo-renamed")
            .call(&mut store, (value,))
            .await
            .expect("the call returns");
        assert_eq!(echoed, value);
    }
}

#[wcmp_macros::test]
async fn it_builds_no_val_for_a_list_of_derived_records() {
    // A record of two numbers points nowhere, so the list crosses in
    // one write and one read: nothing of the call grows with it but
    // the result list, which is one allocation at either length.
    let (mut store, instance) = derived().await;
    let echo = typed::<(Vec<Point>,), (Vec<Point>,)>(&instance, "echo-points");
    let reset = typed::<(), ()>(&instance, "reset");
    let points = |count: u32| {
        (0..count)
            .map(|x| Point { x, y: x * 2 })
            .collect::<Vec<_>>()
    };
    echo.call(&mut store, (points(4096),))
        .await
        .expect("the warm-up");
    reset.call(&mut store, ()).await.expect("the reset");
    let (small, output) = counted(echo.call(&mut store, (points(16),))).await;
    assert_eq!(output.expect("the call").0, points(16));
    reset.call(&mut store, ()).await.expect("the reset");
    let (large, output) = counted(echo.call(&mut store, (points(4096),))).await;
    assert_eq!(output.expect("the call").0, points(4096));
    assert_eq!(
        small, large,
        "{small} allocations for 16 records, {large} for 4096"
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_derived_record_whose_fields_differ_and_names_the_field() {
    let (_store, instance) = derived().await;
    let func = || instance.get_func("shape-of").expect("the export");
    let name = func()
        .typed::<(PointZ,), (Shape,)>()
        .expect_err("a field named `z` is not `y`");
    assert_eq!(
        path(name),
        [
            TypePathStep::Parameter(0),
            TypePathStep::Field("y".to_owned())
        ]
    );
    let wide = func()
        .typed::<(PointWide,), (Shape,)>()
        .expect_err("a `u64` field is not a `u32` one");
    assert_eq!(
        path(wide),
        [
            TypePathStep::Parameter(0),
            TypePathStep::Field("x".to_owned())
        ]
    );
    let extra = func()
        .typed::<(Point3,), (Shape,)>()
        .expect_err("a third field is one too many");
    assert_eq!(
        path(extra),
        [
            TypePathStep::Parameter(0),
            TypePathStep::Field("z".to_owned())
        ]
    );
}

/// A guest that imports a function over a record whose second field is
/// named `z`.
const CONSUMES_Z: &[u8] = component!(
    r#"
    (component
      (import "wcmp-tests:derive/consume@0.1.0" (instance $host
        (type $point' (record (field "x" u32) (field "z" u32)))
        (export "point" (type $point (eq $point')))
        (export "consume" (func (param "p" $point)))))
      (alias export $host "consume" (func $consume))
      (core func $consume-lower (canon lower (func $consume))))
    "#
);

#[wcmp_macros::test]
async fn it_refuses_to_link_a_host_function_over_a_record_that_differs() {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, CONSUMES_Z)
        .await
        .expect("the component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let consume: InterfaceIdentifier = "wcmp-tests:derive/consume@0.1.0"
        .parse()
        .expect("identifier");
    linker
        .instance(&consume)
        .func_wrap("consume", |_call: HostCall<'_, ()>, (_point,): (Point,)| {
            Ok(())
        })
        .expect("the registration");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let Err(error) = linker.instantiate(&mut store, &component).await else {
        panic!("a record with a field `z` is not `point`");
    };
    let Error::TypeMismatch(mismatch) = &error else {
        panic!("expected a type mismatch, got {error:?}");
    };
    assert_eq!(
        mismatch.position,
        TypeMismatchPosition::HostFunctionRegistration {
            interface: consume,
            item: "consume".to_owned(),
            path: vec![
                TypePathStep::Parameter(0),
                TypePathStep::Field("z".to_owned())
            ],
        },
        "{error}"
    );
}

#[wcmp_macros::test]
async fn it_crosses_flags_types_of_every_width() {
    let (mut store, instance) = derived().await;
    let (one,) = typed::<(One,), (One,)>(&instance, "echo-one")
        .call(&mut store, (One::A,))
        .await
        .expect("one flag crosses");
    assert_eq!(one, One::A);
    let (eight,) = typed::<(Eight,), (Eight,)>(&instance, "echo-eight")
        .call(&mut store, (Eight::A | Eight::H,))
        .await
        .expect("eight flags cross");
    assert_eq!(eight, Eight::A | Eight::H);
    let (nine,) = typed::<(Nine,), (Nine,)>(&instance, "echo-nine")
        .call(&mut store, (Nine::B | Nine::I,))
        .await
        .expect("nine flags cross");
    assert_eq!(nine, Nine::B | Nine::I);
    let (thirty_two,) = typed::<(ThirtyTwo,), (ThirtyTwo,)>(&instance, "echo-thirty-two")
        .call(&mut store, (ThirtyTwo::F0 | ThirtyTwo::F31,))
        .await
        .expect("thirty-two flags cross");
    assert_eq!(thirty_two, ThirtyTwo::F0 | ThirtyTwo::F31);
}

#[wcmp_macros::test]
fn it_lays_out_more_than_thirty_two_flags_in_words_of_thirty_two() {
    // The Component Model limits a flags type to 32 flags, so no
    // component declares 33 and none crosses. `flags!` takes them as
    // Wasmtime's does, in two words.
    let flags = ThirtyThree::F1 | ThirtyThree::F32;
    assert_eq!(flags.as_array(), [0b10, 0b1]);
    assert_eq!(<ThirtyThree as ComponentType>::SIZE32, 8);
    assert_eq!(<ThirtyThree as ComponentType>::ALIGN32, 4);
    assert!(flags.contains(ThirtyThree::F32));
    assert!(!flags.contains(ThirtyThree::F31));
}

#[wcmp_macros::test]
fn it_gives_a_flags_type_wasmtimes_api() {
    let none = Permissions::empty();
    let both = Permissions::all();
    assert_eq!(none, Permissions::default());
    assert_eq!(both, Permissions::READ | Permissions::WRITE);
    assert!(both.contains(Permissions::READ));
    assert!(!Permissions::READ.contains(both));
    assert!(both.intersects(Permissions::WRITE));
    assert!(!Permissions::READ.intersects(Permissions::WRITE));
    assert_eq!(both & Permissions::WRITE, Permissions::WRITE);
    assert_eq!(both ^ Permissions::WRITE, Permissions::READ);
    assert_eq!(
        !Permissions::READ,
        Permissions::WRITE,
        "padding bits take no part"
    );
    let mut flags = Permissions::READ;
    flags |= Permissions::WRITE;
    assert_eq!(flags, both);
    flags &= Permissions::WRITE;
    assert_eq!(flags, Permissions::WRITE);
    flags ^= Permissions::READ;
    assert_eq!(flags, both);
    assert_eq!(Permissions::WRITE.as_array(), [0b10]);
    assert_eq!(format!("{:?}", both), "(READ|WRITE)");
}

#[wcmp_macros::test]
async fn it_refuses_a_flags_type_whose_names_or_count_differ() {
    let (_store, instance) = derived().await;
    let func = || instance.get_func("echo-permissions").expect("the export");
    func()
        .typed::<(Permissions,), (Permissions,)>()
        .expect("the renamed flags match");
    let count = func()
        .typed::<(Nine,), (Permissions,)>()
        .expect_err("nine flags are not two");
    assert_eq!(path(count), [TypePathStep::Parameter(0)]);
    let names = func()
        .typed::<(Permissions,), (Eight,)>()
        .expect_err("flags named `A` to `H` are not `read` and `write`");
    assert_eq!(path(names), [TypePathStep::Result(0)]);
}

#[wcmp_macros::test]
async fn it_refuses_a_flags_type_whose_names_differ_at_the_same_count() {
    let (_store, instance) = derived().await;
    let error = instance
        .get_func("echo-permissions")
        .expect("the export")
        .typed::<(ReadExec,), (Permissions,)>()
        .expect_err("flags named `read` and `exec` are not `read` and `write`");
    let Error::TypeMismatch(mismatch) = &error else {
        panic!("expected a type mismatch, got {error:?}");
    };
    assert_eq!(
        mismatch.position,
        TypeMismatchPosition::TypedConversion {
            export: "echo-permissions".to_owned(),
            path: vec![TypePathStep::Parameter(0)],
        },
        "{error}"
    );
}

#[wcmp_macros::test]
async fn it_lowers_and_lifts_back_each_case_of_a_derived_variant() {
    // The variant lowers as the export's flat parameters and lifts
    // back out of the memory its result spills to.
    let (mut store, instance) = derived().await;
    let echo = typed::<(Shape,), (Shape,)>(&instance, "echo-shape");
    for shape in [Shape::Circle(9), Shape::Circle(u32::MAX), Shape::Nothing] {
        let (echoed,) = echo
            .call(&mut store, (shape,))
            .await
            .expect("the call returns");
        assert_eq!(echoed, shape);
    }
}

#[wcmp_macros::test]
async fn it_lowers_flags_of_every_width_into_memory_inside_a_spilled_record() {
    // The record's seventeen flat values spill, so the host writes it
    // into the guest's memory: at 16, the first address the guest's
    // allocator gives after `reset`. The guest returns that address,
    // and the host lifts the record back out of it.
    let (mut store, instance) = derived().await;
    let flagged = Flagged {
        perms: Permissions::WRITE,
        nine: Nine::A | Nine::I,
        wide: ThirtyTwo::F0 | ThirtyTwo::F31,
        rest: core::array::from_fn(|index| index as u32 + 1),
    };
    typed::<(), ()>(&instance, "reset")
        .call(&mut store, ())
        .await
        .expect("the reset");
    let (echoed,) = typed::<(Flagged,), (Flagged,)>(&instance, "echo-flagged")
        .call(&mut store, (flagged,))
        .await
        .expect("the call returns");
    assert_eq!(echoed, flagged);

    // The Canonical ABI's layout: the `u8` of `permissions` at 0, the
    // `u16` of `nine` at 2, the `u32` of `thirty-two` at 4, and the
    // list from 8. The byte at 1 is padding.
    let peek = typed::<(u32,), (u32,)>(&instance, "peek");
    let mut words = Vec::new();
    for address in [16, 20, 24, 76] {
        words.push(peek.call(&mut store, (address,)).await.expect("the peek").0);
    }
    assert_eq!(words[0] & 0xffff_00ff, 0x0101_0002);
    assert_eq!(words[1..], [0x8000_0001, 1, 14]);
}

#[wcmp_macros::test]
fn it_leaves_the_padding_bits_of_a_flags_type_clear() {
    assert_eq!(Three::all().as_array(), [0b111]);
    assert_eq!((!Three::empty()).as_array(), [0b111]);
    assert_eq!((!Three::A).as_array(), [0b110]);
    assert_eq!(Nine::all().as_array(), [0x1ff]);
}

#[wcmp_macros::test]
async fn it_drops_the_padding_bits_a_guest_sets_when_flags_lift() {
    let (mut store, instance) = derived().await;
    // Flat: a core `i32` of `0xff` for a type of one flag.
    let (one,) = typed::<(), (One,)>(&instance, "padded-one")
        .call(&mut store, ())
        .await
        .expect("the call returns");
    assert_eq!(one, One::all());
    assert_eq!(one.as_array(), [1]);
    // In memory: a byte of `0xff` for one flag and a `u16` of `0xffff`
    // for nine.
    let ((one, nine),) = typed::<(), ((One, Nine),)>(&instance, "padded-pair")
        .call(&mut store, ())
        .await
        .expect("the call returns");
    assert_eq!(one.as_array(), [1]);
    assert_eq!(nine, Nine::all());
    assert_eq!(nine.as_array(), [0x1ff]);
}

#[path = "support/backend.rs"]
mod test_backend;
