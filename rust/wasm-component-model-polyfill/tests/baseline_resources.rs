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
    AbiCause, Component, Engine, Error, InterfaceIdentifier, Linker, ResourceHandle,
    ResourceTypeId, Store, Val,
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
    let component = Component::new(&engine, COMPONENT).expect("component parses");
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
        .expect("instantiate");
    let drop2 = inst.get_func("drop2").expect("drop2 export");
    let h1 = store.resource_new(type_id, 1).expect("mint h1");
    let h2 = store.resource_new(type_id, 2).expect("mint h2");
    drop2
        .call(&mut store, &[Val::Own(h1), Val::Own(h2)])
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
    assert_eq!(a0.index, 0);
    assert_eq!(a1.index, 1);
    assert_eq!(b0.index, 0);
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
    let component = Component::new(&engine, COMPONENT).expect("component parses");
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
        .expect("instantiate");
    let consume = inst.get_func("consume").expect("consume export");
    let outcome = consume.call(&mut consumer, &[Val::Own(foreign_handle)]);
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
    let component = Component::new(&engine, COMPONENT).expect("component parses");
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
        .expect("instantiate");
    let consume = inst.get_func("consume").expect("consume export");
    let bogus = ResourceHandle {
        type_id,
        index: 999,
        rep: 0,
    };
    let outcome = consume.call(&mut store, &[Val::Own(bogus)]);
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
    let component = Component::new(&engine, COMPONENT).expect("component parses");
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
        .expect("instantiate");
    let drop_alpha = inst.get_func("drop-alpha").expect("drop-alpha");
    let drop_beta = inst.get_func("drop-beta").expect("drop-beta");
    let a = store.resource_new(alpha_id, 11).expect("mint alpha");
    let b = store.resource_new(beta_id, 22).expect("mint beta");
    drop_alpha
        .call(&mut store, &[Val::Own(a)])
        .expect("call drop-alpha");
    drop_beta
        .call(&mut store, &[Val::Own(b)])
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
    let component = Component::new(&engine, COMPONENT).expect("component parses");
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
        .expect("instantiate");
    let consume = inst.get_func("consume").expect("consume");

    let h0 = store.resource_new(type_id, 1).expect("h0");
    let h1 = store.resource_new(type_id, 2).expect("h1");
    let h2 = store.resource_new(type_id, 3).expect("h2");
    let h1_index = h1.index;
    consume
        .call(&mut store, &[Val::Own(h1)])
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
    let component = Component::new(&engine, COMPONENT).expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    // Register the interface but no `thing` resource.
    let iface: InterfaceIdentifier = "pdd009-tests:host/resources@0.1.0"
        .parse()
        .expect("identifier");
    let _ = linker.instance(&iface);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let outcome = linker.instantiate(&mut store, &component);
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

fn local_resource_instance() -> (Store<()>, wasm_component_model_polyfill::Instance) {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, LOCAL_RESOURCE).expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .expect("instantiate");
    (store, instance)
}

fn make_handle(
    store: &mut Store<()>,
    instance: &wasm_component_model_polyfill::Instance,
    rep: u32,
) -> ResourceHandle {
    let make = instance.get_func("make").expect("make export");
    let results = make.call(store, &[Val::U32(rep)]).expect("make call");
    match results.as_ref() {
        [Val::Own(handle)] => *handle,
        other => panic!("expected an owned handle, got {other:?}"),
    }
}

#[wcmp_macros::test]
async fn it_translates_and_instantiates_a_locally_defined_resource() {
    let (mut store, instance) = local_resource_instance();
    let handle = make_handle(&mut store, &instance, 7);
    assert_eq!(
        handle.index, 0,
        "the first handle takes the first table slot"
    );
}

#[wcmp_macros::test]
async fn it_mints_a_distinct_resource_type_identity_per_instantiation() {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, LOCAL_RESOURCE).expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let first = linker
        .instantiate(&mut store, &component)
        .expect("first instantiation");
    let second = linker
        .instantiate(&mut store, &component)
        .expect("second instantiation");
    let a = make_handle(&mut store, &first, 1);
    let b = make_handle(&mut store, &second, 2);
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
    let (mut store, instance) = local_resource_instance();
    let handle = make_handle(&mut store, &instance, 7);
    let dispose = instance.get_func("dispose").expect("dispose export");
    dispose
        .call(&mut store, &[Val::Own(handle)])
        .expect("dispose call");
    let dropped = instance.get_func("dropped").expect("dropped export");
    let last = instance.get_func("last").expect("last export");
    assert_eq!(
        dropped.call(&mut store, &[]).expect("dropped").as_ref(),
        &[Val::U32(1)],
        "the in-binary destructor ran when the guest dropped the handle"
    );
    assert_eq!(
        last.call(&mut store, &[]).expect("last").as_ref(),
        &[Val::U32(7)],
        "the destructor received the dropped entry's rep"
    );
}

