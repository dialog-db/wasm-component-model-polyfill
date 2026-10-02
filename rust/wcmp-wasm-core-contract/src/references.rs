// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! References: those the host reads, and those it only passes through.

use wcmp_macros::wasm;
use wcmp_wasm_core::{
    AnyRef, Capability, Engine, Error, ExternRef, FuncType, HeapType, I31, Val, ValType,
};

use crate::support;

/// The host makes an `externref`, a guest hands it back, and the host
/// reads the value it made. A null `externref` comes back null.
pub async fn it_reads_an_externref_the_guest_hands_back(engine: &Engine) {
    let mut store = support::store(engine, ());
    let instance = support::instance(
        &mut store,
        wasm!(
            r#"
            (module
              (func (export "echo") (param externref) (result externref)
                local.get 0))
            "#
        ),
        &[],
    )
    .await;
    let echo = support::func(&mut store, instance, "echo");

    let made =
        ExternRef::new(&mut store, String::from("payload")).expect("the store makes an externref");
    let echoed = support::call(&mut store, echo, &[made.into()], &[ValType::EXTERNREF]);
    let Val::ExternRef(Some(echoed)) = echoed[0] else {
        panic!("the guest hands back an externref: {echoed:?}");
    };
    let value = echoed
        .data(&store)
        .expect("the externref belongs to the store");
    assert_eq!(
        value.downcast_ref::<String>().map(String::as_str),
        Some("payload")
    );

    let null = support::call(
        &mut store,
        echo,
        &[Val::ExternRef(None)],
        &[ValType::EXTERNREF],
    );
    assert!(matches!(null[0], Val::ExternRef(None)), "{null:?}");
}

/// A guest hands out a `funcref`. The host reads its type and calls it.
pub async fn it_calls_a_funcref_the_guest_hands_out(engine: &Engine) {
    let mut store = support::store(engine, ());
    let instance = support::instance(
        &mut store,
        wasm!(
            r#"
            (module
              (func $triple (param i32) (result i32)
                local.get 0
                i32.const 3
                i32.mul)
              (elem declare func $triple)
              (func (export "get") (result funcref)
                ref.func $triple))
            "#
        ),
        &[],
    )
    .await;
    let get = support::func(&mut store, instance, "get");

    let handed = support::call(&mut store, get, &[], &[ValType::FUNCREF]);
    let Val::FuncRef(Some(triple)) = handed[0] else {
        panic!("the guest hands out a funcref: {handed:?}");
    };
    if let Some(ty) = triple.ty(&store).expect("the funcref belongs to the store") {
        assert_eq!(ty, FuncType::new([ValType::I32], [ValType::I32]));
    }
    let tripled = support::call(&mut store, triple, &[Val::I32(3)], &[ValType::I32]);
    assert_eq!(tripled[0].i32(), Some(9));
}

/// A guest hands out an `i31ref`, and the host reads its integer. The host
/// makes an `i31ref`, and a guest reads its integer. A backend that lacks
/// `gc` refuses the module, and the host's `i31ref`, with `Unsupported`
/// and `gc`.
pub async fn it_reads_an_i31ref(engine: &Engine) {
    let bytes = wasm!(
        r#"
        (module
          (func (export "make") (param i32) (result i31ref)
            local.get 0
            ref.i31)
          (func (export "read") (param i31ref) (result i32)
            local.get 0
            i31.get_s))
        "#
    );
    if support::refuses(engine, &[Capability::Gc], &[bytes]).await {
        let mut store = support::store(engine, ());
        let host_made = AnyRef::from_i31(&mut store, I31::wrapping_i32(-7));
        assert!(
            matches!(host_made, Err(Error::Unsupported(Capability::Gc))),
            "{host_made:?}"
        );
        return;
    }
    let mut store = support::store(engine, ());
    let instance = support::instance(&mut store, bytes, &[]).await;
    let make = support::func(&mut store, instance, "make");
    let read = support::func(&mut store, instance, "read");

    let made = support::call(&mut store, make, &[Val::I32(-5)], &[ValType::I31REF]);
    let Val::AnyRef(Some(made)) = made[0] else {
        panic!("the guest hands out an i31ref: {made:?}");
    };
    let value = made
        .as_i31(&store)
        .expect("the reference belongs to the store")
        .expect("the reference is an i31ref");
    assert_eq!(value.get_i32(), -5);

    let host_made =
        AnyRef::from_i31(&mut store, I31::wrapping_i32(-7)).expect("the store makes an i31ref");
    let read_back = support::call(&mut store, read, &[host_made.into()], &[ValType::I32]);
    assert_eq!(read_back[0].i32(), Some(-7));
}

