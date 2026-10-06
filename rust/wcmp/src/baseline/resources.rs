// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Resource-handle tests: depth coverage for the host-resource
//! surface PDD009 introduced. These tests probe the canonical-ABI
//! runtime-state rules from multiple angles — borrow lifetime,
//! per-store isolation, type-id discipline, and stale-handle
//! errors — and stake out parity stubs for capabilities the
//! polyfill does not yet realise (locally-defined resources,
//! multi-level resource sharing, host-side handle minting from
//! inside a host trampoline, the [constructor]/[method] dispatch
//! shape).

#![cfg(test)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::internal::{ResourceHandleInternal, ResourceTypeIdInternal};
use crate::resource::ResourceHandleParts;
use crate::store::{StoreContextInternalExt, StoreInternalExt};
use crate::{
    AbiCause, AbiPosition, Component, Engine, Error, FunctionParameter, FunctionType, HostCall,
    HostResource, InterfaceIdentifier, Linker, ResourceHandle, ResourceType, ResourceTypeId, Store,
    Val, ValueType,
};
use wcmp_macros::component;

#[wcmp_macros::test]
async fn it_runs_destructors_in_drop_order_for_multiple_handles() {
    // The component imports a single resource type and exposes a
    // function that drops every handle in the input list. The host
    // observes destructor invocations in order.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (import "pdd009-tests:host/resources@0.1.0" (instance $i
            (export "thing" (type (sub resource)))))
          (alias export $i "thing" (type $thing))
          (core func $thing-drop (canon resource.drop $thing))
          (core module $m
            (func (import "host" "drop") (param i32))
            (func (export "drop2") (param i32 i32)
              local.get 0 call 0
              local.get 1 call 0))
          (core instance $core (instantiate $m
            (with "host" (instance
              (export "drop" (func $thing-drop))))))
          (func (export "drop2") (param "a" (own $thing)) (param "b" (own $thing))
            (canon lift (core func $core "drop2"))))
        "#
    );

    let dropped: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));
    let log = dropped.clone();
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let mut linker: Linker<Arc<Mutex<Vec<u32>>>> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd009-tests:host/resources@0.1.0"
        .parse()
        .expect("identifier");
    let type_id = linker
        .instance(&iface)
        .resource(
            "thing",
            |data: &mut Arc<Mutex<Vec<u32>>>, rep: u32| -> crate::Result<()> {
                data.lock().expect("dropped lock").push(rep);
                Ok(())
            },
        )
        .expect("the registration");

    let mut store: Store<Arc<Mutex<Vec<u32>>>> = Store::new(&engine, log).expect("store");
    let inst = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let drop2 = inst.get_func("drop2").expect("drop2 export");
    let h1 = store.resource_new(type_id, 1).expect("mint h1");
    let h2 = store.resource_new(type_id, 2).expect("mint h2");
    drop2
        .call(&mut store, &[Val::Own(h1), Val::Own(h2)])
        .await
        .expect("call drop2");
    assert_eq!(*dropped.lock().expect("read"), vec![1, 2]);
}

#[wcmp_macros::test]
async fn it_isolates_handle_tables_across_stores_with_the_same_engine() {
    // Two stores share the engine and linker but mint handles in
    // their own tables. Index assignment is independent.
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let linker: Linker<()> = Linker::new(&engine);
    let _ = linker; // unused but proves shareability of the linker
    let store_a: Store<()> = Store::new(&engine, ()).expect("store a");
    let store_b: Store<()> = Store::new(&engine, ()).expect("store b");
    let type_id = ResourceTypeId::fresh();
    let a0 = store_a.resource_new(type_id, 1).expect("a0");
    let a1 = store_a.resource_new(type_id, 2).expect("a1");
    let b0 = store_b.resource_new(type_id, 100).expect("b0");
    // Both stores allocate from index 0; the tables are
    // store-local.
    assert_eq!(a0.index(), 1, "index 0 is reserved");
    assert_eq!(a1.index(), 2);
    assert_eq!(b0.index(), 1);
}

#[wcmp_macros::test]
async fn it_rejects_a_handle_whose_type_id_is_not_registered_in_the_store() {
    // A handle minted under one store's table cannot be lowered
    // into another store's call: the lower path's `validate_handle`
    // walks the destination store's tables and finds the
    // corresponding entry missing.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (import "pdd009-tests:host/resources@0.1.0" (instance $i
            (export "thing" (type (sub resource)))))
          (alias export $i "thing" (type $thing))
          (core func $thing-drop (canon resource.drop $thing))
          (core module $m
            (func (import "host" "drop") (param i32))
            (func (export "consume") (param i32) local.get 0 call 0))
          (core instance $core (instantiate $m
            (with "host" (instance
              (export "drop" (func $thing-drop))))))
          (func (export "consume") (param "h" (own $thing))
            (canon lift (core func $core "consume"))))
        "#
    );
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd009-tests:host/resources@0.1.0"
        .parse()
        .expect("identifier");
    let type_id = linker
        .instance(&iface)
        .resource("thing", |_data: &mut (), _rep: u32| -> crate::Result<()> {
            Ok(())
        })
        .expect("the registration");

    let mut consumer: Store<()> = Store::new(&engine, ()).expect("consumer store");
    let producer: Store<()> = Store::new(&engine, ()).expect("producer store");
    let foreign_handle = producer.resource_new(type_id, 99).expect("mint foreign");
    let inst = linker
        .instantiate(&mut consumer, &component)
        .await
        .expect("instantiate");
    let consume = inst.get_func("consume").expect("consume export");
    let outcome = consume
        .call(&mut consumer, &[Val::Own(foreign_handle)])
        .await;
    assert!(
        matches!(outcome, Err(Error::Abi(_))),
        "expected Error::Abi for cross-store handle, got {outcome:?}"
    );
}

#[wcmp_macros::test]
async fn it_rejects_a_completely_fabricated_handle_index() {
    // A `ResourceHandle` constructed with an index that was never
    // allocated trips `AbiCause::InvalidHandle` on lower.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (import "pdd009-tests:host/resources@0.1.0" (instance $i
            (export "thing" (type (sub resource)))))
          (alias export $i "thing" (type $thing))
          (core func $thing-drop (canon resource.drop $thing))
          (core module $m
            (func (import "host" "drop") (param i32))
            (func (export "consume") (param i32) local.get 0 call 0))
          (core instance $core (instantiate $m
            (with "host" (instance
              (export "drop" (func $thing-drop))))))
          (func (export "consume") (param "h" (own $thing))
            (canon lift (core func $core "consume"))))
        "#
    );
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd009-tests:host/resources@0.1.0"
        .parse()
        .expect("identifier");
    let type_id = linker
        .instance(&iface)
        .resource("thing", |_data: &mut (), _rep: u32| -> crate::Result<()> {
            Ok(())
        })
        .expect("the registration");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let inst = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let consume = inst.get_func("consume").expect("consume export");
    let bogus: ResourceHandle = ResourceHandleParts {
        type_id,
        index: 999,
        rep: 0,
        generation: 0,
    }
    .into();
    let outcome = consume.call(&mut store, &[Val::Own(bogus)]).await;
    assert!(matches!(outcome, Err(Error::Abi(_))));
}

#[wcmp_macros::test]
async fn it_supports_two_distinct_resource_types_in_one_interface() {
    // Two independently-registered resources share a single
    // imported instance. Handles minted under each remain
    // structurally distinct, and dropping one does not perturb the
    // other.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (import "pdd009-tests:host/multi@0.1.0" (instance $i
            (export "alpha" (type (sub resource)))
            (export "beta" (type (sub resource)))))
          (alias export $i "alpha" (type $alpha))
          (alias export $i "beta" (type $beta))
          (core func $alpha-drop (canon resource.drop $alpha))
          (core func $beta-drop (canon resource.drop $beta))
          (core module $m
            (func (import "host" "drop-a") (param i32))
            (func (import "host" "drop-b") (param i32))
            (func (export "drop-alpha") (param i32) local.get 0 call 0)
            (func (export "drop-beta") (param i32) local.get 0 call 1))
          (core instance $core (instantiate $m
            (with "host" (instance
              (export "drop-a" (func $alpha-drop))
              (export "drop-b" (func $beta-drop))))))
          (func (export "drop-alpha") (param "h" (own $alpha))
            (canon lift (core func $core "drop-alpha")))
          (func (export "drop-beta") (param "h" (own $beta))
            (canon lift (core func $core "drop-beta"))))
        "#
    );
    #[derive(Default)]
    struct Counters {
        alphas: Vec<u32>,
        betas: Vec<u32>,
    }
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let mut linker: Linker<Counters> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd009-tests:host/multi@0.1.0".parse().expect("identifier");
    let mut iface_view = linker.instance(&iface);
    let alpha_id = iface_view
        .resource("alpha", |c: &mut Counters, rep: u32| -> crate::Result<()> {
            c.alphas.push(rep);
            Ok(())
        })
        .expect("the registration");
    let beta_id = iface_view
        .resource("beta", |c: &mut Counters, rep: u32| -> crate::Result<()> {
            c.betas.push(rep);
            Ok(())
        })
        .expect("the registration");
    let mut store: Store<Counters> = Store::new(&engine, Counters::default()).expect("store");
    let inst = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let drop_alpha = inst.get_func("drop-alpha").expect("drop-alpha");
    let drop_beta = inst.get_func("drop-beta").expect("drop-beta");
    let a = store.resource_new(alpha_id, 11).expect("mint alpha");
    let b = store.resource_new(beta_id, 22).expect("mint beta");
    drop_alpha
        .call(&mut store, &[Val::Own(a)])
        .await
        .expect("call drop-alpha");
    drop_beta
        .call(&mut store, &[Val::Own(b)])
        .await
        .expect("call drop-beta");
    assert_eq!(store.data().alphas, vec![11]);
    assert_eq!(store.data().betas, vec![22]);
}

#[wcmp_macros::test]
async fn it_reuses_freed_handle_indices_after_drop() {
    // A drop frees the slot; the next mint reuses the same index.
    // This is the structural form of the canonical-ABI runtime-
    // state rule that the resource trampoline will exercise.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (import "pdd009-tests:host/resources@0.1.0" (instance $i
            (export "thing" (type (sub resource)))))
          (alias export $i "thing" (type $thing))
          (core func $thing-drop (canon resource.drop $thing))
          (core module $m
            (func (import "host" "drop") (param i32))
            (func (export "consume") (param i32) local.get 0 call 0))
          (core instance $core (instantiate $m
            (with "host" (instance
              (export "drop" (func $thing-drop))))))
          (func (export "consume") (param "h" (own $thing))
            (canon lift (core func $core "consume"))))
        "#
    );
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd009-tests:host/resources@0.1.0"
        .parse()
        .expect("identifier");
    let type_id = linker
        .instance(&iface)
        .resource("thing", |_: &mut (), _: u32| -> crate::Result<()> {
            Ok(())
        })
        .expect("the registration");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let inst = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let consume = inst.get_func("consume").expect("consume");

    let h0 = store.resource_new(type_id, 1).expect("h0");
    let h1 = store.resource_new(type_id, 2).expect("h1");
    let h2 = store.resource_new(type_id, 3).expect("h2");
    let h1_index = h1.index();
    consume
        .call(&mut store, &[Val::Own(h1)])
        .await
        .expect("drop middle");
    // The middle slot is free; minting again reuses it.
    let h3 = store.resource_new(type_id, 4).expect("h3");
    assert_eq!(h3.index(), h1_index);
    // The two siblings are still live and distinct.
    assert_ne!(h0.index(), h2.index());
    assert_ne!(h0.index(), h3.index());
}

