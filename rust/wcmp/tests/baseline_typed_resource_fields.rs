// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Baseline tests for resource handles as fields of derived types.
//!
//! A derived record holds a `Resource<T>` or a `ResourceAny` as it
//! holds any other field. A `borrow` field is valid until the host
//! function that received it returns. A lift that fails after it moved
//! an `own` field out of the guest's table traps and poisons the store,
//! so the moved handle is unreachable.

#![cfg(test)]

use wcmp::{
    AbiCause, Component, ComponentType, Engine, Error, HostCall, Instance, InterfaceIdentifier,
    Lift, Linker, Lower, Resource, ResourceAny, ResourceType, Store, StoreContext, TaskCause,
    TypedFunc,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// The interface the guest of [`FIELDS`] imports.
const HOST: &str = "wcmp-tests:fields/host@0.1.0";

/// The marker of the host resource type `thing`.
struct Thing;

/// The record `borrows`: two borrows of a `thing`.
#[derive(ComponentType, Lift, Lower)]
#[component(record)]
struct Borrows {
    thing: Resource<Thing>,
    any: ResourceAny,
}

/// The record `owns`: two owned `thing`s.
#[derive(ComponentType, Lift, Lower)]
#[component(record)]
struct Owns {
    thing: Resource<Thing>,
    any: ResourceAny,
}

/// The record `bad`: an owned `thing`, then a string.
#[derive(ComponentType, Lift, Lower)]
#[component(record)]
struct Bad {
    thing: Resource<Thing>,
    name: String,
}

/// A guest that imports the host resource `thing` and three functions
/// over records that hold it.
///
/// - `run` hands the host's `take-borrows` a borrow of each `thing`
///   it receives, drops both, then drops both `thing`s the host's
///   `give-owns` returns.
/// - `inspect` takes a borrow of a `thing` and does nothing.
/// - `bad` hands the host's `take-bad` the `thing` it receives and a
///   string of one byte, `0xff`, which is not UTF-8.
const FIELDS: &[u8] = component!(
    r#"
    (component
      (import "wcmp-tests:fields/host@0.1.0" (instance $host
        (export "thing" (type $thing (sub resource)))
        (type $borrows' (record (field "thing" (borrow $thing)) (field "any" (borrow $thing))))
        (export "borrows" (type $borrows (eq $borrows')))
        (type $owns' (record (field "thing" (own $thing)) (field "any" (own $thing))))
        (export "owns" (type $owns (eq $owns')))
        (type $bad' (record (field "thing" (own $thing)) (field "name" string)))
        (export "bad" (type $bad (eq $bad')))
        (export "take-borrows" (func (param "p" $borrows)))
        (export "give-owns" (func (result $owns)))
        (export "take-bad" (func (param "p" $bad)))))
      (alias export $host "thing" (type $thing))
      (alias export $host "take-borrows" (func $take-borrows))
      (alias export $host "give-owns" (func $give-owns))
      (alias export $host "take-bad" (func $take-bad))
      (core module $libc (memory (export "memory") 1))
      (core instance $libc (instantiate $libc))
      (core func $take-borrows-lower (canon lower (func $take-borrows)))
      (core func $give-owns-lower
        (canon lower (func $give-owns) (memory (core memory $libc "memory"))))
      (core func $take-bad-lower
        (canon lower (func $take-bad) (memory (core memory $libc "memory"))))
      (core func $thing-drop (canon resource.drop $thing))
      (core module $m
        (import "libc" "memory" (memory 1))
        (import "host" "take-borrows" (func $take-borrows (param i32 i32)))
        (import "host" "give-owns" (func $give-owns (param i32)))
        (import "host" "take-bad" (func $take-bad (param i32 i32 i32)))
        (import "host" "drop" (func $drop (param i32)))
        (func (export "run") (param i32 i32)
          local.get 0 local.get 1 call $take-borrows
          local.get 0 call $drop
          local.get 1 call $drop
          i32.const 16 call $give-owns
          i32.const 16 i32.load call $drop
          i32.const 20 i32.load call $drop)
        (func (export "inspect") (param i32))
        (func (export "bad") (param i32)
          i32.const 64 i32.const 0xff i32.store8
          local.get 0 i32.const 64 i32.const 1 call $take-bad))
      (core instance $c (instantiate $m
        (with "libc" (instance $libc))
        (with "host" (instance
          (export "take-borrows" (func $take-borrows-lower))
          (export "give-owns" (func $give-owns-lower))
          (export "take-bad" (func $take-bad-lower))
          (export "drop" (func $thing-drop))))))
      (func (export "run") (param "a" (own $thing)) (param "b" (own $thing))
        (canon lift (core func $c "run")))
      (func (export "inspect") (param "h" (borrow $thing))
        (canon lift (core func $c "inspect")))
      (func (export "bad") (param "h" (own $thing))
        (canon lift (core func $c "bad"))))
    "#
);

/// What the host saw: the reps of the borrows `take-borrows` received,
/// the reps the `thing` destructor received, the borrow `take-borrows`
/// kept past its return, and whether `take-bad` ran.
#[derive(Default)]
struct Seen {
    borrowed: Vec<u32>,
    dropped: Vec<u32>,
    kept: Option<ResourceAny>,
    took_bad: bool,
}

/// Instantiate [`FIELDS`] with `thing` registered as
/// [`ResourceType::host::<Thing>()`] and typed host functions over the
/// derived records.
async fn instance() -> (Store<Seen>, Instance) {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, FIELDS)
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
                store.data_mut().dropped.push(rep);
                Ok(())
            },
        )
        .expect("the registration of `thing`");
    linker
        .instance(&host)
        .func_wrap(
            "take-borrows",
            |mut call: HostCall<'_, Seen>, (borrows,): (Borrows,)| {
                assert!(!borrows.thing.owned() && !borrows.any.owned());
                let any = borrows.any.try_into_resource::<Thing>(call.store())?.rep();
                let seen = call.data_mut();
                seen.borrowed.extend([borrows.thing.rep(), any]);
                seen.kept = Some(borrows.any);
                Ok(())
            },
        )
        .expect("the registration of `take-borrows`");
    linker
        .instance(&host)
        .func_wrap("give-owns", |mut call: HostCall<'_, Seen>, (): ()| {
            let any = ResourceAny::try_from_resource(Resource::<Thing>::new_own(8), call.store())?;
            Ok((Owns {
                thing: Resource::new_own(7),
                any,
            },))
        })
        .expect("the registration of `give-owns`");
    linker
        .instance(&host)
        .func_wrap("take-bad", |mut call: HostCall<'_, Seen>, (_,): (Bad,)| {
            call.data_mut().took_bad = true;
            Ok(())
        })
        .expect("the registration of `take-bad`");
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

#[wcmp_macros::test]
async fn it_crosses_borrow_and_own_fields_of_a_derived_record() {
    let (mut store, instance) = instance().await;
    typed::<(Resource<Thing>, Resource<Thing>), ()>(&instance, "run")
        .call(&mut store, (Resource::new_own(1), Resource::new_own(2)))
        .await
        .expect("the guest runs");
    assert_eq!(store.data().borrowed, vec![1, 2], "the borrow fields");
    assert_eq!(
        store.data().dropped,
        vec![1, 2, 7, 8],
        "the guest dropped what it received, then the own fields"
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_borrow_field_kept_past_its_host_function() {
    let (mut store, instance) = instance().await;
    typed::<(Resource<Thing>, Resource<Thing>), ()>(&instance, "run")
        .call(&mut store, (Resource::new_own(1), Resource::new_own(2)))
        .await
        .expect("the guest runs");
    let kept = store.data_mut().kept.take().expect("the kept borrow");
    let refused = typed::<(ResourceAny,), ()>(&instance, "inspect")
        .call(&mut store, (kept,))
        .await
        .expect_err("the borrow ended with its host function");
    let Error::Abi(abi) = &refused else {
        panic!("expected an ABI failure, got {refused:?}");
    };
    assert!(
        matches!(abi.cause, AbiCause::InvalidHandle { .. }),
        "{refused}"
    );
}

#[wcmp_macros::test]
async fn it_poisons_the_store_when_a_lift_fails_after_moving_an_own_field() {
    let (mut store, instance) = instance().await;
    let refused = typed::<(Resource<Thing>,), ()>(&instance, "bad")
        .call(&mut store, (Resource::new_own(3),))
        .await
        .expect_err("the string field is not UTF-8");
    // The lift of the string is what failed, after the `own` field
    // moved, and the call gets the failure back as the lift raised it.
    let Error::Abi(abi) = &refused else {
        panic!("expected an ABI failure, got {refused:?}");
    };
    assert!(
        matches!(&abi.cause, AbiCause::InvalidEncoding { message } if message == "invalid utf-8"),
        "expected the invalid-encoding cause of a string that is not UTF-8, got {refused}"
    );
    assert!(!store.data().took_bad, "the host function never ran");
    assert!(
        store.data().dropped.is_empty(),
        "the moved handle is unreachable and no destructor ran: {refused}"
    );
    let later = typed::<(Resource<Thing>, Resource<Thing>), ()>(&instance, "run")
        .call(&mut store, (Resource::new_own(1), Resource::new_own(2)))
        .await
        .expect_err("the store is poisoned");
    assert!(
        matches!(later, Error::Task(TaskCause::CannotEnter)),
        "{later:?}"
    );
}

#[path = "support/backend.rs"]
mod test_backend;
