// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Baseline tests for a resource one interface `use`s from another.
//!
//! An interface `b` that does `use a.{r}` is imported as an instance
//! whose type carries `(export "r" (type (eq …)))`: the resource `a`
//! brought in, not a new one. It links with `r` registered under `a`
//! alone, as in Wasmtime, whose type checker accepts such an import
//! with no definition of its own. A registration of `r` under `b` that
//! is there anyway must carry the identity `a`'s did. A resource `b`
//! declares itself still needs a registration under `b`.

#![cfg(test)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use wcmp::{
    Component, Engine, Error, HostCall, Instance, InterfaceIdentifier, Linker, Resource,
    ResourceType, Store, StoreContext,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// The host's resource type.
struct Counter;

/// Another host resource type.
struct Other;

/// A guest whose `wcmp-tests:eq/users` uses the resource of
/// `wcmp-tests:eq/counters`. `run` makes a counter through `counters`,
/// reads it through `users`, drops it, and returns the read.
const USES_A_RESOURCE: &[u8] = component!(
    r#"
    (component
      (import "wcmp-tests:eq/counters" (instance $counters
        (export "counter" (type $counter (sub resource)))
        (export "[constructor]counter" (func (param "start" u32) (result (own $counter))))))
      (alias export $counters "counter" (type $counter))
      (import "wcmp-tests:eq/users" (instance $users
        (alias outer 1 $counter (type $outer))
        (export "counter" (type $c (eq $outer)))
        (export "read" (func (param "c" (borrow $c)) (result u32)))))
      (alias export $counters "[constructor]counter" (func $new))
      (alias export $users "read" (func $read))
      (core func $new-lower (canon lower (func $new)))
      (core func $read-lower (canon lower (func $read)))
      (core func $drop (canon resource.drop $counter))
      (core module $m
        (import "" "new" (func $new (param i32) (result i32)))
        (import "" "read" (func $read (param i32) (result i32)))
        (import "" "drop" (func $drop (param i32)))
        (func (export "run") (param $start i32) (result i32)
          (local $h i32) (local $read i32)
          (local.set $h (call $new (local.get $start)))
          (local.set $read (call $read (local.get $h)))
          (call $drop (local.get $h))
          (local.get $read)))
      (core instance $i (instantiate $m
        (with "" (instance
          (export "new" (func $new-lower))
          (export "read" (func $read-lower))
          (export "drop" (func $drop))))))
      (func (export "run") (param "start" u32) (result u32) (canon lift (core func $i "run"))))
    "#
);

/// A guest whose `wcmp-tests:eq/users` declares a resource of its own,
/// not `counters`'.
const DECLARES_ITS_OWN: &[u8] = component!(
    r#"
    (component
      (import "wcmp-tests:eq/counters" (instance $counters
        (export "counter" (type $counter (sub resource)))))
      (import "wcmp-tests:eq/users" (instance $users
        (export "counter" (type $c (sub resource)))
        (export "read" (func (param "c" (borrow $c)) (result u32))))))
    "#
);

/// The interface `name` of the package `wcmp-tests:eq`.
fn interface(name: &str) -> InterfaceIdentifier {
    format!("wcmp-tests:eq/{name}")
        .parse()
        .expect("an interface name")
}

/// A linker with `counter` and its constructor under `counters`, whose
/// destructor counts into `drops`, and `read` under `users`.
fn linker(engine: &Engine, drops: Arc<AtomicU32>) -> Linker<()> {
    let mut linker: Linker<()> = Linker::new(engine);
    let mut counters = linker.instance(&interface("counters"));
    counters
        .resource(
            "counter",
            ResourceType::host::<Counter>(),
            move |_store: StoreContext<'_, ()>, _rep: u32| {
                drops.fetch_add(1, Ordering::Relaxed);
                Ok(())
            },
        )
        .expect("the resource");
    counters
        .func_wrap(
            "[constructor]counter",
            |_call: HostCall<'_, ()>, (start,): (u32,)| Ok((Resource::<Counter>::new_own(start),)),
        )
        .expect("the constructor");
    let mut users = linker.instance(&interface("users"));
    users
        .func_wrap(
            "read",
            |_call: HostCall<'_, ()>, (c,): (Resource<Counter>,)| Ok((c.rep(),)),
        )
        .expect("read");
    linker
}

/// Instantiate `bytes` with `linker`.
async fn instantiate(
    bytes: &[u8],
    linker: &Linker<()>,
    engine: &Engine,
) -> Result<(Store<()>, Instance), Error> {
    let component = Component::new(engine, bytes)
        .await
        .expect("the component parses");
    let mut store = Store::new(engine, ()).expect("store");
    let instance = linker.instantiate(&mut store, &component).await?;
    Ok((store, instance))
}

#[wcmp_macros::test]
async fn it_links_a_used_resource_registered_under_its_defining_interface_alone() {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let drops = Arc::new(AtomicU32::new(0));
    let linker = linker(&engine, drops.clone());
    let (mut store, instance) = instantiate(USES_A_RESOURCE, &linker, &engine)
        .await
        .expect("the component links with `counter` registered under `counters` alone");

    let (read,) = instance
        .get_func("run")
        .expect("the component exports `run`")
        .typed::<(u32,), (u32,)>()
        .expect("the export's type")
        .call(&mut store, (9,))
        .await
        .expect("call run");
    assert_eq!(
        read, 9,
        "the handle minted under `counters` passed through `users`' function"
    );
    assert_eq!(
        drops.load(Ordering::Relaxed),
        1,
        "the guest's drop reached the host"
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_registration_of_the_used_resource_with_another_identity() {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let mut linker = linker(&engine, Arc::new(AtomicU32::new(0)));
    linker
        .instance(&interface("users"))
        .resource("counter", ResourceType::host::<Other>(), |_store, _rep| {
            Ok(())
        })
        .expect("the registration");

    let error = instantiate(USES_A_RESOURCE, &linker, &engine)
        .await
        .err()
        .expect("the registration names another resource");
    let Error::TypeMismatch(mismatch) = &error else {
        panic!("the identities differ, got {error:?}");
    };
    let rendered = format!("{mismatch:?}");
    assert!(
        rendered.contains("users") && rendered.contains("counter"),
        "the mismatch names the interface and the item: {rendered}"
    );
}

#[wcmp_macros::test]
async fn it_needs_a_registration_of_a_resource_an_interface_declares_itself() {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let linker = linker(&engine, Arc::new(AtomicU32::new(0)));
    let error = instantiate(DECLARES_ITS_OWN, &linker, &engine)
        .await
        .err()
        .expect("`users` declares a resource nothing registers");
    assert!(
        matches!(&error, Error::Link(link) if format!("{link:?}").contains("counter")),
        "the resource `users` declares is unresolved, got {error:?}"
    );
}

#[path = "support/backend.rs"]
mod test_backend;