#[wcmp_macros::test]
async fn it_rejects_a_component_that_imports_an_unsatisfied_resource() {
    // The component's import declares `thing` as a resource, but
    // the host registers nothing under that label. The resolver
    // surfaces `LinkError::UnresolvedImport` at instantiate time.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (import "pdd009-tests:host/resources@0.1.0" (instance $i
            (export "thing" (type (sub resource)))))
          (alias export $i "thing" (type $thing))
          (core func $thing-drop (canon resource.drop $thing))
          (core module $m
            (func (import "host" "drop") (param i32))
            (func (export "noop") nop))
          (core instance $core (instantiate $m
            (with "host" (instance
              (export "drop" (func $thing-drop))))))
          (func (export "noop")
            (canon lift (core func $core "noop"))))
        "#
    );
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    // Register the interface but no `thing` resource.
    let iface: InterfaceIdentifier = "pdd009-tests:host/resources@0.1.0"
        .parse()
        .expect("identifier");
    let _ = linker.instance(&iface);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let outcome = linker.instantiate(&mut store, &component).await;
    let err = outcome.err().expect("instantiation should fail");
    assert!(
        matches!(err, Error::Link(_)),
        "expected Error::Link, got {err:?}"
    );
}

/// A component that defines its own resource type with an in-binary
/// destructor. The destructor lives in a first core instance so the
/// resource type can name it; the second core instance mints and
/// drops handles through `resource.new` and `resource.drop`.
const LOCAL_RESOURCE: &[u8] = component!(
    r#"
    (component
      (core module $d
        (global $dropped (mut i32) (i32.const 0))
        (global $last (mut i32) (i32.const 0))
        (func (export "dtor") (param i32)
          global.get $dropped i32.const 1 i32.add global.set $dropped
          local.get 0 global.set $last)
        (func (export "dropped") (result i32) global.get $dropped)
        (func (export "last") (result i32) global.get $last))
      (core instance $di (instantiate $d))
      (type $thing (resource (rep i32) (dtor (core func $di "dtor"))))
      (core func $new (canon resource.new $thing))
      (core func $drop (canon resource.drop $thing))
      (core module $m
        (import "" "new" (func $new (param i32) (result i32)))
        (import "" "drop" (func $drop (param i32)))
        (func (export "make") (param i32) (result i32) local.get 0 call $new)
        (func (export "dispose") (param i32) local.get 0 call $drop))
      (core instance $i (instantiate $m
        (with "" (instance (export "new" (func $new)) (export "drop" (func $drop))))))
      (export $thing' "thing" (type $thing))
      (func (export "make") (param "rep" u32) (result (own $thing'))
        (canon lift (core func $i "make")))
      (func (export "dispose") (param "h" (own $thing'))
        (canon lift (core func $i "dispose")))
      (func (export "dropped") (result u32) (canon lift (core func $di "dropped")))
      (func (export "last") (result u32) (canon lift (core func $di "last"))))
    "#
);

async fn local_resource_instance() -> (Store<()>, crate::Instance) {
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, LOCAL_RESOURCE)
        .await
        .expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    (store, instance)
}

async fn make_handle(
    store: &mut Store<()>,
    instance: &crate::Instance,
    rep: u32,
) -> ResourceHandle {
    let make = instance.get_func("make").expect("make export");
    let results = make.call(store, &[Val::U32(rep)]).await.expect("make call");
    match results.as_ref() {
        [Val::Own(handle)] => *handle,
        other => panic!("expected an owned handle, got {other:?}"),
    }
}

#[wcmp_macros::test]
async fn it_translates_and_instantiates_a_locally_defined_resource() {
    let (mut store, instance) = local_resource_instance().await;
    let handle = make_handle(&mut store, &instance, 7).await;
    assert_eq!(
        handle.index(),
        1,
        "the first handle takes the first table slot"
    );
}

#[wcmp_macros::test]
async fn it_mints_a_distinct_resource_type_identity_per_instantiation() {
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, LOCAL_RESOURCE)
        .await
        .expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let first = linker
        .instantiate(&mut store, &component)
        .await
        .expect("first instantiation");
    let second = linker
        .instantiate(&mut store, &component)
        .await
        .expect("second instantiation");
    let a = make_handle(&mut store, &first, 1).await;
    let b = make_handle(&mut store, &second, 2).await;
    assert_ne!(
        a.type_id(),
        b.type_id(),
        "each instantiation carries its own resource type identity"
    );
}

// ----------------------------------------------------------------
// Stubs: capabilities the polyfill does not yet realise.
// ----------------------------------------------------------------

#[wcmp_macros::test]
async fn it_supports_a_locally_defined_resource_with_an_in_binary_destructor() {
    let (mut store, instance) = local_resource_instance().await;
    let handle = make_handle(&mut store, &instance, 7).await;
    let dispose = instance.get_func("dispose").expect("dispose export");
    dispose
        .call(&mut store, &[Val::Own(handle)])
        .await
        .expect("dispose call");
    let dropped = instance.get_func("dropped").expect("dropped export");
    let last = instance.get_func("last").expect("last export");
    assert_eq!(
        dropped
            .call(&mut store, &[])
            .await
            .expect("dropped")
            .as_ref(),
        &[Val::U32(1)],
        "the in-binary destructor ran when the guest dropped the handle"
    );
    assert_eq!(
        last.call(&mut store, &[]).await.expect("last").as_ref(),
        &[Val::U32(7)],
        "the destructor received the dropped entry's rep"
    );
}

#[wcmp_macros::test]
async fn it_runs_the_in_binary_destructor_exactly_once_per_dropped_handle() {
    let (mut store, instance) = local_resource_instance().await;
    let dispose = instance.get_func("dispose").expect("dispose export");
    let dropped = instance.get_func("dropped").expect("dropped export");
    let kept = make_handle(&mut store, &instance, 1).await;
    for rep in [2, 3] {
        let handle = make_handle(&mut store, &instance, rep).await;
        dispose
            .call(&mut store, &[Val::Own(handle)])
            .await
            .expect("dispose call");
    }
    assert_eq!(
        dropped
            .call(&mut store, &[])
            .await
            .expect("dropped")
            .as_ref(),
        &[Val::U32(2)],
        "two handles dropped, two destructor runs; the kept handle ran none"
    );
    let _ = kept;
}

#[wcmp_macros::test]
async fn it_rejects_a_handle_from_another_instance_of_the_same_component() {
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, LOCAL_RESOURCE)
        .await
        .expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let first = linker
        .instantiate(&mut store, &component)
        .await
        .expect("first instantiation");
    let second = linker
        .instantiate(&mut store, &component)
        .await
        .expect("second instantiation");
    let handle = make_handle(&mut store, &first, 1).await;
    let dispose = second.get_func("dispose").expect("dispose export");
    let err = dispose
        .call(&mut store, &[Val::Own(handle)])
        .await
        .expect_err("a handle from another instance must not lower");
    assert!(
        matches!(&err, Error::Abi(abi) if matches!(abi.cause, AbiCause::UnregisteredResourceType)),
        "expected the unregistered-resource-type cause, got {err:?}"
    );
}

#[wcmp_macros::test]
async fn it_rejects_a_local_destructor_with_the_wrong_signature() {
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (core module $d
            (func (export "dtor") (param i32) (result i32) local.get 0))
          (core instance $di (instantiate $d))
          (type $thing (resource (rep i32) (dtor (core func $di "dtor"))))
          (core func $new (canon resource.new $thing))
          (core module $m
            (import "" "new" (func $new (param i32) (result i32)))
            (func (export "make") (param i32) (result i32) local.get 0 call $new))
          (core instance $i (instantiate $m
            (with "" (instance (export "new" (func $new))))))
          (export $thing' "thing" (type $thing))
          (func (export "make") (param "rep" u32) (result (own $thing'))
            (canon lift (core func $i "make"))))
        "#
    );
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let outcome = match Component::new(&engine, COMPONENT).await {
        Ok(component) => {
            let linker: Linker<()> = Linker::new(&engine);
            let mut store: Store<()> = Store::new(&engine, ()).expect("store");
            linker.instantiate(&mut store, &component).await.map(|_| ())
        }
        Err(err) => Err(err),
    };
    let err = outcome.expect_err("a destructor that returns a value is refused");
    assert!(
        matches!(
            &err,
            Error::Instantiation(_) | Error::InvalidComponentBinary { .. }
        ),
        "expected an instantiation or validation error, got {err:?}"
    );
}

/// A component that imports one resource type through two
/// interfaces: `a` declares `thing` and `make`, `b` declares its
/// `thing` equal to `a`'s and takes one by value in `consume`.
const SHARED: &[u8] = component!(
    r#"
    (component
      (import "pdd013-tests:host/a@0.1.0" (instance $a
        (export "thing" (type $thing (sub resource)))
        (export "make" (func (result (own $thing))))))
      (alias export $a "thing" (type $thing))
      (import "pdd013-tests:host/b@0.1.0" (instance $b
        (alias outer 1 $thing (type $outer))
        (export "thing" (type (eq $outer)))
        (export "consume" (func (param "h" (own $outer))))))
      (alias export $a "make" (func $make))
      (alias export $b "consume" (func $consume))
      (core func $core-make (canon lower (func $make)))
      (core func $core-consume (canon lower (func $consume)))
      (core module $m
        (import "host" "make" (func $make (result i32)))
        (import "host" "consume" (func $consume (param i32)))
        (func (export "run") call $make call $consume))
      (core instance $c (instantiate $m
        (with "host" (instance
          (export "make" (func $core-make))
          (export "consume" (func $core-consume))))))
      (func (export "run") (canon lift (core func $c "run"))))
    "#
);

#[wcmp_macros::test]
async fn it_shares_a_single_resource_type_across_two_imported_interfaces() {
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, SHARED)
        .await
        .expect("component parses");
    let mut linker: Linker<Vec<u32>> = Linker::new(&engine);
    let a: InterfaceIdentifier = "pdd013-tests:host/a@0.1.0".parse().expect("identifier");
    let b: InterfaceIdentifier = "pdd013-tests:host/b@0.1.0".parse().expect("identifier");
    let thing: HostResource<Vec<u32>> =
        HostResource::new(|dropped: &mut Vec<u32>, rep: u32| -> crate::Result<()> {
            dropped.push(rep);
            Ok(())
        });
    let type_id = linker
        .instance(&a)
        .resource_with("thing", thing.clone())
        .expect("the registration");
    linker
        .instance(&b)
        .resource_with("thing", thing)
        .expect("the registration");
    linker
        .instance(&a)
        .func_new(
            "make",
            FunctionType {
                parameters: Vec::new(),
                result: Some(ValueType::Own(ResourceType::new("thing"))),
                async_: false,
            },
            move |call: HostCall<'_, Vec<u32>>, _args, results| {
                results[0] = Val::Own(call.resource_new(type_id, 9)?);
                Ok(())
            },
        )
        .expect("the registration");
    linker
        .instance(&b)
        .func_new(
            "consume",
            FunctionType {
                parameters: vec![FunctionParameter {
                    name: "h".to_owned(),
                    ty: ValueType::Own(ResourceType::new("thing")),
                }],
                result: None,
                async_: false,
            },
            |mut call: HostCall<'_, Vec<u32>>, args, _results| {
                let Val::Own(handle) = &args[0] else {
                    panic!("expected an owned handle, got {args:?}");
                };
                // The handle minted under `a` arrives through `b` with the
                // same identity; record the rep the host gave it.
                call.data_mut().push(handle.rep());
                Ok(())
            },
        )
        .expect("the registration");
    let mut store: Store<Vec<u32>> = Store::new(&engine, Vec::new()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("one identity behind both interfaces links");
    let run = instance.get_func("run").expect("run export");
    run.call(&mut store, &[]).await.expect("run");
    assert_eq!(store.data(), &vec![9]);
}

#[wcmp_macros::test]
async fn it_rejects_two_identities_for_one_declared_resource_type() {
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, SHARED)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let a: InterfaceIdentifier = "pdd013-tests:host/a@0.1.0".parse().expect("identifier");
    let b: InterfaceIdentifier = "pdd013-tests:host/b@0.1.0".parse().expect("identifier");
    let type_id = linker
        .instance(&a)
        .resource("thing", |_: &mut (), _: u32| -> crate::Result<()> {
            Ok(())
        })
        .expect("the registration");
    linker
        .instance(&b)
        .resource("thing", |_: &mut (), _: u32| -> crate::Result<()> {
            Ok(())
        })
        .expect("the registration");
    linker
        .instance(&a)
        .func_new(
            "make",
            FunctionType {
                parameters: Vec::new(),
                result: Some(ValueType::Own(ResourceType::new("thing"))),
                async_: false,
            },
            move |call: HostCall<'_, ()>, _args, results| {
                results[0] = Val::Own(call.resource_new(type_id, 1)?);
                Ok(())
            },
        )
        .expect("the registration");
    linker
        .instance(&b)
        .func_new(
            "consume",
            FunctionType {
                parameters: vec![FunctionParameter {
                    name: "h".to_owned(),
                    ty: ValueType::Own(ResourceType::new("thing")),
                }],
                result: None,
                async_: false,
            },
            |_: HostCall<'_, ()>, _args, _results| Ok(()),
        )
        .expect("the registration");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let err = match linker.instantiate(&mut store, &component).await {
        Ok(_) => panic!("two identities for one declared resource type must not link"),
        Err(err) => err,
    };
    assert!(
        matches!(&err, Error::TypeMismatch(mismatch)
            if matches!(mismatch.position, crate::TypeMismatchPosition::HostFunctionRegistration { ref item, .. } if item == "thing")),
        "expected a type mismatch naming the `thing` registration, got {err:?}"
    );
}

