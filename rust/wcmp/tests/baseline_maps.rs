//! Baseline tests for `map<K, V>` values: the type projection, the
//! canonical ABI in both directions, and the typed Rust conversion.
//! A map is laid out as the list of its key-value tuples, so the
//! guests below handle a map as they would handle that list.

#![cfg(test)]

use std::collections::HashMap;

use wcmp::{
    AbiCause, Component, Engine, Error, ExternType, Linker, MapType, PrimitiveType, Store, Val,
    ValueType,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// A guest that sums a map's values (`sum`), returns a map unchanged
/// (`identity`), and hands a map to the host (`forward`).
const COMPONENT: &[u8] = component!(
    r#"
    (component
      (import "count" (func $count (param "m" (map string u32)) (result u32)))
      (core module $libc
        (memory (export "memory") 1)
        (global $bump (mut i32) (i32.const 16))
        (func (export "cabi_realloc")
              (param $old i32) (param $old-size i32) (param $align i32) (param $size i32)
              (result i32)
          (local $ptr i32)
          (local.set $ptr
            (i32.and
              (i32.add (global.get $bump) (i32.sub (local.get $align) (i32.const 1)))
              (i32.sub (i32.const 0) (local.get $align))))
          (global.set $bump (i32.add (local.get $ptr) (local.get $size)))
          (local.get $ptr)))
      (core instance $libc (instantiate $libc))
      (core func $count-lower
        (canon lower (func $count) (memory (core memory $libc "memory"))))
      (core module $m
        (import "" "memory" (memory 1))
        (import "" "realloc" (func $realloc (param i32 i32 i32 i32) (result i32)))
        (import "" "count" (func $count (param i32 i32) (result i32)))
        (func (export "sum") (param $ptr i32) (param $len i32) (result i32)
          (local $i i32)
          (local $acc i32)
          (block $done
            (loop $next
              (br_if $done (i32.ge_u (local.get $i) (local.get $len)))
              (local.set $acc
                (i32.add
                  (local.get $acc)
                  (i32.load offset=8
                    (i32.add (local.get $ptr) (i32.mul (local.get $i) (i32.const 12))))))
              (local.set $i (i32.add (local.get $i) (i32.const 1)))
              (br $next)))
          (local.get $acc))
        (func (export "forward") (param $ptr i32) (param $len i32) (result i32)
          local.get $ptr local.get $len call $count)
        (func (export "identity") (param $ptr i32) (param $len i32) (result i32)
          (local $ret i32)
          (local.set $ret
            (call $realloc (i32.const 0) (i32.const 0) (i32.const 4) (i32.const 8)))
          (i32.store (local.get $ret) (local.get $ptr))
          (i32.store offset=4 (local.get $ret) (local.get $len))
          (local.get $ret)))
      (core instance $i (instantiate $m
        (with "" (instance
          (export "memory" (memory $libc "memory"))
          (export "realloc" (func $libc "cabi_realloc"))
          (export "count" (func $count-lower))))))
      (func (export "sum") (param "m" (map string u32)) (result u32)
        (canon lift (core func $i "sum")
          (memory (core memory $libc "memory"))
          (realloc (core func $libc "cabi_realloc"))))
      (func (export "forward") (param "m" (map string u32)) (result u32)
        (canon lift (core func $i "forward")
          (memory (core memory $libc "memory"))
          (realloc (core func $libc "cabi_realloc"))))
      (func (export "identity") (param "m" (map string u32)) (result (map string u32))
        (canon lift (core func $i "identity")
          (memory (core memory $libc "memory"))
          (realloc (core func $libc "cabi_realloc")))))
    "#
);

fn string_to_u32() -> ValueType {
    ValueType::Map(MapType::new(
        ValueType::Primitive(PrimitiveType::String),
        ValueType::Primitive(PrimitiveType::U32),
    ))
}

fn entries(pairs: &[(&str, u32)]) -> Val {
    Val::Map(
        pairs
            .iter()
            .map(|(key, value)| (Val::String((*key).to_owned()), Val::U32(*value)))
            .collect(),
    )
}

async fn instantiate() -> (Store<()>, wcmp::Instance) {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("a component with map types parses");

    // The projection describes the map by its key and value types.
    let sum = component
        .exports
        .iter()
        .find(|export| export.name.to_string() == "sum")
        .expect("`sum` is exported");
    let ExternType::Function(signature) = &sum.ty else {
        panic!("expected a function, got {:?}", sum.ty);
    };
    assert_eq!(signature.parameters[0].ty, string_to_u32());
    let count = &component.imports[0];
    let ExternType::Function(signature) = &count.ty else {
        panic!("expected a function import, got {:?}", count.ty);
    };
    assert_eq!(signature.parameters[0].ty, string_to_u32());

    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap("count", |_, (map,): (HashMap<String, u32>,)| {
            Ok(map.len() as u32)
        })
        .expect("the registration");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiation succeeds");
    (store, instance)
}

#[wcmp_macros::test]
async fn it_round_trips_a_map_through_an_export_untyped() {
    let (mut store, instance) = instantiate().await;
    let sum = instance.get_func("sum").expect("`sum` is exported");
    let total = sum
        .call(&mut store, &[entries(&[("a", 1), ("b", 2), ("c", 39)])])
        .await
        .expect("the map lowers into the guest");
    assert_eq!(total.as_ref(), &[Val::U32(42)]);

    let identity = instance
        .get_func("identity")
        .expect("`identity` is exported");
    let back = identity
        .call(&mut store, &[entries(&[("x", 7), ("y", 8)])])
        .await
        .expect("the map lifts out of the guest");
    assert_eq!(back.as_ref(), &[entries(&[("x", 7), ("y", 8)])]);
    let empty = identity
        .call(&mut store, &[entries(&[])])
        .await
        .expect("an empty map round-trips");
    assert_eq!(empty.as_ref(), &[entries(&[])]);
}

#[wcmp_macros::test]
async fn it_round_trips_a_map_through_an_export_typed() {
    let (mut store, instance) = instantiate().await;
    let sum = instance
        .get_func("sum")
        .expect("`sum` is exported")
        .typed::<(HashMap<String, u32>,), u32>()
        .expect("a Rust hash map converts to `map<string, u32>`");
    let map: HashMap<String, u32> = [("a".to_owned(), 1), ("b".to_owned(), 41)]
        .into_iter()
        .collect();
    assert_eq!(
        sum.call(&mut store, (map.clone(),)).await.expect("call"),
        42
    );

    let identity = instance
        .get_func("identity")
        .expect("`identity` is exported")
        .typed::<(HashMap<String, u32>,), HashMap<String, u32>>()
        .expect("typed conversion succeeds");
    assert_eq!(
        identity
            .call(&mut store, (map.clone(),))
            .await
            .expect("call"),
        map
    );
}

#[wcmp_macros::test]
async fn it_passes_a_map_to_a_host_function() {
    let (mut store, instance) = instantiate().await;
    let forward = instance.get_func("forward").expect("`forward` is exported");
    let count = forward
        .call(&mut store, &[entries(&[("a", 1), ("b", 2), ("a", 3)])])
        .await
        .expect("the guest hands the map to the host");
    // The host's hash map keeps the last value for the repeated key.
    assert_eq!(count.as_ref(), &[Val::U32(2)]);
}

#[wcmp_macros::test]
async fn it_reports_a_value_or_type_that_is_not_a_map() {
    let (mut store, instance) = instantiate().await;
    let sum = instance.get_func("sum").expect("`sum` is exported");

    // A list is not a map at the untyped boundary, even though the
    // layouts agree.
    let err = sum
        .call(
            &mut store,
            &[Val::List(Box::new([Val::Tuple(Box::new([
                Val::String("a".to_owned()),
                Val::U32(1),
            ]))]))],
        )
        .await
        .expect_err("a list does not lower into a map parameter");
    match err {
        Error::Abi(inner) => assert!(matches!(inner.cause, AbiCause::HostValueMismatch)),
        other => panic!("expected an ABI error, got {other:?}"),
    }

    // A typed handle whose Rust type is a list does not convert.
    let err = instance
        .get_func("sum")
        .expect("`sum` is exported")
        .typed::<(Vec<String>,), u32>()
        .expect_err("a list type does not match a map type");
    assert!(matches!(err, Error::TypeMismatch(_)));
}

#[path = "support/backend.rs"]
mod test_backend;
