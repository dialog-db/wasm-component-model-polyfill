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
    Component, Engine, Error, InterfaceIdentifier, Linker, ResourceHandle, ResourceTypeId, Store,
    Val,
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
    let drop2 = inst.get_func(&mut store, "drop2").expect("drop2 export");
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
    let consume = inst
        .get_func(&mut consumer, "consume")
        .expect("consume export");
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
    let consume = inst.get_func(&mut store, "consume").expect("consume export");
    let bogus = ResourceHandle {
        type_id,
        index: 999,
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
    let iface: InterfaceIdentifier = "pdd009-tests:host/multi@0.1.0"
        .parse()
        .expect("identifier");
    let mut iface_view = linker.instance(&iface);
    let alpha_id =
        iface_view.resource(
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
    let drop_alpha = inst.get_func(&mut store, "drop-alpha").expect("drop-alpha");
    let drop_beta = inst.get_func(&mut store, "drop-beta").expect("drop-beta");
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
    let consume = inst.get_func(&mut store, "consume").expect("consume");

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

// ----------------------------------------------------------------
// Stubs: capabilities the polyfill does not yet realise.
// ----------------------------------------------------------------

#[wcmp_macros::test]
#[ignore = "stub: locally-defined resources (`(type (resource (rep i32) (dtor (func $f))))`) — `GlobalInitializer::Resource` is currently `todo!()`"]
async fn it_supports_a_locally_defined_resource_with_an_in_binary_destructor() {
    todo!(
        "load a component that defines its own resource with a destructor pointing at a core func, instantiate it, and assert the in-binary destructor runs synchronously when the handle is dropped"
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
#[ignore = "stub: `[constructor]X` / `[method]X.fn` shaped exports — the resource constructor/method shape uses a name-mangling convention the polyfill does not yet route specially"]
async fn it_supports_resource_constructor_and_method_shaped_exports() {
    todo!(
        "guest-defined resource exposed as `[constructor]bar` and `[method]bar.value` (mirrors WCL's guest_resource example); the polyfill currently surfaces these as plain functions but does not validate their constructor/method semantics"
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