/// A component that imports a resource type and a `make` function
/// returning `own<thing>`, and re-exports what `make` returns.
const MINTER: &[u8] = component!(
    r#"
    (component
      (import "pdd011-tests:host/things@0.1.0" (instance $i
        (export "thing" (type $thing (sub resource)))
        (export "make" (func (result (own $thing))))))
      (alias export $i "make" (func $make))
      (alias export $i "thing" (type $thing))
      (core func $core-make (canon lower (func $make)))
      (core module $m
        (import "host" "make" (func $make (result i32)))
        (func (export "run") (result i32) call $make))
      (core instance $c (instantiate $m
        (with "host" (instance (export "make" (func $core-make))))))
      (func (export "run") (result (own $thing)) (canon lift (core func $c "run"))))
    "#
);

#[wcmp_macros::test]
async fn it_lets_a_host_function_mint_a_resource_handle_during_a_guest_call() {
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, MINTER)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd011-tests:host/things@0.1.0"
        .parse()
        .expect("identifier");
    let type_id = linker
        .instance(&iface)
        .resource("thing", |_: &mut (), _: u32| -> crate::Result<()> {
            Ok(())
        })
        .expect("the registration");
    linker
        .instance(&iface)
        .func_new(
            "make",
            FunctionType {
                parameters: Vec::new(),
                result: Some(ValueType::Own(ResourceType::new("thing"))),
                async_: false,
            },
            move |call: HostCall<'_, ()>, _args, results| {
                results[0] = Val::Own(call.resource_new(type_id, 42)?);
                Ok(())
            },
        )
        .expect("the registration");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let run = instance.get_func("run").expect("run export");
    let results = run.call(&mut store, &[]).await.expect("run");
    let [Val::Own(handle)] = results.as_ref() else {
        panic!("expected an owned handle, got {results:?}");
    };
    assert_eq!(
        handle.type_id(),
        type_id,
        "the handle carries the registered identity"
    );
    assert_eq!(
        handle.rep(),
        42,
        "the handle carries the rep the host minted"
    );
}

#[wcmp_macros::test]
async fn it_rejects_a_host_mint_against_an_unknown_resource_type() {
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, MINTER)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd011-tests:host/things@0.1.0"
        .parse()
        .expect("identifier");
    linker
        .instance(&iface)
        .resource("thing", |_: &mut (), _: u32| -> crate::Result<()> {
            Ok(())
        })
        .expect("the registration");
    let stranger = ResourceTypeId::fresh();
    linker
        .instance(&iface)
        .func_new(
            "make",
            FunctionType {
                parameters: Vec::new(),
                result: Some(ValueType::Own(ResourceType::new("thing"))),
                async_: false,
            },
            move |call: HostCall<'_, ()>, _args, results| {
                results[0] = Val::Own(call.resource_new(stranger, 1)?);
                Ok(())
            },
        )
        .expect("the registration");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let run = instance.get_func("run").expect("run export");
    let err = run
        .call(&mut store, &[])
        .await
        .expect_err("minting against an unknown identity fails the call");
    // The host error traps the guest, and the call gets it back as
    // the host function raised it.
    assert!(
        matches!(&err, Error::Abi(abi) if matches!(abi.cause, AbiCause::UnregisteredResourceType)),
        "expected the unregistered-resource-type cause, got {err:?}"
    );
    assert!(
        err.to_string().contains("at result: no host registration"),
        "an identity no registration and no instantiation of the store ever \
         introduced has no name to render, so the refusal names no value \
         type, got {err}"
    );
}

#[wcmp_macros::test]
async fn it_supports_resource_constructor_and_method_shaped_exports() {
    // The component imports an interface declaring a resource and
    // its `[constructor]`, `[method]`, and `[static]` shaped
    // functions. Wasmtime's runtime treats these as plain functions
    // with bracketed names; classification (`FunctionKind`) is a
    // bindgen-time concern, not a runtime one. The polyfill mirrors
    // that convention: the navigator addresses each shape by its
    // literal wire-name.
    use crate::{ExternType, PrimitiveType, ValueType};
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (type $iface (instance
            (export "bar" (type $bar (sub resource)))
            (type $ctor-ty (func (result (own $bar))))
            (export "[constructor]bar" (func (type $ctor-ty)))
            (type $method-ty (func (param "self" (borrow $bar)) (result u32)))
            (export "[method]bar.value" (func (type $method-ty)))
            (type $static-ty (func (result u32)))
            (export "[static]bar.kind" (func (type $static-ty)))))
          (import "test:host/things@0.1.0" (instance (type $iface))))
        "#
    );

    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let import = component
        .imports
        .iter()
        .find(|i| i.name.to_string() == "test:host/things@0.1.0")
        .expect("`test:host/things` import present");
    let ExternType::Instance(instance) = &import.ty else {
        panic!("expected an instance import");
    };

    // The resource type itself is exposed alongside the bracketed
    // function names; each one is addressable as a plain instance
    // item.
    let names: Vec<&str> = instance.items.iter().map(|i| i.name.as_str()).collect();
    assert!(names.contains(&"bar"), "resource type `bar` is exported");
    assert!(
        names.contains(&"[constructor]bar"),
        "constructor shape uses literal `[constructor]bar` name; got {names:?}",
    );
    assert!(
        names.contains(&"[method]bar.value"),
        "method shape uses literal `[method]bar.value` name; got {names:?}",
    );
    assert!(
        names.contains(&"[static]bar.kind"),
        "static-method shape uses literal `[static]bar.kind` name; got {names:?}",
    );

    // The constructor's projected signature returns `own<bar>` and
    // takes no parameters.
    let ctor = instance
        .items
        .iter()
        .find(|i| i.name == "[constructor]bar")
        .expect("constructor present");
    let ExternType::Function(ctor_ty) = &ctor.ty else {
        panic!("constructor should project to a function");
    };
    assert!(ctor_ty.parameters.is_empty());
    let Some(ValueType::Own(_)) = &ctor_ty.result else {
        panic!(
            "constructor should return own<bar>, got {:?}",
            ctor_ty.result
        );
    };

    // The method takes `borrow<bar>` as its first parameter and
    // returns `u32`.
    let method = instance
        .items
        .iter()
        .find(|i| i.name == "[method]bar.value")
        .expect("method present");
    let ExternType::Function(method_ty) = &method.ty else {
        panic!("method should project to a function");
    };
    assert_eq!(method_ty.parameters.len(), 1);
    let ValueType::Borrow(_) = &method_ty.parameters[0].ty else {
        panic!(
            "method's `self` should be borrow<bar>, got {:?}",
            method_ty.parameters[0].ty
        );
    };
    assert_eq!(
        method_ty.result,
        Some(ValueType::Primitive(PrimitiveType::U32))
    );

    // The static method takes no parameters and returns `u32`.
    let static_fn = instance
        .items
        .iter()
        .find(|i| i.name == "[static]bar.kind")
        .expect("static method present");
    let ExternType::Function(static_ty) = &static_fn.ty else {
        panic!("static method should project to a function");
    };
    assert!(static_ty.parameters.is_empty());
    assert_eq!(
        static_ty.result,
        Some(ValueType::Primitive(PrimitiveType::U32))
    );
}

