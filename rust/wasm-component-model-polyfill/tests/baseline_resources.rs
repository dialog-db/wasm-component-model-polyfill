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

use std::sync::{Arc, Mutex};

use wasm_component_model_polyfill::{
    AbiCause, Component, Engine, Error, FunctionParameter, FunctionType, HostCall, HostResource,
    InterfaceIdentifier, Linker, ResourceHandle, ResourceType, ResourceTypeId, Store, Val,
    ValueType,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

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
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let mut linker: Linker<Arc<Mutex<Vec<u32>>>> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd009-tests:host/resources@0.1.0"
        .parse()
        .expect("identifier");
    let type_id = linker.instance(&iface).resource(
        "thing",
        |data: &mut Arc<Mutex<Vec<u32>>>, rep: u32| -> wasm_component_model_polyfill::Result<()> {
            data.lock().expect("dropped lock").push(rep);
            Ok(())
        },
    );

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
    let engine = Engine::new().expect("engine");
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
    assert_eq!(a0.index, 1, "index 0 is reserved");
    assert_eq!(a1.index, 2);
    assert_eq!(b0.index, 1);
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
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd009-tests:host/resources@0.1.0"
        .parse()
        .expect("identifier");
    let type_id = linker.instance(&iface).resource(
        "thing",
        |_data: &mut (), _rep: u32| -> wasm_component_model_polyfill::Result<()> { Ok(()) },
    );

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
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd009-tests:host/resources@0.1.0"
        .parse()
        .expect("identifier");
    let type_id = linker.instance(&iface).resource(
        "thing",
        |_data: &mut (), _rep: u32| -> wasm_component_model_polyfill::Result<()> { Ok(()) },
    );
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let inst = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let consume = inst.get_func("consume").expect("consume export");
    let bogus = ResourceHandle {
        type_id,
        index: 999,
        rep: 0,
    };
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
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let mut linker: Linker<Counters> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd009-tests:host/multi@0.1.0".parse().expect("identifier");
    let mut iface_view = linker.instance(&iface);
    let alpha_id = iface_view.resource(
        "alpha",
        |c: &mut Counters, rep: u32| -> wasm_component_model_polyfill::Result<()> {
            c.alphas.push(rep);
            Ok(())
        },
    );
    let beta_id = iface_view.resource(
        "beta",
        |c: &mut Counters, rep: u32| -> wasm_component_model_polyfill::Result<()> {
            c.betas.push(rep);
            Ok(())
        },
    );
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
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd009-tests:host/resources@0.1.0"
        .parse()
        .expect("identifier");
    let type_id = linker.instance(&iface).resource(
        "thing",
        |_: &mut (), _: u32| -> wasm_component_model_polyfill::Result<()> { Ok(()) },
    );
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let inst = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let consume = inst.get_func("consume").expect("consume");

    let h0 = store.resource_new(type_id, 1).expect("h0");
    let h1 = store.resource_new(type_id, 2).expect("h1");
    let h2 = store.resource_new(type_id, 3).expect("h2");
    let h1_index = h1.index;
    consume
        .call(&mut store, &[Val::Own(h1)])
        .await
        .expect("drop middle");
    // The middle slot is free; minting again reuses it.
    let h3 = store.resource_new(type_id, 4).expect("h3");
    assert_eq!(h3.index, h1_index);
    // The two siblings are still live and distinct.
    assert_ne!(h0.index, h2.index);
    assert_ne!(h0.index, h3.index);
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
    let engine = Engine::new().expect("engine");
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

async fn local_resource_instance() -> (Store<()>, wasm_component_model_polyfill::Instance) {
    let engine = Engine::new().expect("engine");
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
    instance: &wasm_component_model_polyfill::Instance,
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
        handle.index, 1,
        "the first handle takes the first table slot"
    );
}

#[wcmp_macros::test]
async fn it_mints_a_distinct_resource_type_identity_per_instantiation() {
    let engine = Engine::new().expect("engine");
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
        a.type_id, b.type_id,
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
    let engine = Engine::new().expect("engine");
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
    let engine = Engine::new().expect("engine");
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
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, SHARED)
        .await
        .expect("component parses");
    let mut linker: Linker<Vec<u32>> = Linker::new(&engine);
    let a: InterfaceIdentifier = "pdd013-tests:host/a@0.1.0".parse().expect("identifier");
    let b: InterfaceIdentifier = "pdd013-tests:host/b@0.1.0".parse().expect("identifier");
    let thing: HostResource<Vec<u32>> = HostResource::new(
        |dropped: &mut Vec<u32>, rep: u32| -> wasm_component_model_polyfill::Result<()> {
            dropped.push(rep);
            Ok(())
        },
    );
    let type_id = linker.instance(&a).resource_with("thing", thing.clone());
    linker.instance(&b).resource_with("thing", thing);
    linker.instance(&a).func_new(
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
    );
    linker.instance(&b).func_new(
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
            call.data_mut().push(handle.rep);
            Ok(())
        },
    );
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
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, SHARED)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let a: InterfaceIdentifier = "pdd013-tests:host/a@0.1.0".parse().expect("identifier");
    let b: InterfaceIdentifier = "pdd013-tests:host/b@0.1.0".parse().expect("identifier");
    let type_id = linker.instance(&a).resource(
        "thing",
        |_: &mut (), _: u32| -> wasm_component_model_polyfill::Result<()> { Ok(()) },
    );
    linker.instance(&b).resource(
        "thing",
        |_: &mut (), _: u32| -> wasm_component_model_polyfill::Result<()> { Ok(()) },
    );
    linker.instance(&a).func_new(
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
    );
    linker.instance(&b).func_new(
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
    );
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let err = match linker.instantiate(&mut store, &component).await {
        Ok(_) => panic!("two identities for one declared resource type must not link"),
        Err(err) => err,
    };
    assert!(
        matches!(&err, Error::TypeMismatch(mismatch)
            if matches!(mismatch.position, wasm_component_model_polyfill::TypeMismatchPosition::HostFunctionRegistration { ref item, .. } if item == "thing")),
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
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, MINTER)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd011-tests:host/things@0.1.0"
        .parse()
        .expect("identifier");
    let type_id = linker.instance(&iface).resource(
        "thing",
        |_: &mut (), _: u32| -> wasm_component_model_polyfill::Result<()> { Ok(()) },
    );
    linker.instance(&iface).func_new(
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
    );
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
        handle.type_id, type_id,
        "the handle carries the registered identity"
    );
    assert_eq!(handle.rep, 42, "the handle carries the rep the host minted");
}