#[wcmp_macros::test]
async fn it_runs_the_in_binary_destructor_exactly_once_per_dropped_handle() {
    let (mut store, instance) = local_resource_instance();
    let dispose = instance.get_func("dispose").expect("dispose export");
    let dropped = instance.get_func("dropped").expect("dropped export");
    let kept = make_handle(&mut store, &instance, 1);
    for rep in [2, 3] {
        let handle = make_handle(&mut store, &instance, rep);
        dispose
            .call(&mut store, &[Val::Own(handle)])
            .expect("dispose call");
    }
    assert_eq!(
        dropped.call(&mut store, &[]).expect("dropped").as_ref(),
        &[Val::U32(2)],
        "two handles dropped, two destructor runs; the kept handle ran none"
    );
    let _ = kept;
}

#[wcmp_macros::test]
async fn it_rejects_a_handle_from_another_instance_of_the_same_component() {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, LOCAL_RESOURCE).expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let first = linker
        .instantiate(&mut store, &component)
        .expect("first instantiation");
    let second = linker
        .instantiate(&mut store, &component)
        .expect("second instantiation");
    let handle = make_handle(&mut store, &first, 1);
    let dispose = second.get_func("dispose").expect("dispose export");
    let err = dispose
        .call(&mut store, &[Val::Own(handle)])
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
    let outcome = Component::new(&engine, COMPONENT).and_then(|component| {
        let linker: Linker<()> = Linker::new(&engine);
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");
        linker.instantiate(&mut store, &component).map(|_| ())
    });
    let err = outcome.expect_err("a destructor that returns a value is refused");
    assert!(
        matches!(
            &err,
            Error::Instantiation(_) | Error::InvalidComponentBinary { .. }
        ),
        "expected an instantiation or validation error, got {err:?}"
    );
}

#[wcmp_macros::test]
#[ignore = "stub: multi-level resource sharing — a single resource type imported by two interfaces and used in both"]
async fn it_shares_a_single_resource_type_across_two_imported_interfaces() {
    todo!(
        "import the same resource type via two separate interface imports (mirrors wasm_component_layer's multilevel_resource example) and assert handles minted under one interface lower correctly through the other"
    );
}

#[wcmp_macros::test]
#[ignore = "stub: the polyfill's host-function callback signature `Fn(&mut T, &[Val], &mut [Val])` does not give the host access to the per-store handle tables, so a host function cannot mint a fresh resource handle from inside a guest call (wasm_component_layer's `Func::new` passes a `StoreContextMut` that does)"]
async fn it_lets_a_host_function_mint_a_resource_handle_during_a_guest_call() {
    todo!(
        "register a host function that returns `own<thing>`, where the function body mints a fresh handle from within the closure; today the closure has no path to `Store::resource_new`"
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
    let component = Component::new(&engine, COMPONENT).expect("component parses");
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

#[wcmp_macros::test]
#[ignore = "stub: typed strongly-shaped host resources — wasm_component_layer's `ResourceType::new::<T>(name)` keys the resource type by Rust `TypeId` and rejects mismatched reps at registration time"]
async fn it_rejects_a_destructor_registered_against_a_mismatched_rust_type() {
    todo!(
        "the polyfill currently passes `u32` reps; if and when we adopt a strongly-typed resource API, this test should fail to compile or fail at registration when the destructor's parameter type disagrees with the registration type"
    );
}

#[wcmp_macros::test]
#[ignore = "stub: borrow lifetime tracking at host-call return — `AbiCause::OutstandingBorrows` is reserved but not yet emitted"]
async fn it_rejects_a_host_call_that_leaves_outstanding_borrows() {
    todo!(
        "the canonical ABI's runtime-state rules require that no `borrow<T>` lifted in for the call remain live at return; the polyfill defers tracking this counter until borrows are exercised by a test that requires the enforcement"
    );
}