/// A guest hands out a GC struct. The host holds it, tests it for null,
/// and gives it back to a guest of the same store, which reads it. The
/// host reads the struct's concrete type as one handle at both ends. A
/// backend that lacks `gc` refuses the module with `Unsupported` and `gc`.
pub async fn it_passes_a_gc_object_back_to_its_guest(engine: &Engine) {
    let bytes = wasm!(
        r#"
        (module
          (type $pair (struct (field i32) (field i32)))
          (func (export "make") (param i32 i32) (result (ref null $pair))
            local.get 0
            local.get 1
            struct.new $pair)
          (func (export "nothing") (result anyref)
            ref.null any)
          (func (export "sum") (param (ref null $pair)) (result i32)
            local.get 0
            struct.get $pair 0
            local.get 0
            struct.get $pair 1
            i32.add))
        "#
    );
    if support::refuses(engine, &[Capability::Gc], &[bytes]).await {
        return;
    }
    let mut store = support::store(engine, ());
    let instance = support::instance(&mut store, bytes, &[]).await;
    let make = support::func(&mut store, instance, "make");
    let nothing = support::func(&mut store, instance, "nothing");
    let sum = support::func(&mut store, instance, "sum");

    let made_ty = make
        .ty(&store)
        .expect("the function belongs to the store")
        .expect("the engine knows the type of an export");
    let summed_ty = sum
        .ty(&store)
        .expect("the function belongs to the store")
        .expect("the engine knows the type of an export");
    let (Some(ValType::Ref(made)), Some(ValType::Ref(summed))) =
        (made_ty.results().first(), summed_ty.params().first())
    else {
        panic!("both functions name the struct type: {made_ty:?} {summed_ty:?}");
    };
    assert!(matches!(made.heap, HeapType::Concrete(_)), "{made:?}");
    assert_eq!(made.heap, summed.heap, "one type is one handle");

    let pair = support::call(
        &mut store,
        make,
        &[Val::I32(2), Val::I32(5)],
        &[ValType::Ref(*made)],
    );
    assert!(!pair[0].is_null(), "the struct is not null");
    assert!(matches!(pair[0], Val::AnyRef(Some(_))), "{pair:?}");
    let total = support::call(&mut store, sum, &pair, &[ValType::I32]);
    assert_eq!(total[0].i32(), Some(7));

    let none = support::call(&mut store, nothing, &[], &[ValType::ANYREF]);
    assert!(matches!(none[0], Val::AnyRef(None)), "{none:?}");
}

/// A guest catches an exception and hands out its `exnref`. The host holds
/// it, tests it for null, and gives it back to the guest, which rethrows
/// it and reads the payload. A backend that lacks `exceptions` refuses the
/// module with `Unsupported` and `exceptions`.
pub async fn it_passes_an_exnref_back_to_its_guest(engine: &Engine) {
    let bytes = wasm!(
        r#"
        (module
          (tag $oops (param i32))
          (func (export "catch") (param i32) (result exnref)
            block $caught (result exnref)
              try_table (catch_all_ref $caught)
                local.get 0
                throw $oops
              end
              unreachable
            end)
          (func (export "nothing") (result exnref)
            ref.null exn)
          (func (export "payload") (param exnref) (result i32)
            block $caught (result i32)
              try_table (catch $oops $caught)
                local.get 0
                throw_ref
              end
              unreachable
            end))
        "#
    );
    if support::refuses(engine, &[Capability::Exceptions], &[bytes]).await {
        return;
    }
    let mut store = support::store(engine, ());
    let instance = support::instance(&mut store, bytes, &[]).await;
    let catch = support::func(&mut store, instance, "catch");
    let nothing = support::func(&mut store, instance, "nothing");
    let payload = support::func(&mut store, instance, "payload");

    let caught = support::call(&mut store, catch, &[Val::I32(9)], &[ValType::EXNREF]);
    assert!(matches!(caught[0], Val::ExnRef(Some(_))), "{caught:?}");
    let read = support::call(&mut store, payload, &caught, &[ValType::I32]);
    assert_eq!(read[0].i32(), Some(9));

    let none = support::call(&mut store, nothing, &[], &[ValType::EXNREF]);
    assert!(none[0].is_null(), "{none:?}");
}