/// A component that imports a resource type and exports two
/// functions over a borrow of it: `hold` keeps the borrow (never
/// drops it) and `peek` reads its rep through the host and drops it.
const BORROWER: &[u8] = component!(
    r#"
    (component
      (import "pdd014-tests:host/things@0.1.0" (instance $i
        (export "thing" (type $thing (sub resource)))
        (export "rep" (func (param "h" (borrow $thing)) (result u32)))))
      (alias export $i "thing" (type $thing))
      (alias export $i "rep" (func $rep))
      (core func $core-rep (canon lower (func $rep)))
      (core func $drop (canon resource.drop $thing))
      (core module $m
        (import "host" "rep" (func $rep (param i32) (result i32)))
        (import "host" "drop" (func $drop (param i32)))
        (func (export "hold") (param i32))
        (func (export "peek") (param i32) (result i32)
          (local $v i32)
          local.get 0 call $rep local.set $v
          local.get 0 call $drop
          local.get $v))
      (core instance $c (instantiate $m
        (with "host" (instance
          (export "rep" (func $core-rep))
          (export "drop" (func $drop))))))
      (func (export "hold") (param "h" (borrow $thing))
        (canon lift (core func $c "hold")))
      (func (export "peek") (param "h" (borrow $thing)) (result u32)
        (canon lift (core func $c "peek"))))
    "#
);

/// A linker carrying everything [`BORROWER`] imports, with the
/// `thing` resource registered from `resource` under the label the
/// component imports it by. Returns the identity to mint with.
fn borrower_linker(engine: &Engine, resource: HostResource<()>) -> (Linker<()>, ResourceTypeId) {
    let mut linker: Linker<()> = Linker::new(engine);
    let iface: InterfaceIdentifier = "pdd014-tests:host/things@0.1.0"
        .parse()
        .expect("identifier");
    let type_id = linker
        .instance(&iface)
        .resource_with("thing", resource)
        .expect("the registration");
    linker
        .instance(&iface)
        .func_new(
            "rep",
            FunctionType {
                parameters: vec![FunctionParameter {
                    name: "h".to_owned(),
                    ty: ValueType::Borrow(ResourceType::new("thing")),
                }],
                result: Some(ValueType::Primitive(crate::PrimitiveType::U32)),
                async_: false,
            },
            |_: HostCall<'_, ()>, args, results| {
                let Val::Borrow(handle) = &args[0] else {
                    panic!("expected a borrowed handle, got {args:?}");
                };
                results[0] = Val::U32(handle.rep());
                Ok(())
            },
        )
        .expect("the registration");
    (linker, type_id)
}

async fn borrower_instance() -> (Store<()>, crate::Instance, ResourceTypeId) {
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, BORROWER)
        .await
        .expect("component parses");
    let (linker, type_id) = borrower_linker(
        &engine,
        HostResource::new(|_: &mut (), _: u32| -> crate::Result<()> { Ok(()) }),
    );
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    (store, instance, type_id)
}

#[wcmp_macros::test]
async fn it_rejects_a_host_call_that_leaves_outstanding_borrows() {
    let (mut store, instance, type_id) = borrower_instance().await;
    let handle = store.resource_new(type_id, 5).expect("mint");
    let hold = instance.get_func("hold").expect("hold export");
    let err = hold
        .call(&mut store, &[Val::Borrow(handle)])
        .await
        .expect_err("the guest kept the borrow, so the call must fail");
    assert!(
        matches!(&err, Error::Abi(abi) if matches!(abi.cause, AbiCause::OutstandingBorrows { count: 1 })),
        "expected one outstanding borrow, got {err:?}"
    );
}

#[wcmp_macros::test]
async fn it_leaves_the_owning_handle_live_after_a_borrow_is_dropped_in_the_call() {
    let (mut store, instance, type_id) = borrower_instance().await;
    let handle = store.resource_new(type_id, 7).expect("mint");
    let peek = instance.get_func("peek").expect("peek export");
    let results = peek
        .call(&mut store, &[Val::Borrow(handle)])
        .await
        .expect("the guest dropped its borrow before returning");
    assert_eq!(
        results.as_ref(),
        &[Val::U32(7)],
        "the host read the rep through the borrow"
    );
    let mut tables = store.internal().tables().lock().expect("tables");
    let host_table = tables.host_table(type_id);
    assert_eq!(
        tables
            .for_table(host_table)
            .and_then(|t| t.get(handle.index())),
        Some(7),
        "the owning entry is live after the call"
    );
}

#[wcmp_macros::test]
async fn it_refuses_to_lower_a_borrow_of_a_released_handle() {
    // A handle is a copyable record, so a copy of one outlives the
    // entry it names. Lowering that copy as a `borrow<T>` would put
    // a freed rep in front of the guest if the lower took the
    // handle's own rep at its word, so the lower looks the handle up
    // in the host's table instead and finds nothing there.
    let (mut store, instance, type_id) = borrower_instance().await;
    let handle = store.resource_new(type_id, 9).expect("mint");
    store.resource_drop(handle).expect("the host releases it");
    let peek = instance.get_func("peek").expect("peek export");
    let err = peek
        .call(&mut store, &[Val::Borrow(handle)])
        .await
        .expect_err("a released handle names no entry to borrow");
    assert!(
        matches!(&err, Error::Abi(abi) if matches!(abi.cause, AbiCause::InvalidHandle { .. })),
        "expected the invalid-handle cause, got {err:?}"
    );
}

#[wcmp_macros::test]
async fn it_refuses_to_lower_a_borrow_the_host_never_minted() {
    // The same lookup refuses a handle the host assembled out of
    // thin air: an index the host's table for the type never handed
    // out names no entry, whatever rep the record carries beside it.
    let (mut store, instance, type_id) = borrower_instance().await;
    let forged: ResourceHandle = ResourceHandleParts {
        type_id,
        index: 999,
        rep: 4,
        generation: 0,
    }
    .into();
    let peek = instance.get_func("peek").expect("peek export");
    let err = peek
        .call(&mut store, &[Val::Borrow(forged)])
        .await
        .expect_err("the host never minted this handle");
    assert!(
        matches!(&err, Error::Abi(abi) if matches!(abi.cause, AbiCause::InvalidHandle { .. })),
        "expected the invalid-handle cause, got {err:?}"
    );
}

/// How an attempt to release a handle went: whether the failure was
/// the invalid-handle cause, and what it said.
type ReleaseAttempt = Option<(bool, String)>;

/// An instance over [`BORROWER`] whose `rep` import tries to release
/// the handle `lent` names and records the attempt in `attempt`.
/// The guest's `peek` calls that import while it still holds the
/// borrow the host lent for the call, so the attempt lands in the
/// middle of the call.
async fn drop_attempt_instance(
    lent: Arc<Mutex<Option<ResourceHandle>>>,
    attempt: Arc<Mutex<ReleaseAttempt>>,
) -> (Store<()>, crate::Instance, ResourceTypeId) {
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, BORROWER)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd014-tests:host/things@0.1.0"
        .parse()
        .expect("identifier");
    let type_id = linker
        .instance(&iface)
        .resource("thing", |_: &mut (), _: u32| -> crate::Result<()> {
            Ok(())
        })
        .expect("the registration");
    linker
        .instance(&iface)
        .func_new(
            "rep",
            FunctionType {
                parameters: vec![FunctionParameter {
                    name: "h".to_owned(),
                    ty: ValueType::Borrow(ResourceType::new("thing")),
                }],
                result: Some(ValueType::Primitive(crate::PrimitiveType::U32)),
                async_: false,
            },
            move |mut call: HostCall<'_, ()>, args, results| {
                let Val::Borrow(borrowed) = &args[0] else {
                    panic!("expected a borrowed handle, got {args:?}");
                };
                results[0] = Val::U32(borrowed.rep());
                let owned = lent
                    .lock()
                    .expect("the lent handle")
                    .expect("the host minted a handle before the call");
                *attempt.lock().expect("the attempt") =
                    Some(match call.store().internal().resource_drop(owned) {
                        Ok(()) => (false, "the release succeeded".to_owned()),
                        Err(err) => (
                            matches!(&err, Error::Abi(abi) if matches!(
                                abi.cause,
                                AbiCause::InvalidHandle { .. }
                            )),
                            err.to_string(),
                        ),
                    });
                Ok(())
            },
        )
        .expect("the registration");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    (store, instance, type_id)
}

#[wcmp_macros::test]
async fn it_refuses_to_release_a_lent_handle_until_the_call_ends() {
    // The handle the host lowers as a `borrow<T>` is lent to the
    // export's task for the length of the call, so a host function
    // the guest calls in the middle of that call cannot take the
    // owning entry back out. The lend comes back when the task
    // resolves, and the same release then succeeds.
    let lent = Arc::new(Mutex::new(None));
    let attempt: Arc<Mutex<ReleaseAttempt>> = Arc::new(Mutex::new(None));
    let (mut store, instance, type_id) = drop_attempt_instance(lent.clone(), attempt.clone()).await;
    let handle = store.resource_new(type_id, 13).expect("mint");
    *lent.lock().expect("the lent handle") = Some(handle);

    let peek = instance.get_func("peek").expect("peek export");
    let results = peek
        .call(&mut store, &[Val::Borrow(handle)])
        .await
        .expect("the guest dropped its borrow before returning");
    assert_eq!(
        results.as_ref(),
        &[Val::U32(13)],
        "the guest read the rep of the entry the host lent"
    );

    let (invalid_handle, message) = attempt
        .lock()
        .expect("the attempt")
        .clone()
        .expect("the guest called the host's `rep`");
    assert!(
        invalid_handle,
        "releasing a lent handle mid-call must fail with the invalid-handle \
         cause, got: {message}"
    );
    assert!(
        message.contains("cannot remove owned resource while borrowed"),
        "the refusal must name the lend, got: {message}"
    );

    store
        .resource_drop(handle)
        .expect("the call ended, so the lend came back and the release stands");
}

/// Every message in an error's source chain, joined so a cause the
/// substrate wrapped can be matched wherever it put it. A failure
/// raised inside a host function reaches the guest's call through the
/// runtime's trap surface, which keeps the structured cause as a
/// source rather than as the top-level error.
fn chain(error: &Error) -> String {
    let mut out = String::new();
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(link) = current {
        if !out.is_empty() {
            out.push_str(": ");
        }
        out.push_str(&link.to_string());
        current = link.source();
    }
    out
}

