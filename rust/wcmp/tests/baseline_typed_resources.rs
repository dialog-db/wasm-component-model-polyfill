// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Baseline tests for the resource handles: `Resource<T>`,
//! `ResourceDynamic`, and `ResourceAny`.
//!
//! A typed handle crosses where the component declares an `own` or a
//! `borrow` of its identity. An owned handle moves into the guest the
//! first time it lowers as an `own`, and a second lowering fails. A
//! `ResourceAny` names an entry of the store's host table, crosses
//! wherever the component declares a handle, and checks the identity
//! when it lowers.

#![cfg(test)]

use std::sync::Arc;

use wcmp::{
    AbiCause, Component, Engine, Error, HostCall, Instance, InterfaceIdentifier, Linker, Resource,
    ResourceAny, ResourceDynamic, ResourceType, Store, StoreContext, TypedFunc, ValueType,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// The interface the guest of [`RESOURCES`] imports.
const HOST: &str = "wcmp-tests:resources/host@0.1.0";

/// The marker of the host resource type `thing`.
struct Thing;

/// A marker that no component imports.
struct Other;

/// A guest that imports the host resource `thing` and defines a
/// resource `doc` of its own.
///
/// - `keep` hands the host's `inspect` a borrow of the `thing` it
///   receives, then drops it.
/// - `peek` does the same with a borrow of a `thing`.
/// - `echo` returns the `thing` it receives.
/// - `make-doc` makes a `doc` of the next rep, from 1.
/// - `take-doc` returns the rep of the `doc` it receives, then drops
///   it.
///
/// The destructor of `doc` hands the host's `dropped` the rep.
const RESOURCES: &[u8] = component!(
    r#"
    (component
      (import "wcmp-tests:resources/host@0.1.0" (instance $host
        (export "thing" (type $thing (sub resource)))
        (export "inspect" (func (param "h" (borrow $thing))))
        (export "dropped" (func (param "rep" u32)))))
      (alias export $host "thing" (type $thing))
      (alias export $host "inspect" (func $inspect))
      (alias export $host "dropped" (func $dropped))
      (core func $inspect-lower (canon lower (func $inspect)))
      (core func $dropped-lower (canon lower (func $dropped)))
      (core func $thing-drop (canon resource.drop $thing))
      (core module $d
        (import "host" "dropped" (func $dropped (param i32)))
        (func (export "dtor") (param i32) local.get 0 call $dropped))
      (core instance $di (instantiate $d
        (with "host" (instance (export "dropped" (func $dropped-lower))))))
      (type $doc (resource (rep i32) (dtor (core func $di "dtor"))))
      (core func $doc-new (canon resource.new $doc))
      (core func $doc-rep (canon resource.rep $doc))
      (core func $doc-drop (canon resource.drop $doc))
      (core module $m
        (import "host" "inspect" (func $inspect (param i32)))
        (import "host" "thing-drop" (func $thing-drop (param i32)))
        (import "host" "doc-new" (func $doc-new (param i32) (result i32)))
        (import "host" "doc-rep" (func $doc-rep (param i32) (result i32)))
        (import "host" "doc-drop" (func $doc-drop (param i32)))
        (global $next (mut i32) (i32.const 1))
        (func (export "keep") (param i32)
          local.get 0 call $inspect
          local.get 0 call $thing-drop)
        (func (export "echo") (param i32) (result i32) local.get 0)
        (func (export "make-doc") (result i32)
          global.get $next call $doc-new
          global.get $next i32.const 1 i32.add global.set $next)
        (func (export "take-doc") (param i32) (result i32)
          (local $rep i32)
          local.get 0 call $doc-rep local.set $rep
          local.get 0 call $doc-drop
          local.get $rep))
      (core instance $c (instantiate $m
        (with "host" (instance
          (export "inspect" (func $inspect-lower))
          (export "thing-drop" (func $thing-drop))
          (export "doc-new" (func $doc-new))
          (export "doc-rep" (func $doc-rep))
          (export "doc-drop" (func $doc-drop))))))
      (export $doc' "doc" (type $doc))
      (func (export "keep") (param "h" (own $thing))
        (canon lift (core func $c "keep")))
      (func (export "peek") (param "h" (borrow $thing))
        (canon lift (core func $c "keep")))
      (func (export "echo") (param "h" (own $thing)) (result (own $thing))
        (canon lift (core func $c "echo")))
      (func (export "make-doc") (result (own $doc'))
        (canon lift (core func $c "make-doc")))
      (func (export "take-doc") (param "h" (own $doc')) (result u32)
        (canon lift (core func $c "take-doc"))))
    "#
);

/// What the host functions of [`RESOURCES`] saw: the reps `inspect`
/// received, the reps the `thing` destructor received, and the reps
/// `dropped` received from the `doc` destructor.
#[derive(Default)]
struct Seen {
    inspected: Vec<u32>,
    things_dropped: Vec<u32>,
    docs_dropped: Vec<u32>,
}

/// Instantiate [`RESOURCES`] with `thing` registered as
/// [`ResourceType::host::<Thing>()`] and typed host functions.
async fn instance() -> (Store<Seen>, Instance) {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, RESOURCES)
        .await
        .expect("the component parses");
    let mut linker: Linker<Seen> = Linker::new(&engine);
    let host: InterfaceIdentifier = HOST.parse().expect("identifier");
    linker
        .instance(&host)
        .resource(
            "thing",
            ResourceType::host::<Thing>(),
            |mut store: StoreContext<'_, Seen>, rep| -> wcmp::Result<()> {
                store.data_mut().things_dropped.push(rep);
                Ok(())
            },
        )
        .expect("the registration of `thing`");
    linker
        .instance(&host)
        .func_wrap(
            "inspect",
            |mut call: HostCall<'_, Seen>, (thing,): (Resource<Thing>,)| {
                assert!(!thing.owned(), "a `borrow` parameter lifts as a borrow");
                call.data_mut().inspected.push(thing.rep());
                Ok(())
            },
        )
        .expect("the registration of `inspect`");
    linker
        .instance(&host)
        .func_wrap("dropped", |mut call: HostCall<'_, Seen>, (rep,): (u32,)| {
            call.data_mut().docs_dropped.push(rep);
            Ok(())
        })
        .expect("the registration of `dropped`");
    let mut store = Store::new(&engine, Seen::default()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the component links");
    (store, instance)
}

/// The typed handle of the export `name`.
fn typed<P, R>(instance: &Instance, name: &str) -> TypedFunc<P, R>
where
    P: wcmp::ComponentNamedList + wcmp::Lower,
    R: wcmp::ComponentNamedList + wcmp::Lift,
{
    instance
        .get_func(name)
        .expect("the export")
        .typed::<P, R>()
        .expect("the export has the typed signature")
}

/// The reason of an invalid-handle ABI failure.
fn invalid_handle(error: &Error) -> &str {
    match error {
        Error::Abi(abi) => match &abi.cause {
            AbiCause::InvalidHandle { reason } => reason,
            other => panic!("expected the invalid-handle cause, got {other:?}"),
        },
        other => panic!("expected an ABI failure, got {other:?}"),
    }
}

#[wcmp_macros::test]
async fn it_crosses_a_resource_as_a_typed_handle_and_refuses_a_second_own() {
    let (mut store, instance) = instance().await;
    // The handle is shared so that the test can lower it twice.
    let keep = typed::<(Arc<Resource<Thing>>,), ()>(&instance, "keep");
    let thing = Arc::new(Resource::<Thing>::new_own(5));
    assert!(thing.owned());
    keep.call(&mut store, (thing.clone(),))
        .await
        .expect("the guest takes the handle");
    assert_eq!(store.data().inspected, vec![5], "the guest's borrow");
    assert_eq!(store.data().things_dropped, vec![5], "the guest's drop");

    let refused = keep
        .call(&mut store, (thing,))
        .await
        .expect_err("the handle moved into the guest");
    assert_eq!(
        invalid_handle(&refused),
        "host resource already consumed",
        "{refused}"
    );
}

#[wcmp_macros::test]
async fn it_lends_an_owned_handle_then_moves_it_then_finds_its_entry_freed() {
    let (mut store, instance) = instance().await;
    // The handle is shared so that the test can lower it three times.
    let peek = typed::<(Arc<Resource<Thing>>,), ()>(&instance, "peek");
    let keep = typed::<(Arc<Resource<Thing>>,), ()>(&instance, "keep");
    let thing = Arc::new(Resource::<Thing>::new_own(5));

    // Lowered as a `borrow`, the handle, in no table yet, takes an
    // entry in the host's table and lends it for the call. The guest's
    // borrow names the rep, and its drop of the borrow runs no
    // destructor.
    peek.call(&mut store, (thing.clone(),))
        .await
        .expect("the guest borrows the handle");
    assert_eq!(store.data().inspected, vec![5], "the guest's borrow");
    assert!(store.data().things_dropped.is_empty(), "the host owns it");
    assert!(thing.owned(), "a borrow leaves the handle owned");

    // Lowered as an `own`, the entry moves out of the host's table and
    // into the guest's, whose drop runs the destructor.
    keep.call(&mut store, (thing.clone(),))
        .await
        .expect("the guest takes the handle");
    assert_eq!(store.data().inspected, vec![5, 5], "the guest's own");
    assert_eq!(store.data().things_dropped, vec![5], "the guest's drop");

    // The handle keeps the index of the entry that moved, as Wasmtime's
    // does, so a second `own` finds the entry freed. It is not taken
    // the way a handle in no table is.
    let refused = keep
        .call(&mut store, (thing,))
        .await
        .expect_err("the handle's entry moved into the guest");
    assert_eq!(
        invalid_handle(&refused),
        "handle index 1 is not live in the host's resource table",
        "{refused}"
    );
    assert_eq!(store.data().inspected, vec![5, 5], "the guest never ran");
}

#[wcmp_macros::test]
async fn it_names_the_identity_the_host_registered_in_an_exports_type() {
    let (_store, instance) = instance().await;
    let thing = ResourceType::host::<Thing>();
    let keep = instance.get_func("keep").expect("the export").ty();
    assert_eq!(keep.parameters[0].ty, ValueType::Own(thing));
    let peek = instance.get_func("peek").expect("the export").ty();
    assert_eq!(peek.parameters[0].ty, ValueType::Borrow(thing));
    let echo = instance.get_func("echo").expect("the export").ty();
    assert_eq!(echo.parameters[0].ty, ValueType::Own(thing));
    assert_eq!(echo.result, Some(ValueType::Own(thing)));
}

#[wcmp_macros::test]
async fn it_hands_back_the_rep_of_a_typed_handle() {
    let (mut store, instance) = instance().await;
    let echo = typed::<(Resource<Thing>,), (Resource<Thing>,)>(&instance, "echo");
    let (back,) = echo
        .call(&mut store, (Resource::new_own(11),))
        .await
        .expect("the guest returns the handle");
    assert_eq!(back.rep(), 11);
    assert!(back.owned());
}

#[wcmp_macros::test]
async fn it_refuses_to_lower_a_borrow_as_an_own() {
    let (mut store, instance) = instance().await;
    let keep = typed::<(Resource<Thing>,), ()>(&instance, "keep");
    let refused = keep
        .call(&mut store, (Resource::new_borrow(3),))
        .await
        .expect_err("a borrow is not an own");
    assert_eq!(
        invalid_handle(&refused),
        "cannot lower a `borrow` resource into an `own`",
        "{refused}"
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_typed_handle_of_another_identity() {
    let (_store, instance) = instance().await;
    let error = instance
        .get_func("keep")
        .expect("the export")
        .typed::<(Resource<Other>,), ()>()
        .expect_err("`thing` is not `Other`");
    assert!(matches!(error, Error::TypeMismatch(_)), "{error:?}");
    instance
        .get_func("keep")
        .expect("the export")
        .typed::<(ResourceAny,), ()>()
        .expect("a `ResourceAny` crosses as any handle");
}

#[wcmp_macros::test]
async fn it_crosses_a_guest_resource_as_a_resource_any() {
    let (mut store, instance) = instance().await;
    let make = typed::<(), (ResourceAny,)>(&instance, "make-doc");
    let take = typed::<(ResourceAny,), (u32,)>(&instance, "take-doc");

    let (first,) = make.call(&mut store, ()).await.expect("a first doc");
    assert!(first.owned());
    assert_ne!(first.ty(), ResourceType::host::<Thing>());
    let (rep,) = take
        .call(&mut store, (first,))
        .await
        .expect("the guest takes it back");
    assert_eq!(rep, 1);
    assert_eq!(store.data().docs_dropped, vec![1], "the guest dropped it");

    let (second,) = make.call(&mut store, ()).await.expect("a second doc");
    second
        .resource_drop(&mut store.as_context_mut())
        .expect("the host releases it");
    assert_eq!(
        store.data().docs_dropped,
        vec![1, 2],
        "the destructor ran once for it"
    );
    second
        .resource_drop(&mut store.as_context_mut())
        .expect_err("the entry is gone");
    assert_eq!(store.data().docs_dropped, vec![1, 2]);
}

#[wcmp_macros::test]
async fn it_refuses_a_resource_any_of_another_type() {
    let (mut store, instance) = instance().await;
    let make = typed::<(), (ResourceAny,)>(&instance, "make-doc");
    let keep = typed::<(ResourceAny,), ()>(&instance, "keep");
    let (doc,) = make.call(&mut store, ()).await.expect("a doc");
    let refused = keep
        .call(&mut store, (doc,))
        .await
        .expect_err("a `doc` is not a `thing`");
    let Error::Abi(abi) = &refused else {
        panic!("expected an ABI failure, got {refused:?}");
    };
    assert!(
        matches!(abi.cause, AbiCause::UnregisteredResourceType),
        "{refused}"
    );
}

#[wcmp_macros::test]
async fn it_converts_between_a_resource_any_and_a_typed_handle() {
    let (mut store, instance) = instance().await;
    let mut context = store.as_context_mut();
    let any = ResourceAny::try_from_resource(Resource::<Thing>::new_own(3), &mut context)
        .expect("an owned handle takes an entry");
    assert!(any.owned());
    assert_eq!(any.ty(), ResourceType::host::<Thing>());
    any.try_into_resource::<Other>(&mut context)
        .expect_err("the type is not `Other`");
    let thing = any
        .try_into_resource::<Thing>(&mut context)
        .expect("the type is `Thing`");
    assert_eq!(thing.rep(), 3);
    assert!(thing.owned());

    // The typed handle crosses after its round trip.
    let echo = typed::<(Resource<Thing>,), (Resource<Thing>,)>(&instance, "echo");
    let (back,) = echo
        .call(&mut store, (thing,))
        .await
        .expect("the handle crosses");
    assert_eq!(back.rep(), 3);
}

#[wcmp_macros::test]
async fn it_converts_a_host_borrow_to_a_resource_any_with_no_call_in_flight() {
    let (mut store, instance) = instance().await;
    let mut context = store.as_context_mut();
    let any = ResourceAny::try_from_resource(Resource::<Thing>::new_borrow(5), &mut context)
        .expect("a borrow the host made needs no call in flight");
    assert!(!any.owned());
    assert_eq!(any.ty(), ResourceType::host::<Thing>());
    let back = any
        .try_into_resource::<Thing>(&mut context)
        .expect("the typed round trip");
    assert_eq!(back.rep(), 5);
    assert!(!back.owned());
    any.resource_drop(&mut context)
        .expect("releasing it does nothing");
    any.resource_drop(&mut context)
        .expect("and releasing it again does nothing either");

    let peek = typed::<(ResourceAny,), ()>(&instance, "peek");
    peek.call(&mut store, (any,))
        .await
        .expect("the guest borrows its rep");
    assert_eq!(store.data().inspected, vec![5], "the guest saw the rep");

    let keep = typed::<(ResourceAny,), ()>(&instance, "keep");
    let refused = keep
        .call(&mut store, (any,))
        .await
        .expect_err("a borrow is not an own");
    assert_eq!(
        invalid_handle(&refused),
        "cannot lower a `borrow` resource into an `own`",
        "{refused}"
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_resource_any_whose_entry_was_freed_and_reused() {
    let (mut store, instance) = instance().await;
    let mut context = store.as_context_mut();
    let stale = ResourceAny::try_from_resource(Resource::<Thing>::new_own(1), &mut context)
        .expect("a first entry");
    stale
        .resource_drop(&mut context)
        .expect("the host releases it");
    let _reused = ResourceAny::try_from_resource(Resource::<Thing>::new_own(2), &mut context)
        .expect("a second entry takes the freed slot");
    let keep = typed::<(ResourceAny,), ()>(&instance, "keep");
    let refused = keep
        .call(&mut store, (stale,))
        .await
        .expect_err("the slot holds another resource now");
    assert!(
        invalid_handle(&refused).starts_with("host-owned resource was already de-allocated"),
        "{refused}"
    );
}

/// The interface the guest of [`DYNAMIC`] imports.
const DYNAMIC_HOST: &str = "wcmp-tests:resources/dynamic@0.1.0";

/// A guest that imports two host resources, `one` and `two`, and
/// returns each it receives.
const DYNAMIC: &[u8] = component!(
    r#"
    (component
      (import "wcmp-tests:resources/dynamic@0.1.0" (instance $host
        (export "one" (type (sub resource)))
        (export "two" (type (sub resource)))))
      (alias export $host "one" (type $one))
      (alias export $host "two" (type $two))
      (core module $m (func (export "echo") (param i32) (result i32) local.get 0))
      (core instance $c (instantiate $m))
      (func (export "echo-one") (param "h" (own $one)) (result (own $one))
        (canon lift (core func $c "echo")))
      (func (export "echo-two") (param "h" (own $two)) (result (own $two))
        (canon lift (core func $c "echo"))))
    "#
);

#[wcmp_macros::test]
async fn it_crosses_a_resource_as_a_dynamic_handle() {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, DYNAMIC)
        .await
        .expect("the component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let host: InterfaceIdentifier = DYNAMIC_HOST.parse().expect("identifier");
    for (name, ty) in [("one", 1), ("two", 2)] {
        linker
            .instance(&host)
            .resource(
                name,
                ResourceType::host_dynamic(ty),
                |_: StoreContext<'_, ()>, _| -> wcmp::Result<()> { Ok(()) },
            )
            .expect("the registration");
    }
    let mut store = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the component links");
    let one = typed::<(ResourceDynamic,), (ResourceDynamic,)>(&instance, "echo-one");
    let two = typed::<(ResourceDynamic,), (ResourceDynamic,)>(&instance, "echo-two");

    let (back,) = one
        .call(&mut store, (ResourceDynamic::new_own(7, 1),))
        .await
        .expect("a handle of type 1 crosses as `one`");
    assert_eq!((back.rep(), back.ty(), back.owned()), (7, 1, true));
    let (back,) = two
        .call(&mut store, (ResourceDynamic::new_own(8, 2),))
        .await
        .expect("a handle of type 2 crosses as `two`");
    assert_eq!((back.rep(), back.ty(), back.owned()), (8, 2, true));

    let refused = two
        .call(&mut store, (ResourceDynamic::new_own(9, 1),))
        .await
        .expect_err("a handle of type 1 is not a `two`");
    let Error::Abi(abi) = &refused else {
        panic!("expected an ABI failure, got {refused:?}");
    };
    assert!(
        matches!(abi.cause, AbiCause::UnregisteredResourceType),
        "{refused}"
    );
}

/// A guest that imports two interfaces, each of which declares a
/// resource named `error` and a function over a borrow of it.
const TWO_ERRORS: &[u8] = component!(
    r#"
    (component
      (import "wcmp-tests:resources/x@0.1.0" (instance $x
        (export "error" (type $error (sub resource)))
        (export "f" (func (param "e" (borrow $error))))))
      (import "wcmp-tests:resources/y@0.1.0" (instance $y
        (export "error" (type $error (sub resource)))
        (export "f" (func (param "e" (borrow $error)))))))
    "#
);

/// The markers of the two `error` resource types of [`TWO_ERRORS`].
struct XError;
struct YError;

#[wcmp_macros::test]
async fn it_type_checks_a_typed_handle_by_identity() {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, TWO_ERRORS)
        .await
        .expect("the component parses");
    let x: InterfaceIdentifier = "wcmp-tests:resources/x@0.1.0".parse().expect("identifier");
    let y: InterfaceIdentifier = "wcmp-tests:resources/y@0.1.0".parse().expect("identifier");
    let link = async |y_takes_x: bool| {
        let mut linker: Linker<()> = Linker::new(&engine);
        for (iface, ty) in [
            (&x, ResourceType::host::<XError>()),
            (&y, ResourceType::host::<YError>()),
        ] {
            linker
                .instance(iface)
                .resource(
                    "error",
                    ty,
                    |_: StoreContext<'_, ()>, _| -> wcmp::Result<()> { Ok(()) },
                )
                .expect("the registration");
        }
        linker
            .instance(&x)
            .func_wrap("f", |_: HostCall<'_, ()>, (_,): (Resource<XError>,)| Ok(()))
            .expect("the registration of `x`'s `f`");
        if y_takes_x {
            linker
                .instance(&y)
                .func_wrap("f", |_: HostCall<'_, ()>, (_,): (Resource<XError>,)| Ok(()))
                .expect("the registration of `y`'s `f`");
        } else {
            linker
                .instance(&y)
                .func_wrap("f", |_: HostCall<'_, ()>, (_,): (Resource<YError>,)| Ok(()))
                .expect("the registration of `y`'s `f`");
        }
        let mut store = Store::new(&engine, ()).expect("store");
        linker.instantiate(&mut store, &component).await.map(|_| ())
    };
    link(false)
        .await
        .expect("each `f` takes the `error` of its own interface");
    let error = link(true).await.expect_err("`y`'s `error` is not `x`'s");
    assert!(matches!(error, Error::TypeMismatch(_)), "{error:?}");
}

/// A typed handle is `Send` and `Sync` whatever its marker is, which
/// the compiler checks here.
const _: fn() = || {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<Resource<std::rc::Rc<()>>>();
    send_sync::<ResourceDynamic>();
    send_sync::<ResourceAny>();
};

#[path = "support/backend.rs"]
mod test_backend;