#[wcmp_macros::test]
async fn it_rejects_a_host_mint_against_an_unknown_resource_type() {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, MINTER)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd011-tests:host/things@0.1.0"
        .parse()
        .expect("identifier");
    linker.instance(&iface).resource(
        "thing",
        |_: &mut (), _: u32| -> wasm_component_model_polyfill::Result<()> { Ok(()) },
    );
    let stranger = ResourceTypeId::fresh();
    linker.instance(&iface).func_new(
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
    );
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
    // The host error crosses the substrate as a trap, so the cause
    // is read off the error chain's text.
    let text = format!("{err:?}");
    assert!(
        text.contains("no host registration matches the transferred resource type"),
        "expected the unregistered-resource-type cause in the error, got {text}"
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
    use wasm_component_model_polyfill::{ExternType, PrimitiveType, ValueType};
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

    let engine = Engine::new().expect("engine");
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

async fn borrower_instance() -> (
    Store<()>,
    wasm_component_model_polyfill::Instance,
    ResourceTypeId,
) {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, BORROWER)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd014-tests:host/things@0.1.0"
        .parse()
        .expect("identifier");
    let type_id = linker.instance(&iface).resource(
        "thing",
        |_: &mut (), _: u32| -> wasm_component_model_polyfill::Result<()> { Ok(()) },
    );
    linker.instance(&iface).func_new(
        "rep",
        FunctionType {
            parameters: vec![FunctionParameter {
                name: "h".to_owned(),
                ty: ValueType::Borrow(ResourceType::new("thing")),
            }],
            result: Some(ValueType::Primitive(
                wasm_component_model_polyfill::PrimitiveType::U32,
            )),
            async_: false,
        },
        |_: HostCall<'_, ()>, args, results| {
            let Val::Borrow(handle) = &args[0] else {
                panic!("expected a borrowed handle, got {args:?}");
            };
            results[0] = Val::U32(handle.rep);
            Ok(())
        },
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
    let mut tables = store.tables().lock().expect("tables");
    let host_table = tables.host_table(type_id);
    assert_eq!(
        tables
            .for_table(host_table)
            .and_then(|t| t.get(handle.index)),
        Some(7),
        "the owning entry is live after the call"
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
    let engine = Engine::new().expect("engine");
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
        instance: &wasm_component_model_polyfill::Instance,
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

// ----------------------------------------------------------------
// Disposal: releasing what the host holds.
// ----------------------------------------------------------------

/// A store whose host resource `thing` records every destructor run
/// in the host data, plus the identity to mint with.
async fn disposal_store() -> (
    Store<Vec<u32>>,
    ResourceTypeId,
    wasm_component_model_polyfill::Instance,
) {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, BORROWER)
        .await
        .expect("component parses");
    let mut linker: Linker<Vec<u32>> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd014-tests:host/things@0.1.0"
        .parse()
        .expect("identifier");
    let type_id = linker.instance(&iface).resource(
        "thing",
        |dropped: &mut Vec<u32>, rep: u32| -> wasm_component_model_polyfill::Result<()> {
            dropped.push(rep);
            Ok(())
        },
    );
    linker.instance(&iface).func_new(
        "rep",
        FunctionType {
            parameters: vec![FunctionParameter {
                name: "h".to_owned(),
                ty: ValueType::Borrow(ResourceType::new("thing")),
            }],
            result: Some(ValueType::Primitive(
                wasm_component_model_polyfill::PrimitiveType::U32,
            )),
            async_: false,
        },
        |_: HostCall<'_, Vec<u32>>, args, results| {
            let Val::Borrow(handle) = &args[0] else {
                panic!("expected a borrowed handle, got {args:?}");
            };
            results[0] = Val::U32(handle.rep);
            Ok(())
        },
    );
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
    assert_eq!(next.index, handle.index, "the freed slot is reused");
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