/// A [`BORROWER`] instance whose `rep` import records the handle the
/// guest lent it in `seen`, so that a test can take that handle —
/// which carries the guest's table index, not one of the host's —
/// and try to lower it back into a guest.
async fn recording_borrower_instance(
    seen: Arc<Mutex<Option<ResourceHandle>>>,
) -> (Store<()>, crate::Instance, ResourceTypeId) {
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, BORROWER)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd014-tests:host/things@0.1.0"
        .parse()
        .expect("identifier");
    let type_id = linker
        .instance(&iface)
        .resource("thing", |_: &mut (), _: u32| -> crate::Result<()> {
            Ok(())
        })
        .expect("the registration");
    linker
        .instance(&iface)
        .func_new(
            "rep",
            FunctionType {
                parameters: vec![FunctionParameter {
                    name: "h".to_owned(),
                    ty: ValueType::Borrow(ResourceType::new("thing")),
                }],
                result: Some(ValueType::Primitive(crate::PrimitiveType::U32)),
                async_: false,
            },
            move |_: HostCall<'_, ()>, args, results| {
                let Val::Borrow(handle) = &args[0] else {
                    panic!("expected a borrowed handle, got {args:?}");
                };
                results[0] = Val::U32(handle.rep());
                *seen.lock().expect("the borrowed handle") = Some(*handle);
                Ok(())
            },
        )
        .expect("the registration");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    (store, instance, type_id)
}

#[wcmp_macros::test]
async fn it_refuses_to_lower_a_borrow_the_host_kept_past_the_call_that_lent_it() {
    // A `borrow<T>` the host receives from a guest names a borrow entry
    // of the host's own table, which the call that handed it over owns:
    // it goes when that call ends. A copy the host keeps past the call
    // names nothing, so lowering it back is refused, and nothing is lent
    // to the refused call. The guest's own index plays no part: the
    // guest sees the borrow at index one of its table, where the host's
    // table holds another resource.
    let seen: Arc<Mutex<Option<ResourceHandle>>> = Arc::new(Mutex::new(None));
    let (mut store, instance, type_id) = recording_borrower_instance(seen.clone()).await;
    let first = store.resource_new(type_id, 100).expect("mint");
    let second = store.resource_new(type_id, 200).expect("mint");
    assert_eq!(
        (first.index(), second.index()),
        (1, 2),
        "the host's table hands out index one first"
    );

    let peek = instance.get_func("peek").expect("peek export");
    let results = peek
        .call(&mut store, &[Val::Borrow(second)])
        .await
        .expect("the guest dropped its borrow before returning");
    assert_eq!(
        results.as_ref(),
        &[Val::U32(200)],
        "the guest read the rep of the entry the host lent"
    );

    let lifted = seen
        .lock()
        .expect("the borrowed handle")
        .expect("the guest called the host's `rep`");
    assert_eq!(lifted.rep(), 200, "the host's borrow carries the rep");
    assert_ne!(
        lifted.index(),
        first.index(),
        "the host's borrow names an entry of its own table, not the guest's index"
    );

    let err = peek
        .call(&mut store, &[Val::Borrow(lifted)])
        .await
        .expect_err("the borrow went with the call that handed it to the host");
    assert!(
        matches!(&err, Error::Abi(abi) if matches!(abi.cause, AbiCause::InvalidHandle { .. })),
        "expected the invalid-handle cause, got {err:?}"
    );

    store
        .resource_drop(first)
        .expect("the refused lower left the other entry alone");
    store
        .resource_drop(second)
        .expect("the first call ended, so its lend came back");
}

/// The reason of the invalid-handle failure `err` carries.
fn invalid_handle_reason(err: &Error) -> String {
    match err {
        Error::Abi(abi) => match &abi.cause {
            AbiCause::InvalidHandle { reason } => reason.clone(),
            other => panic!("expected the invalid-handle cause, got {other:?}"),
        },
        other => panic!("expected an ABI failure, got {other:?}"),
    }
}

#[wcmp_macros::test]
async fn it_refuses_to_lower_a_stale_handle_whose_slot_was_reminted_with_the_same_rep() {
    // The host releases a handle and mints another with the same rep,
    // which takes the freed index. The released handle and the new one
    // carry the same index and the same rep, and only the generation of
    // the entry tells them apart, as Wasmtime's host index does.
    let (mut store, instance, type_id) = borrower_instance().await;
    let stale = store.resource_new(type_id, 9).expect("mint");
    store.resource_drop(stale).expect("release");
    let fresh = store.resource_new(type_id, 9).expect("mint again");
    assert_eq!(
        (fresh.index(), fresh.rep()),
        (stale.index(), stale.rep()),
        "the new handle took the released index, with the same rep"
    );

    let peek = instance.get_func("peek").expect("peek export");
    let err = peek
        .call(&mut store, &[Val::Borrow(stale)])
        .await
        .expect_err("the released handle names another entry now");
    assert!(
        invalid_handle_reason(&err).contains("host-owned resource was already de-allocated"),
        "the refusal is Wasmtime's for a host handle whose resource is gone, got {err}"
    );
    // The refused lower is a trap, which poisons the store, so the new
    // handle is checked through its release, which runs no guest code.
    store
        .resource_drop(fresh)
        .expect("the resource that took the index is still the host's");
}

#[wcmp_macros::test]
async fn it_refuses_to_release_a_stale_handle_whose_slot_was_reminted() {
    // Releasing the old handle again must not release the resource that
    // took its index, which would run that resource's destructor.
    let (mut store, _instance, type_id) = borrower_instance().await;
    let stale = store.resource_new(type_id, 9).expect("mint");
    store.resource_drop(stale).expect("release");
    let fresh = store.resource_new(type_id, 9).expect("mint again");

    let err = store
        .resource_drop(stale)
        .expect_err("the released handle names another entry now");
    assert!(
        invalid_handle_reason(&err).contains("host-owned resource was already de-allocated"),
        "got {err}"
    );
    store
        .resource_drop(fresh)
        .expect("the resource that took the index is still the host's");
}

#[wcmp_macros::test]
async fn it_names_the_rep_mismatch_when_a_handle_records_another_rep() {
    // A handle whose index and generation agree with a live entry but
    // whose rep does not came from somewhere other than this entry. The
    // refusal says so, rather than calling a live index unknown.
    let (mut store, instance, type_id) = borrower_instance().await;
    let first = store.resource_new(type_id, 100).expect("mint");
    let forged: ResourceHandle = ResourceHandleParts {
        type_id,
        index: first.index(),
        rep: 200,
        generation: first.generation(),
    }
    .into();
    let peek = instance.get_func("peek").expect("peek export");
    let err = peek
        .call(&mut store, &[Val::Borrow(forged)])
        .await
        .expect_err("the forged handle records another rep");
    let reason = invalid_handle_reason(&err);
    assert!(
        reason.contains("with rep 100, not the rep 200 the handle records"),
        "the refusal names the mismatch, got {reason}"
    );
    store
        .resource_drop(first)
        .expect("the refused lower lent nothing");
}

/// A component over the same imported `thing` whose callback export
/// `lend` hands the borrow it was given to the host's asynchronous
/// `hold-borrow`, waits for that call, and then drops the borrow.
const LENDER: &[u8] = component!(
    r#"
    (component
      (import "pdd014-tests:host/things@0.1.0" (instance $i
        (export "thing" (type $thing (sub resource)))
        (export "hold-borrow" (func async (param "h" (borrow $thing))))))
      (alias export $i "thing" (type $thing))
      (alias export $i "hold-borrow" (func $hold))
      (core module $libc (memory (export "mem") 1))
      (core instance $libc (instantiate $libc))
      (core func $hold (canon lower (func $hold) async (memory (core memory $libc "mem"))))
      (core func $drop (canon resource.drop $thing))
      (core func $set-new (canon waitable-set.new))
      (core func $join (canon waitable.join))
      (core func $subtask-drop (canon subtask.drop))
      (core func $task-return (canon task.return))
      (core module $m
        (import "" "hold" (func $hold (param i32) (result i32)))
        (import "" "drop" (func $drop (param i32)))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (import "" "waitable.join" (func $join (param i32 i32)))
        (import "" "subtask.drop" (func $subtask-drop (param i32)))
        (import "" "task.return" (func $task-return))
        (global $h (mut i32) (i32.const 0))
        (global $subtask (mut i32) (i32.const 0))
        (func $finish (result i32)
          (call $drop (global.get $h))
          (call $task-return)
          (i32.const 0))
        (func (export "lend") (param $h i32) (result i32)
          (local $status i32) (local $set i32)
          (global.set $h (local.get $h))
          (local.set $status (call $hold (local.get $h)))
          (if (i32.eq (i32.and (local.get $status) (i32.const 15)) (i32.const 2))
            (then (return (call $finish))))
          (global.set $subtask (i32.shr_u (local.get $status) (i32.const 4)))
          (local.set $set (call $set-new))
          (call $join (global.get $subtask) (local.get $set))
          (i32.or (i32.shl (local.get $set) (i32.const 4)) (i32.const 2)))
        (func (export "lend-callback") (param i32 i32 i32) (result i32)
          (call $join (global.get $subtask) (i32.const 0))
          (call $subtask-drop (global.get $subtask))
          (call $finish)))
      (core instance $c (instantiate $m (with "" (instance
        (export "hold" (func $hold))
        (export "drop" (func $drop))
        (export "waitable-set.new" (func $set-new))
        (export "waitable.join" (func $join))
        (export "subtask.drop" (func $subtask-drop))
        (export "task.return" (func $task-return))))))
      (func (export "lend") async (param "h" (borrow $thing))
        (canon lift (core func $c "lend") async
          (callback (core func $c "lend-callback")))))
    "#
);

