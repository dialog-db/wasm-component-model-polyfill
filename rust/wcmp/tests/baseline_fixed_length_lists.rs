//! Baseline tests for fixed-length `list<T, N>` values: the type
//! projection, the canonical ABI in both directions, and the typed
//! Rust conversion. A fixed-length list is laid out inline, so a
//! guest sees its elements as it would see a tuple of them.

#![cfg(test)]

use wcmp::{
    AbiCause, Component, Engine, Error, ExternType, FixedLengthListType, Linker, PrimitiveType,
    Store, Val, ValueType,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// A guest that sums a `list<u32, 4>` (`sum`), returns a `list<u8,
/// 16>` unchanged (`identity`), and hands a `list<u32, 4>` to the
/// host (`forward`).
const COMPONENT: &[u8] = component!(
    r#"
    (component
      (import "check" (func $check (param "l" (list u32 4)) (result u32)))
      (core func $check-lower (canon lower (func $check)))
      (core module $m
        (import "" "check" (func $check (param i32 i32 i32 i32) (result i32)))
        (memory (export "memory") 1)
        (func (export "sum") (param i32 i32 i32 i32) (result i32)
          (i32.add
            (i32.add (local.get 0) (local.get 1))
            (i32.add (local.get 2) (local.get 3))))
        (func (export "forward") (param i32 i32 i32 i32) (result i32)
          local.get 0 local.get 1 local.get 2 local.get 3 call $check)
        (func (export "identity")
              (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32)
              (result i32)
          (i32.store8 offset=0 (i32.const 32) (local.get 0))
          (i32.store8 offset=1 (i32.const 32) (local.get 1))
          (i32.store8 offset=2 (i32.const 32) (local.get 2))
          (i32.store8 offset=3 (i32.const 32) (local.get 3))
          (i32.store8 offset=4 (i32.const 32) (local.get 4))
          (i32.store8 offset=5 (i32.const 32) (local.get 5))
          (i32.store8 offset=6 (i32.const 32) (local.get 6))
          (i32.store8 offset=7 (i32.const 32) (local.get 7))
          (i32.store8 offset=8 (i32.const 32) (local.get 8))
          (i32.store8 offset=9 (i32.const 32) (local.get 9))
          (i32.store8 offset=10 (i32.const 32) (local.get 10))
          (i32.store8 offset=11 (i32.const 32) (local.get 11))
          (i32.store8 offset=12 (i32.const 32) (local.get 12))
          (i32.store8 offset=13 (i32.const 32) (local.get 13))
          (i32.store8 offset=14 (i32.const 32) (local.get 14))
          (i32.store8 offset=15 (i32.const 32) (local.get 15))
          (i32.const 32)))
      (core instance $i (instantiate $m
        (with "" (instance (export "check" (func $check-lower))))))
      (func (export "sum") (param "l" (list u32 4)) (result u32)
        (canon lift (core func $i "sum")))
      (func (export "forward") (param "l" (list u32 4)) (result u32)
        (canon lift (core func $i "forward")))
      (func (export "identity") (param "l" (list u8 16)) (result (list u8 16))
        (canon lift (core func $i "identity") (memory (core memory $i "memory")))))
    "#
);

fn fixed(values: &[u32]) -> Val {
    Val::FixedLengthList(values.iter().map(|v| Val::U32(*v)).collect())
}

fn bytes(values: &[u8]) -> Val {
    Val::FixedLengthList(values.iter().map(|v| Val::U8(*v)).collect())
}

async fn instantiate() -> (Store<()>, wcmp::Instance) {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("a component with fixed-length list types parses");

    // The projection carries the element type and the length.
    let sum = component
        .exports
        .iter()
        .find(|export| export.name.to_string() == "sum")
        .expect("`sum` is exported");
    let ExternType::Function(signature) = &sum.ty else {
        panic!("expected a function, got {:?}", sum.ty);
    };
    assert_eq!(
        signature.parameters[0].ty,
        ValueType::FixedLengthList(FixedLengthListType::new(
            ValueType::Primitive(PrimitiveType::U32),
            4
        ))
    );

    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap("check", |_, (list,): ([u32; 4],)| {
            Ok(list.iter().sum::<u32>())
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
async fn it_round_trips_a_fixed_length_list_untyped() {
    let (mut store, instance) = instantiate().await;
    let sum = instance.get_func("sum").expect("`sum` is exported");
    let total = sum
        .call(&mut store, &[fixed(&[1, 2, 3, 36])])
        .await
        .expect("four flat elements lower");
    assert_eq!(total.as_ref(), &[Val::U32(42)]);

    let identity = instance
        .get_func("identity")
        .expect("`identity` is exported");
    let input: Vec<u8> = (0..16).map(|i| i * 13).collect();
    let back = identity
        .call(&mut store, &[bytes(&input)])
        .await
        .expect("sixteen bytes lift out of memory");
    assert_eq!(back.as_ref(), &[bytes(&input)]);
}

#[wcmp_macros::test]
async fn it_round_trips_a_fixed_length_list_typed() {
    let (mut store, instance) = instantiate().await;
    let sum = instance
        .get_func("sum")
        .expect("`sum` is exported")
        .typed::<([u32; 4],), u32>()
        .expect("an array converts to `list<u32, 4>`");
    assert_eq!(
        sum.call(&mut store, ([1, 2, 3, 36],)).await.expect("call"),
        42
    );

    let identity = instance
        .get_func("identity")
        .expect("`identity` is exported")
        .typed::<([u8; 16],), [u8; 16]>()
        .expect("typed conversion succeeds");
    let input: [u8; 16] = core::array::from_fn(|i| (i * 13) as u8);
    assert_eq!(
        identity.call(&mut store, (input,)).await.expect("call"),
        input
    );
}

#[wcmp_macros::test]
async fn it_passes_a_fixed_length_list_to_a_host_function() {
    let (mut store, instance) = instantiate().await;
    let forward = instance.get_func("forward").expect("`forward` is exported");
    let total = forward
        .call(&mut store, &[fixed(&[10, 20, 30, 40])])
        .await
        .expect("the guest hands the list to the host");
    assert_eq!(total.as_ref(), &[Val::U32(100)]);
}

#[wcmp_macros::test]
async fn it_rejects_a_wrong_length() {
    let (mut store, instance) = instantiate().await;
    let sum = instance.get_func("sum").expect("`sum` is exported");

    // Three elements do not lower into `list<u32, 4>`.
    let err = sum
        .call(&mut store, &[fixed(&[1, 2, 3])])
        .await
        .expect_err("a wrong length is refused");
    match err {
        Error::Abi(inner) => assert!(matches!(inner.cause, AbiCause::HostValueMismatch)),
        other => panic!("expected an ABI error, got {other:?}"),
    }

    // A variable-length list is not a fixed-length one. The refused
    // lower above is a trap, and a trap poisons the store, so this one
    // runs in a store of its own.
    let (mut store, instance) = instantiate().await;
    let sum = instance.get_func("sum").expect("`sum` is exported");
    let err = sum
        .call(
            &mut store,
            &[Val::List(Box::new([
                Val::U32(1),
                Val::U32(2),
                Val::U32(3),
                Val::U32(4),
            ]))],
        )
        .await
        .expect_err("a list value does not lower into a fixed-length list");
    assert!(matches!(err, Error::Abi(_)));

    // The typed conversion checks the length as part of the type.
    let err = instance
        .get_func("sum")
        .expect("`sum` is exported")
        .typed::<([u32; 3],), u32>()
        .expect_err("`[u32; 3]` does not match `list<u32, 4>`");
    assert!(matches!(err, Error::TypeMismatch(_)));
}