/// The module of the test of a collection: a struct and its sum, an echo
/// of an `externref`, and `churn`, which allocates `count` arrays of
/// `size` bytes and keeps none of them.
const CHURNS: &[u8] = wasm!(
    r#"
    (module
      (type $pair (struct (field i32) (field i32)))
      (type $bytes (array (mut i8)))
      (func (export "make") (param i32 i32) (result (ref null $pair))
        local.get 0
        local.get 1
        struct.new $pair)
      (func (export "sum") (param (ref null $pair)) (result i32)
        local.get 0
        struct.get $pair 0
        local.get 0
        struct.get $pair 1
        i32.add)
      (func (export "echo") (param externref) (result externref)
        local.get 0)
      (func (export "churn") (param $count i32) (param $size i32)
        block $done
          loop $again
            local.get $count
            i32.eqz
            br_if $done
            local.get $size
            array.new_default $bytes
            drop
            local.get $count
            i32.const 1
            i32.sub
            local.set $count
            br $again
          end
        end))
    "#
);

/// The host holds a GC struct a guest made, an `externref` it made itself,
/// and an `externref` a guest handed back, and nothing else holds any of
/// them. A guest then allocates far more than any engine keeps before it
/// collects. Each reference survives the collection: the host reads its
/// `externref`s, and a guest reads the struct. A backend that lacks `gc`
/// refuses the module with `Unsupported` and `gc`.
pub async fn it_keeps_the_references_the_host_holds_across_a_collection(engine: &Engine) {
    if support::refuses(engine, &[Capability::Gc], &[CHURNS]).await {
        return;
    }
    let mut store = support::store(engine, ());
    let instance = support::instance(&mut store, CHURNS, &[]).await;
    let make = support::func(&mut store, instance, "make");
    let sum = support::func(&mut store, instance, "sum");
    let echo = support::func(&mut store, instance, "echo");
    let churn = support::func(&mut store, instance, "churn");

    let pair = support::call(
        &mut store,
        make,
        &[Val::I32(20), Val::I32(22)],
        &[ValType::ANYREF],
    );
    let made =
        ExternRef::new(&mut store, String::from("made")).expect("the store makes an externref");
    let handed =
        ExternRef::new(&mut store, String::from("handed")).expect("the store makes an externref");
    let handed = support::call(&mut store, echo, &[handed.into()], &[ValType::EXTERNREF]);
    let Val::ExternRef(Some(handed)) = handed[0] else {
        panic!("the guest hands back an externref: {handed:?}");
    };

    // 16,384 arrays of 4 KiB: 64 MiB, all of it garbage once it is made.
    support::call(&mut store, churn, &[Val::I32(16_384), Val::I32(4_096)], &[]);

    for (extern_ref, expected) in [(made, "made"), (handed, "handed")] {
        let value = extern_ref
            .data(&store)
            .expect("the externref belongs to the store");
        assert_eq!(
            value.downcast_ref::<String>().map(String::as_str),
            Some(expected),
            "the host reads its externref after the collection"
        );
        let echoed = support::call(
            &mut store,
            echo,
            &[extern_ref.into()],
            &[ValType::EXTERNREF],
        );
        let Val::ExternRef(Some(echoed)) = echoed[0] else {
            panic!("the guest hands back an externref: {echoed:?}");
        };
        let value = echoed
            .data(&store)
            .expect("the externref belongs to the store");
        assert_eq!(
            value.downcast_ref::<String>().map(String::as_str),
            Some(expected),
            "a guest passes the externref on after the collection"
        );
    }
    let total = support::call(&mut store, sum, &pair, &[ValType::I32]);
    assert_eq!(
        total[0].i32(),
        Some(42),
        "a guest reads the struct after the collection"
    );
}