#[wcmp_macros::test]
async fn it_lowers_a_borrow_the_host_lifted_out_of_a_guest_back_in_with_its_rep() {
    // While `lend`'s call to the host is in flight, the host lowers the
    // borrow it received into a second instance's `peek`. The borrow
    // hands over its rep, as Wasmtime's host borrow does, and lends
    // nothing of its own. Once `lend` resolves, the borrow is gone, and
    // every lend has come back, so the host can release its handle.
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let lender = Component::new(&engine, LENDER)
        .await
        .expect("component parses");
    let borrower = Component::new(&engine, BORROWER)
        .await
        .expect("component parses");
    let (mut linker, type_id) = borrower_linker(
        &engine,
        HostResource::new(|_: &mut (), _: u32| -> crate::Result<()> { Ok(()) }),
    );
    let held: Arc<Mutex<Option<ResourceHandle>>> = Arc::new(Mutex::new(None));
    let release = Arc::new(AtomicBool::new(false));
    {
        let held = held.clone();
        let release = release.clone();
        let iface: InterfaceIdentifier = "pdd014-tests:host/things@0.1.0"
            .parse()
            .expect("identifier");
        linker
            .instance(&iface)
            .func_new_concurrent(
                "hold-borrow",
                FunctionType {
                    parameters: vec![FunctionParameter {
                        name: "h".to_owned(),
                        ty: ValueType::Borrow(ResourceType::new("thing")),
                    }],
                    result: None,
                    async_: true,
                },
                move |_accessor: &crate::Accessor<()>, args: Vec<Val>| {
                    let Some(Val::Borrow(handle)) = args.first() else {
                        panic!("expected a borrowed handle, got {args:?}");
                    };
                    *held.lock().expect("the held handle") = Some(*handle);
                    let release = release.clone();
                    core::future::poll_fn(move |context| {
                        if release.load(Ordering::Relaxed) {
                            return core::task::Poll::Ready(Ok(Vec::new()));
                        }
                        context.waker().wake_by_ref();
                        core::task::Poll::Pending
                    })
                },
            )
            .expect("the registration");
    }
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let lending = linker
        .instantiate(&mut store, &lender)
        .await
        .expect("instantiate the lender");
    let borrowing = linker
        .instantiate(&mut store, &borrower)
        .await
        .expect("instantiate the borrower");
    let lend = lending.get_func("lend").expect("lend export");
    let peek = borrowing.get_func("peek").expect("peek export");
    let minted = store.resource_new(type_id, 77).expect("mint");

    let (peeked, lifted) = store
        .run_concurrent(async |accessor| {
            let arguments = [Val::Borrow(minted)];
            let mut lent = core::pin::pin!(lend.call_concurrent(accessor, &arguments));
            core::future::poll_fn(|context| {
                assert!(
                    lent.as_mut().poll(context).is_pending(),
                    "`lend` waits on the host"
                );
                if held.lock().expect("the held handle").is_some() {
                    return core::task::Poll::Ready(());
                }
                context.waker().wake_by_ref();
                core::task::Poll::Pending
            })
            .await;
            let lifted = held
                .lock()
                .expect("the held handle")
                .expect("the host holds the borrow");
            let peeked = peek
                .call_concurrent(accessor, &[Val::Borrow(lifted)])
                .await
                .expect("the host's borrow lowers back into a guest");
            release.store(true, Ordering::Relaxed);
            lent.await
                .expect("`lend` resolves once the host's call does");
            (peeked, lifted)
        })
        .await
        .expect("the closure runs");
    assert_eq!(
        peeked.as_ref(),
        &[Val::U32(77)],
        "the second guest read the rep the borrow carries"
    );

    let err = peek
        .call(&mut store, &[Val::Borrow(lifted)])
        .await
        .expect_err("the borrow went with the call that handed it over");
    assert!(
        matches!(&err, Error::Abi(abi) if matches!(abi.cause, AbiCause::InvalidHandle { .. })),
        "got {err:?}"
    );
    store
        .resource_drop(minted)
        .expect("every lend of the host's handle came back");
}

/// A component over the same imported `thing` whose `grab` export
/// asks the host for an `own<thing>` while it still holds the borrow
/// the host lent for the call.
const GRABBER: &[u8] = component!(
    r#"
    (component
      (import "pdd014-tests:host/things@0.1.0" (instance $i
        (export "thing" (type $thing (sub resource)))
        (export "take" (func (result (own $thing))))))
      (alias export $i "thing" (type $thing))
      (alias export $i "take" (func $take))
      (core func $core-take (canon lower (func $take)))
      (core func $drop (canon resource.drop $thing))
      (core module $m
        (import "host" "take" (func $take (result i32)))
        (import "host" "drop" (func $drop (param i32)))
        (func (export "grab") (param i32)
          call $take call $drop
          local.get 0 call $drop))
      (core instance $c (instantiate $m
        (with "host" (instance
          (export "take" (func $core-take))
          (export "drop" (func $drop))))))
      (func (export "grab") (param "h" (borrow $thing))
        (canon lift (core func $c "grab"))))
    "#
);

#[wcmp_macros::test]
async fn it_refuses_to_lower_a_lent_handle_again_as_an_own() {
    // The lend the borrow put on the host's entry is what stops the
    // same handle from reaching the guest a second time as an
    // `own<T>`: that lowering takes the entry out of the host's
    // table, which would leave the borrow the guest still holds
    // pointing at nothing. The removal is refused while the lend
    // stands, and the refusal travels out to the guest's call.
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, GRABBER)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd014-tests:host/things@0.1.0"
        .parse()
        .expect("identifier");
    let type_id = linker
        .instance(&iface)
        .resource("thing", |_: &mut (), _: u32| -> crate::Result<()> {
            Ok(())
        })
        .expect("the registration");
    let lent: Arc<Mutex<Option<ResourceHandle>>> = Arc::new(Mutex::new(None));
    let given = lent.clone();
    linker
        .instance(&iface)
        .func_new(
            "take",
            FunctionType {
                parameters: Vec::new(),
                result: Some(ValueType::Own(ResourceType::new("thing"))),
                async_: false,
            },
            move |_: HostCall<'_, ()>, _args, results| {
                let owned = given
                    .lock()
                    .expect("the lent handle")
                    .expect("the host minted a handle before the call");
                results[0] = Val::Own(owned);
                Ok(())
            },
        )
        .expect("the registration");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");

    let handle = store.resource_new(type_id, 21).expect("mint");
    *lent.lock().expect("the lent handle") = Some(handle);

    let grab = instance.get_func("grab").expect("grab export");
    let err = grab
        .call(&mut store, &[Val::Borrow(handle)])
        .await
        .expect_err("the entry is lent for the length of the call");
    let text = chain(&err);
    assert!(
        text.contains("cannot remove owned resource while borrowed"),
        "the refusal must name the lend, got: {text}"
    );

    store
        .resource_drop(handle)
        .expect("the call ended, so the lend came back and the release stands");
}

