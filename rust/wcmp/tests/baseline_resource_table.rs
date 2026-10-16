// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Baseline tests for `ResourceTable`, the table of host values that a
//! `Resource<T>` names by its rep.
//!
//! A host keeps the values behind its resources in the table, hands a
//! guest the handle that `push` answers, and deletes the value when the
//! resource's destructor runs. In the browser the table holds values
//! that are not `Send`.

#![cfg(test)]

use wcmp::{
    Component, Engine, Instance, InterfaceIdentifier, Linker, Resource, ResourceTable,
    ResourceType, Store, StoreContext, TypedFunc,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// A guest that imports the host resource `thing`.
///
/// - `echo` returns the `thing` it receives.
/// - `drop` drops the `thing` it receives.
const THINGS: &[u8] = component!(
    r#"
    (component
      (import "wcmp-tests:table/host@0.1.0" (instance $host
        (export "thing" (type (sub resource)))))
      (alias export $host "thing" (type $thing))
      (core func $thing-drop (canon resource.drop $thing))
      (core module $m
        (import "host" "drop" (func $drop (param i32)))
        (func (export "echo") (param i32) (result i32) local.get 0)
        (func (export "drop") (param i32) local.get 0 call $drop))
      (core instance $c (instantiate $m
        (with "host" (instance (export "drop" (func $thing-drop))))))
      (func (export "echo") (param "h" (own $thing)) (result (own $thing))
        (canon lift (core func $c "echo")))
      (func (export "drop") (param "h" (own $thing))
        (canon lift (core func $c "drop"))))
    "#
);

/// Instantiate [`THINGS`] with `thing` registered as
/// [`ResourceType::host::<V>()`], where `V` is the type of the values
/// the table holds, and a destructor that deletes the value from the
/// table in the store data.
async fn instance<V: 'static>() -> (Store<ResourceTable>, Instance) {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, THINGS)
        .await
        .expect("the component parses");
    let mut linker: Linker<ResourceTable> = Linker::new(&engine);
    let host: InterfaceIdentifier = "wcmp-tests:table/host@0.1.0".parse().expect("identifier");
    linker
        .instance(&host)
        .resource(
            "thing",
            ResourceType::host::<V>(),
            |mut store: StoreContext<'_, ResourceTable>, rep| -> wcmp::Result<()> {
                store
                    .data_mut()
                    .delete(Resource::<V>::new_own(rep))
                    .map_err(wcmp::Error::ResourceTable)?;
                Ok(())
            },
        )
        .expect("the registration of `thing`");
    let mut store = Store::new(&engine, ResourceTable::new()).expect("store");
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
async fn it_frees_a_table_entry_when_a_guest_drops_the_handle() {
    let (mut store, instance) = instance::<String>().await;
    let entry = store
        .data_mut()
        .push("kept".to_owned())
        .expect("a slot for the value");
    let rep = entry.rep();
    assert_eq!(
        store.data().get(&entry).expect("the value"),
        "kept",
        "the handle names the value"
    );
    typed::<(Resource<String>,), ()>(&instance, "drop")
        .call(&mut store, (entry,))
        .await
        .expect("the guest drops the handle");
    assert!(
        matches!(
            store.data().get(&Resource::<String>::new_own(rep)),
            Err(wcmp::ResourceTableError::NotPresent)
        ),
        "the destructor deleted the value"
    );
    assert!(store.data().is_empty());
}

#[cfg(target_arch = "wasm32")]
#[wcmp_macros::test]
async fn it_holds_a_value_that_is_not_send_in_the_browser() {
    use wasm_bindgen::JsValue;

    let (mut store, instance) = instance::<JsValue>().await;
    let entry = store
        .data_mut()
        .push(JsValue::from_str("in the browser"))
        .expect("a slot for the value");
    let rep = entry.rep();
    let (back,) = typed::<(Resource<JsValue>,), (Resource<JsValue>,)>(&instance, "echo")
        .call(&mut store, (entry,))
        .await
        .expect("the handle crosses into the guest and back");
    assert_eq!(back.rep(), rep);
    let value = store
        .data()
        .get(&back)
        .expect("the value is still in the table");
    assert_eq!(value.as_string().as_deref(), Some("in the browser"));
}

#[path = "support/backend.rs"]
mod test_backend;