/// A component that defines its own resource and exports a function
/// over a borrow of it. `read` hands back the core value the lower
/// produced, which for the defining instance is the rep itself.
const LOCAL_BORROWER: &[u8] = component!(
    r#"
    (component
      (type $thing (resource (rep i32)))
      (core func $new (canon resource.new $thing))
      (core module $m
        (import "" "new" (func $new (param i32) (result i32)))
        (func (export "make") (param i32) (result i32) local.get 0 call $new)
        (func (export "read") (param i32) (result i32) local.get 0))
      (core instance $i (instantiate $m
        (with "" (instance (export "new" (func $new))))))
      (export $thing' "thing" (type $thing))
      (func (export "make") (param "rep" u32) (result (own $thing'))
        (canon lift (core func $i "make")))
      (func (export "read") (param "h" (borrow $thing')) (result u32)
        (canon lift (core func $i "read"))))
    "#
);

#[wcmp_macros::test]
async fn it_lowers_a_borrow_into_the_defining_instance_through_the_hosts_entry() {
    // The defining instance receives the rep itself, with no borrow
    // entry behind it, so the lower returns early — but it reaches
    // that early return only after finding the handle's owning entry
    // in the host's table and lending it. The rep the guest reads
    // back is the entry's, and a handle the host has since released
    // names no entry to take one from, even on this path.
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, LOCAL_BORROWER)
        .await
        .expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");

    let handle = make_handle(&mut store, &instance, 42).await;
    let read = instance.get_func("read").expect("read export");
    let results = read
        .call(&mut store, &[Val::Borrow(handle)])
        .await
        .expect("the defining instance borrows its own resource");
    assert_eq!(
        results.as_ref(),
        &[Val::U32(42)],
        "the rep the guest read is the one its owning entry holds"
    );

    store
        .resource_drop(handle)
        .expect("the call ended, so the lend came back");
    let err = read
        .call(&mut store, &[Val::Borrow(handle)])
        .await
        .expect_err("a released handle names no entry to borrow");
    assert!(
        matches!(&err, Error::Abi(abi) if matches!(abi.cause, AbiCause::InvalidHandle { .. })),
        "expected the invalid-handle cause, got {err:?}"
    );
}

#[wcmp_macros::test]
async fn it_allocates_from_index_one_in_each_nested_instance() {
    // Two instantiations of one inner component each keep their own
    // table for the resource they define, and each table hands out
    // index 1 first, as the canonical ABI's reference table does.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (component $inner
            (type $r (resource (rep i32)))
            (core func $ctor (canon resource.new $r))
            (core func $drop (canon resource.drop $r))
            (core module $m
              (import "" "ctor" (func $ctor (param i32) (result i32)))
              (import "" "drop" (func $drop (param i32)))
              (func (export "alloc") (result i32) i32.const 100 call $ctor)
              (func (export "dealloc") (param i32) local.get 0 call $drop))
            (core instance $i (instantiate $m
              (with "" (instance
                (export "ctor" (func $ctor))
                (export "drop" (func $drop))))))
            (func (export "alloc") (result u32) (canon lift (core func $i "alloc")))
            (func (export "dealloc") (param "i" u32) (canon lift (core func $i "dealloc"))))
          (instance $i1 (instantiate $inner))
          (instance $i2 (instantiate $inner))
          (export "alloc-in1" (func $i1 "alloc"))
          (export "dealloc-in1" (func $i1 "dealloc"))
          (export "alloc-in2" (func $i2 "alloc"))
          (export "dealloc-in2" (func $i2 "dealloc")))
        "#
    );
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    async fn call(
        instance: &crate::Instance,
        store: &mut Store<()>,
        name: &str,
        args: &[Val],
    ) -> Box<[Val]> {
        instance
            .get_func(name)
            .unwrap_or_else(|| panic!("{name} export"))
            .call(store, args)
            .await
            .unwrap_or_else(|err| panic!("{name}: {err}"))
    }
    assert_eq!(
        call(&instance, &mut store, "alloc-in1", &[]).await.as_ref(),
        &[Val::U32(1)]
    );
    call(&instance, &mut store, "dealloc-in1", &[Val::U32(1)]).await;
    assert_eq!(
        call(&instance, &mut store, "alloc-in1", &[]).await.as_ref(),
        &[Val::U32(1)]
    );
    assert_eq!(
        call(&instance, &mut store, "alloc-in2", &[]).await.as_ref(),
        &[Val::U32(1)]
    );
    assert_eq!(
        call(&instance, &mut store, "alloc-in2", &[]).await.as_ref(),
        &[Val::U32(2)]
    );
}

/// An outer component that defines `r`, mints a handle for rep 42,
/// and hands that handle to a nested component as `borrow<r>`. The
/// nested component imports the type and the method, and its one
/// export forwards its borrow straight back through the method,
/// which lands in the defining instance and so receives the rep. The
/// nested component drops the borrow entry its own table holds before
/// it returns, as a callee that is not the definer must.
///
/// `run` takes a flag: when it is set the method drops the owning
/// handle while the borrow is still out, which must trap.
///
/// `forward-rep` mints the same handle and then forwards the rep 42
/// rather than its index as the `borrow<r>`. A definer receives its own
/// resource's rep from a lower, but its table, like every other, holds
/// handles by index, so the forward names index 42, where nothing is.
const OUTER_DEFINES_INNER_BORROWS: &[u8] = component!(
    r#"
    (component
      (type $r (resource (rep i32)))
      (core func $new (canon resource.new $r))
      (core func $drop (canon resource.drop $r))
      (core module $method-m
        (import "" "drop" (func $drop (param i32)))
        (global $owner (mut i32) (i32.const 0))
        (global $drop-owner (mut i32) (i32.const 0))
        (func (export "rep") (param $rep i32) (result i32)
          (if (global.get $drop-owner)
            (then global.get $owner call $drop))
          local.get $rep)
        (func (export "arm") (param i32 i32)
          local.get 0 global.set $owner
          local.get 1 global.set $drop-owner))
      (core instance $method-i (instantiate $method-m
        (with "" (instance (export "drop" (func $drop))))))
      (func $method (param "self" (borrow $r)) (result u32)
        (canon lift (core func $method-i "rep")))
      (component $inner
        (import "r" (type $r (sub resource)))
        (import "[method]r.rep" (func $method (param "self" (borrow $r)) (result u32)))
        (core func $method' (canon lower (func $method)))
        (core func $drop (canon resource.drop $r))
        (core module $m
          (import "" "rep" (func $rep (param i32) (result i32)))
          (import "" "drop" (func $drop (param i32)))
          (func (export "forward") (param i32) (result i32)
            (local $seen i32)
            local.get 0 call $rep local.set $seen
            local.get 0 call $drop
            local.get $seen))
        (core instance $i (instantiate $m
          (with "" (instance
            (export "rep" (func $method'))
            (export "drop" (func $drop))))))
        (func (export "forward") (param "self" (borrow $r)) (result u32)
          (canon lift (core func $i "forward"))))
      (instance $in (instantiate $inner
        (with "r" (type $r))
        (with "[method]r.rep" (func $method))))
      (core func $forward (canon lower (func $in "forward")))
      (core module $main
        (import "" "new" (func $new (param i32) (result i32)))
        (import "" "drop" (func $drop (param i32)))
        (import "" "forward" (func $forward (param i32) (result i32)))
        (import "" "arm" (func $arm (param i32 i32)))
        (func (export "run") (param $drop-owner i32) (result i32)
          (local $handle i32) (local $seen i32)
          i32.const 42 call $new local.set $handle
          local.get $handle local.get $drop-owner call $arm
          local.get $handle call $forward local.set $seen
          local.get $handle call $drop
          local.get $seen)
        (func (export "forward-rep") (result i32)
          i32.const 42 call $new drop
          i32.const 42 call $forward))
      (core instance $main-i (instantiate $main
        (with "" (instance
          (export "new" (func $new))
          (export "drop" (func $drop))
          (export "forward" (func $forward))
          (export "arm" (func $method-i "arm"))))))
      (export $r' "r" (type $r))
      (func (export "run") (param "drop-owner" u32) (result u32)
        (canon lift (core func $main-i "run")))
      (func (export "forward-rep") (result u32)
        (canon lift (core func $main-i "forward-rep"))))
    "#
);

#[wcmp_macros::test]
async fn it_traps_a_definer_that_forwards_a_bare_rep_as_a_borrow() {
    // The reference's `lower_borrow` hands the definer the rep, and its
    // `lift_borrow` reads the definer's table by index, so a rep passed
    // on as a borrow names an index the table never gave out.
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, OUTER_DEFINES_INNER_BORROWS)
        .await
        .expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let err = instance
        .get_func("forward-rep")
        .expect("forward-rep export")
        .call(&mut store, &[])
        .await
        .expect_err("the rep names no handle of the definer's table");
    assert!(
        err.to_string().contains("unknown handle index 42")
            || format!("{err:?}").contains("unknown handle index 42"),
        "the forward fails with the unknown-handle trap for index 42, got {err:?}"
    );
}

#[wcmp_macros::test]
async fn it_lifts_a_borrow_out_of_the_defining_instance_through_its_table() {
    // The defining instance addresses its own handles by table index,
    // so the borrow it hands the nested component has to be looked up
    // rather than taken for a rep: `resource.new` put rep 42 at index
    // 1, and the two differ. The nested component sends the borrow
    // straight back through the method, whose lower lands in the
    // defining instance and therefore does pass the rep. A method
    // that sees 1 read the caller's index as a rep.
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, OUTER_DEFINES_INNER_BORROWS)
        .await
        .expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let run = instance.get_func("run").expect("run export");
    let seen = run
        .call(&mut store, &[Val::U32(0)])
        .await
        .expect("the borrow crosses into the nested component and back");
    assert_eq!(
        seen.as_ref(),
        &[Val::U32(42)],
        "the method saw the resource's rep, not the owner's table index"
    );
}

#[wcmp_macros::test]
async fn it_lends_the_owning_handle_a_borrow_leaves_the_defining_instance_on() {
    // The same shape, with the method dropping the owning handle
    // while the borrow it was given is still out. The lift of the
    // borrow lent the owner to the call, so the drop must trap; the
    // drop that follows the call in `run` is never reached.
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, OUTER_DEFINES_INNER_BORROWS)
        .await
        .expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let run = instance.get_func("run").expect("run export");
    let err = run
        .call(&mut store, &[Val::U32(1)])
        .await
        .expect_err("the owning handle is lent out for the duration of the call");
    let text = format!("{err:?}");
    assert!(
        text.contains("cannot remove owned resource while borrowed"),
        "expected the borrowed-resource cause, got {text}"
    );
}

// ----------------------------------------------------------------
// Disposal: releasing what the host holds.
// ----------------------------------------------------------------

/// A store whose host resource `thing` records every destructor run
/// in the host data, plus the identity to mint with.
async fn disposal_store() -> (Store<Vec<u32>>, ResourceTypeId, crate::Instance) {
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, BORROWER)
        .await
        .expect("component parses");
    let mut linker: Linker<Vec<u32>> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd014-tests:host/things@0.1.0"
        .parse()
        .expect("identifier");
    let type_id = linker
        .instance(&iface)
        .resource(
            "thing",
            |dropped: &mut Vec<u32>, rep: u32| -> crate::Result<()> {
                dropped.push(rep);
                Ok(())
            },
        )
        .expect("the registration");
    linker
        .instance(&iface)
        .func_new(
            "rep",
            FunctionType {
                parameters: vec![FunctionParameter {
                    name: "h".to_owned(),
                    ty: ValueType::Borrow(ResourceType::new("thing")),
                }],
                result: Some(ValueType::Primitive(crate::PrimitiveType::U32)),
                async_: false,
            },
            |_: HostCall<'_, Vec<u32>>, args, results| {
                let Val::Borrow(handle) = &args[0] else {
                    panic!("expected a borrowed handle, got {args:?}");
                };
                results[0] = Val::U32(handle.rep());
                Ok(())
            },
        )
        .expect("the registration");
    let mut store: Store<Vec<u32>> = Store::new(&engine, Vec::new()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    (store, type_id, instance)
}

#[wcmp_macros::test]
async fn it_releases_a_host_held_handle_and_runs_its_destructor_once() {
    let (mut store, type_id, _instance) = disposal_store().await;
    let handle = store.resource_new(type_id, 31).expect("mint");
    store.resource_drop(handle).expect("release");
    assert_eq!(
        store.data(),
        &vec![31],
        "the destructor ran once with the rep"
    );
    let next = store.resource_new(type_id, 32).expect("mint again");
    assert_eq!(next.index(), handle.index(), "the freed slot is reused");
}

#[wcmp_macros::test]
async fn it_refuses_to_release_a_handle_twice() {
    let (mut store, type_id, _instance) = disposal_store().await;
    let handle = store.resource_new(type_id, 5).expect("mint");
    store.resource_drop(handle).expect("first release");
    let err = store
        .resource_drop(handle)
        .expect_err("a released handle is not live");
    assert!(
        matches!(&err, Error::Abi(abi) if matches!(abi.cause, AbiCause::InvalidHandle { .. })),
        "expected the invalid-handle cause, got {err:?}"
    );
    assert_eq!(store.data(), &vec![5], "the destructor did not run again");
}

#[wcmp_macros::test]
async fn it_names_the_resource_type_when_it_refuses_a_released_handle() {
    let (mut store, type_id, _instance) = disposal_store().await;
    let handle = store.resource_new(type_id, 5).expect("mint");
    store.resource_drop(handle).expect("first release");
    let err = store
        .resource_drop(handle)
        .expect_err("a released handle is not live");
    let Error::Abi(abi) = &err else {
        panic!("expected a canonical-ABI error, got {err:?}");
    };
    assert_eq!(
        abi.valtype.as_ref().and_then(|valtype| match valtype {
            ValueType::Own(resource) => Some(resource.label()),
            _ => None,
        }),
        Some("thing"),
        "the refusal processes an own handle, so it names the resource type \
         the component imported it under, got {err}"
    );
    assert!(
        err.to_string().contains("label: \"thing\""),
        "the rendered message names that type too, got {err}"
    );
}

#[wcmp_macros::test]
async fn it_releases_a_locally_defined_resource_through_the_store() {
    let (mut store, instance) = local_resource_instance().await;
    let handle = make_handle(&mut store, &instance, 9).await;
    store.resource_drop(handle).expect("release");
    let dropped = instance.get_func("dropped").expect("dropped export");
    let last = instance.get_func("last").expect("last export");
    assert_eq!(
        dropped
            .call(&mut store, &[])
            .await
            .expect("dropped")
            .as_ref(),
        &[Val::U32(1)],
        "the component's in-binary destructor ran once"
    );
    assert_eq!(
        last.call(&mut store, &[]).await.expect("last").as_ref(),
        &[Val::U32(9)]
    );
}

#[wcmp_macros::test]
async fn it_runs_no_destructor_when_a_store_is_dropped() {
    let (store, type_id, instance) = disposal_store().await;
    let _leaked = store.resource_new(type_id, 77).expect("mint");
    // The host data is the only record the destructor writes to; take
    // it out of the store before the store drops.
    drop(instance);
    let dropped = store.data().clone();
    drop(store);
    assert!(
        dropped.is_empty(),
        "a dropped store leaks live handles rather than running destructors"
    );
}

#[wcmp_macros::test]
async fn it_lets_an_instance_drop_before_its_store() {
    let (mut store, instance) = local_resource_instance().await;
    let handle = make_handle(&mut store, &instance, 4).await;
    drop(instance);
    // The store, its tables, and the destructor the dropped instance
    // introduced all outlive the instance handle.
    store
        .resource_drop(handle)
        .expect("a handle minted by a dropped instance still releases");
}

// ----------------------------------------------------------------
// Naming: which label an error about a handle carries.
// ----------------------------------------------------------------

/// Read the label of the `own<T>` an ABI error names, or `None` when
/// it names no value type at all.
fn owned_label(err: &Error) -> Option<String> {
    let Error::Abi(abi) = err else {
        panic!("expected a canonical-ABI error, got {err:?}");
    };
    match abi.valtype.as_ref() {
        Some(ValueType::Own(resource)) => Some(resource.label().to_owned()),
        Some(other) => panic!("expected an own handle, got {other:?}"),
        None => None,
    }
}

#[wcmp_macros::test]
async fn it_names_a_shared_resource_by_the_label_the_component_imported_it_under() {
    // One `HostResource` value registered against two labels is one
    // identity under two names, which PDD013's shared resource type
    // identity allows. The store keeps one of them: the label the
    // component itself imported the resource under, because an
    // instantiation teaches the store every name the component
    // brings in before it sweeps up the linker's remaining labels.
    //
    // `alias` sorts before `thing`, and the sweep visits labels in
    // sorted order, so a store that learned the sweep's names first
    // would render `alias` here.
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, BORROWER)
        .await
        .expect("component parses");
    let thing: HostResource<()> =
        HostResource::new(|_: &mut (), _: u32| -> crate::Result<()> { Ok(()) });
    let (mut linker, type_id) = borrower_linker(&engine, thing.clone());
    linker
        .root()
        .resource_with("alias", thing)
        .expect("the registration");

    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let _instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let handle = store.resource_new(type_id, 1).expect("mint");
    store.resource_drop(handle).expect("first release");
    let err = store
        .resource_drop(handle)
        .expect_err("a released handle is not live");

    assert_eq!(
        owned_label(&err).as_deref(),
        Some("thing"),
        "one identity under two labels renders the one the component \
         imported it under, got {err}"
    );
}

#[wcmp_macros::test]
async fn it_names_a_host_resource_no_component_imported() {
    // A resource registered against the linker that no component
    // ever brings in still has a label — the linker knows it — so
    // the store learns it at instantiation and an error about one of
    // its handles renders it rather than nothing.
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, BORROWER)
        .await
        .expect("component parses");
    let (mut linker, _thing) = borrower_linker(
        &engine,
        HostResource::new(|_: &mut (), _: u32| -> crate::Result<()> { Ok(()) }),
    );
    let gadget = linker
        .root()
        .resource("gadget", |_: &mut (), _: u32| -> crate::Result<()> {
            Ok(())
        })
        .expect("the registration");

    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let _instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let handle = store.resource_new(gadget, 1).expect("mint");
    store
        .resource_drop(handle)
        .expect("released; no instance of this store holds its destructor");
    let err = store
        .resource_drop(handle)
        .expect_err("a released handle is not live");

    assert_eq!(
        owned_label(&err).as_deref(),
        Some("gadget"),
        "a host resource the component never imported renders the label it \
         was registered under, got {err}"
    );
}

#[wcmp_macros::test]
async fn it_sweeps_no_label_for_a_component_that_imports_no_resource() {
    // The sweep is work proportional to the linker, so an
    // instantiation whose component imports no resource skips it.
    // What shows it skipped is the one thing the sweep teaches a
    // store: after such an instantiation, a handle of a resource the
    // linker holds renders no label, as it does in a store no
    // component has been instantiated into.
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, LOCAL_RESOURCE)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let gadget = linker
        .root()
        .resource("gadget", |_: &mut (), _: u32| -> crate::Result<()> {
            Ok(())
        })
        .expect("the registration");

    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let _instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate the component that imports nothing");
    let handle = store.resource_new(gadget, 1).expect("mint");
    store.resource_drop(handle).expect("first release");
    let err = store
        .resource_drop(handle)
        .expect_err("a released handle is not live");

    assert_eq!(
        owned_label(&err),
        None,
        "an instantiation of a component that imports no resource teaches \
         the store no label of the linker's, got {err}"
    );
}

#[wcmp_macros::test]
async fn it_names_a_shared_resource_by_the_importer_after_an_earlier_instantiation() {
    // The same identity under two labels as the test above, but the
    // store takes two components: one that imports nothing, then
    // `BORROWER`. The first instantiation sweeps nothing, because
    // its component imports no resource; the second sweeps the
    // linker and meets this identity as `alias`, a label no
    // component in the store ever uses, and brings it in as `thing`.
    // A swept label is only ever a fallback, so the importer's label
    // takes over and the error still reads `thing`.
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let quiet = Component::new(&engine, LOCAL_RESOURCE)
        .await
        .expect("component parses");
    let borrower = Component::new(&engine, BORROWER)
        .await
        .expect("component parses");
    let thing: HostResource<()> =
        HostResource::new(|_: &mut (), _: u32| -> crate::Result<()> { Ok(()) });
    let (mut linker, type_id) = borrower_linker(&engine, thing.clone());
    linker
        .root()
        .resource_with("alias", thing)
        .expect("the registration");

    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let _quiet_instance = linker
        .instantiate(&mut store, &quiet)
        .await
        .expect("instantiate the component that imports nothing");
    let _borrower_instance = linker
        .instantiate(&mut store, &borrower)
        .await
        .expect("instantiate the importer");
    let handle = store.resource_new(type_id, 1).expect("mint");
    store.resource_drop(handle).expect("first release");
    let err = store
        .resource_drop(handle)
        .expect_err("a released handle is not live");

    assert_eq!(
        owned_label(&err).as_deref(),
        Some("thing"),
        "an earlier instantiation's sweep does not fix the name of an \
         identity a later component imports under its own label, got {err}"
    );
}

#[wcmp_macros::test]
async fn it_names_an_unimported_resource_by_the_first_of_its_labels_in_order() {
    // One identity under two labels that no component in the store
    // imports: both reach the store as fallbacks from the sweep, so
    // the sweep's order is what decides between them. The linker
    // holds its interfaces in a hash order, which must not reach a
    // name a user reads, so the sweep sorts the labels and the store
    // keeps the first. `gizmo` sorts before `widget`, and it is the
    // root registration that holds `widget` here, so nothing but the
    // sort can be producing the answer.
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, BORROWER)
        .await
        .expect("component parses");
    let (mut linker, _thing) = borrower_linker(
        &engine,
        HostResource::new(|_: &mut (), _: u32| -> crate::Result<()> { Ok(()) }),
    );
    let spare: HostResource<()> =
        HostResource::new(|_: &mut (), _: u32| -> crate::Result<()> { Ok(()) });
    let widget = linker
        .root()
        .resource_with("widget", spare.clone())
        .expect("the registration");
    let other: InterfaceIdentifier = "pdd014-tests:host/gizmos@0.1.0"
        .parse()
        .expect("identifier");
    let gizmo = linker
        .instance(&other)
        .resource_with("gizmo", spare)
        .expect("the registration");
    assert_eq!(widget, gizmo, "one host resource value is one identity");

    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let _instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let handle = store.resource_new(widget, 1).expect("mint");
    store
        .resource_drop(handle)
        .expect("released; no instance of this store holds its destructor");
    let err = store
        .resource_drop(handle)
        .expect_err("a released handle is not live");

    assert_eq!(
        owned_label(&err).as_deref(),
        Some("gizmo"),
        "an identity no component imported renders the first of its labels \
         in sorted order, whatever order the linker holds them in, got {err}"
    );
}

#[wcmp_macros::test]
async fn it_names_a_locally_defined_resource_when_it_refuses_a_released_handle() {
    // A resource the component defines has no host registration to
    // take a name from, so the name comes from the component's own
    // resource tables — the label it exports the type under.
    let (mut store, instance) = local_resource_instance().await;
    let handle = make_handle(&mut store, &instance, 9).await;
    store.resource_drop(handle).expect("first release");
    let err = store
        .resource_drop(handle)
        .expect_err("a released handle is not live");

    assert_eq!(
        owned_label(&err).as_deref(),
        Some("thing"),
        "a locally-defined resource renders the label the component \
         declares it under, got {err}"
    );
}

#[wcmp_macros::test]
async fn it_names_the_resource_type_a_refused_host_mint_asked_for() {
    // The mint entry point refuses an identity the calling instance
    // holds no table for. The refusal names that type, under the
    // name the store knows the identity by, exactly as the lower
    // path names the handle slot it refused.
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, MINTER)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd011-tests:host/things@0.1.0"
        .parse()
        .expect("identifier");
    linker
        .instance(&iface)
        .resource("thing", |_: &mut (), _: u32| -> crate::Result<()> {
            Ok(())
        })
        .expect("the registration");
    let gadget = linker
        .root()
        .resource("gadget", |_: &mut (), _: u32| -> crate::Result<()> {
            Ok(())
        })
        .expect("the registration");
    linker
        .instance(&iface)
        .func_new(
            "make",
            FunctionType {
                parameters: Vec::new(),
                result: Some(ValueType::Own(ResourceType::new("thing"))),
                async_: false,
            },
            move |call: HostCall<'_, ()>, _args, results| {
                // `gadget` is registered against the linker but is not a
                // resource type of the instance being served.
                results[0] = Val::Own(call.resource_new(gadget, 1)?);
                Ok(())
            },
        )
        .expect("the registration");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let run = instance.get_func("run").expect("run export");
    let err = run
        .call(&mut store, &[])
        .await
        .expect_err("minting against a type the instance does not hold fails");

    // The host error traps the guest, and the call gets it back as
    // the host function raised it.
    match &err {
        Error::Abi(abi) => {
            assert!(
                matches!(abi.cause, AbiCause::UnregisteredResourceType),
                "expected the unregistered-resource-type cause, got {err:?}"
            );
            assert_eq!(
                abi.valtype,
                Some(ValueType::Own(ResourceType::new("gadget"))),
                "the refusal names the resource type the mint asked for"
            );
        }
        other => panic!("expected the unregistered-resource-type cause, got {other:?}"),
    }
}

/// A component that defines a resource whose in-binary destructor
/// traps.
const TRAPPING_LOCAL_RESOURCE: &[u8] = component!(
    r#"
    (component
      (core module $d
        (func (export "dtor") (param i32) unreachable))
      (core instance $di (instantiate $d))
      (type $thing (resource (rep i32) (dtor (core func $di "dtor"))))
      (core func $new (canon resource.new $thing))
      (core module $m
        (import "" "new" (func $new (param i32) (result i32)))
        (func (export "make") (param i32) (result i32) local.get 0 call $new))
      (core instance $i (instantiate $m
        (with "" (instance (export "new" (func $new))))))
      (export $thing' "thing" (type $thing))
      (func (export "make") (param "rep" u32) (result (own $thing'))
        (canon lift (core func $i "make"))))
    "#
);

#[wcmp_macros::test]
async fn it_reports_a_substrate_failure_when_a_local_destructor_traps() {
    // Releasing a locally-defined resource through the store calls
    // the component's own destructor. A destructor that traps fails
    // the release with the substrate-failure cause, at the
    // destructor's one argument — the resource's rep, not the own
    // handle the host released, so the failure names no value type.
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, TRAPPING_LOCAL_RESOURCE)
        .await
        .expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let handle = make_handle(&mut store, &instance, 3).await;

    let err = store
        .resource_drop(handle)
        .expect_err("a destructor that traps fails the release");

    let Error::Abi(abi) = &err else {
        panic!("expected a canonical-ABI error, got {err:?}");
    };
    assert!(
        matches!(abi.cause, AbiCause::SubstrateFailure(_)),
        "expected the substrate-failure cause, got {err:?}"
    );
    assert_eq!(
        abi.position,
        AbiPosition::Argument(0),
        "the failing call is the destructor's, whose one argument is the rep"
    );
    assert!(
        abi.valtype.is_none(),
        "that argument is a core `u32`, not the own handle the host released, \
         so the failure names no value type, got {err}"
    );
}
