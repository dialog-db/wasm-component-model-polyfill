// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Tests of the resources `wcmp::bindgen!` writes.
//!
//! An imported resource is the empty enum `pub enum Counter {}`, its
//! handle `Resource<Counter>`, and its traits `HostCounter` and
//! `HostCounterWithStore<T>`, which the interface's own traits extend.
//! Its registration names `ResourceType::host::<Counter>()` and a
//! destructor that calls `drop`. A resource another interface `use`s
//! is the same resource, registered once. An exported resource is a
//! `ResourceAny`, whose calls a getter named after it gives.

#![cfg(test)]

use wcmp::{Access, Component, Engine, HasSelf, Instance, Linker, Resource, ResourceTable, Store};

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

mod bindings {
    wcmp::bindgen!({ path: "wit-resources" });
}

/// The same world, with the `store` flag on the counter's destructor.
mod store_drop {
    wcmp::bindgen!({
        path: "wit-resources",
        imports: { "test:resources/counters.[drop]counter": store },
    });
}

use bindings::Resources;
use bindings::test::resources::{counters, users};

/// What the host keeps for each counter.
struct CounterState {
    value: u32,
}

/// The host data.
#[derive(Default)]
struct HostData {
    table: ResourceTable,
    /// The rep of each counter the constructor made.
    made: Vec<u32>,
    /// The rep of each counter a method was handed.
    seen: Vec<u32>,
    /// How many times `drop` ran.
    drops: u32,
}

impl HostData {
    /// The counter `handle` names.
    fn counter(&mut self, handle: &Resource<counters::Counter>) -> &mut CounterState {
        self.seen.push(handle.rep());
        self.table
            .get_mut(&Resource::<CounterState>::new_borrow(handle.rep()))
            .expect("the counter is in the table")
    }

    /// Delete the counter `handle` names.
    fn delete(&mut self, handle: Resource<counters::Counter>) -> wcmp::Result<()> {
        self.table
            .delete(Resource::<CounterState>::new_own(handle.rep()))?;
        self.drops += 1;
        Ok(())
    }
}

impl counters::HostCounter for HostData {
    fn new(&mut self, start: u32) -> Resource<counters::Counter> {
        let pushed = self
            .table
            .push(CounterState { value: start })
            .expect("the table has room");
        self.made.push(pushed.rep());
        Resource::new_own(pushed.rep())
    }

    fn get(&mut self, self_: Resource<counters::Counter>) -> u32 {
        self.counter(&self_).value
    }

    fn increment(&mut self, self_: Resource<counters::Counter>) {
        self.counter(&self_).value += 1;
    }

    fn drop(&mut self, rep: Resource<counters::Counter>) -> wcmp::Result<()> {
        self.delete(rep)
    }
}

impl counters::Host for HostData {}

impl users::Host for HostData {
    fn read(&mut self, c: Resource<users::Counter>) -> u32 {
        // `users::Counter` is `counters::Counter`: one resource, so one
        // Rust type and one identity.
        let c: Resource<counters::Counter> = c;
        self.counter(&c).value
    }
}

/// The host of [`store_drop`], whose `drop` takes an `Access`.
#[derive(Default)]
struct StoreDropData {
    inner: HostData,
}

impl store_drop::test::resources::counters::HostCounter for StoreDropData {
    fn new(&mut self, start: u32) -> Resource<store_drop::test::resources::counters::Counter> {
        Resource::new_own(counters::HostCounter::new(&mut self.inner, start).rep())
    }

    fn get(&mut self, self_: Resource<store_drop::test::resources::counters::Counter>) -> u32 {
        counters::HostCounter::get(&mut self.inner, Resource::new_borrow(self_.rep()))
    }

    fn increment(&mut self, self_: Resource<store_drop::test::resources::counters::Counter>) {
        counters::HostCounter::increment(&mut self.inner, Resource::new_borrow(self_.rep()));
    }
}

impl store_drop::test::resources::counters::HostCounterWithStore<StoreDropData>
    for HasSelf<StoreDropData>
{
    fn drop(
        mut host: Access<'_, StoreDropData, Self>,
        rep: Resource<store_drop::test::resources::counters::Counter>,
    ) -> wcmp::Result<()> {
        host.get().inner.delete(Resource::new_own(rep.rep()))
    }
}

impl store_drop::test::resources::counters::Host for StoreDropData {}

impl store_drop::test::resources::users::Host for StoreDropData {
    fn read(&mut self, c: Resource<store_drop::test::resources::users::Counter>) -> u32 {
        counters::HostCounter::get(&mut self.inner, Resource::new_borrow(c.rep()))
    }
}

/// Instantiate [`guests::RESOURCES`] with the generated registrations for `T`.
async fn instantiate<T: wcmp::MaybeSend + 'static>(
    data: T,
    register: impl FnOnce(&mut Linker<T>),
) -> (Store<T>, Instance, Component) {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, guests::RESOURCES)
        .await
        .expect("the guest parses");
    let mut linker: Linker<T> = Linker::new(&engine);
    register(&mut linker);
    let mut store = Store::new(&engine, data).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the resource registered once serves both interfaces");
    (store, instance, component)
}

/// Call the guest's `run` with `start`.
async fn run<T: 'static>(store: &mut Store<T>, instance: &Instance, start: u32) -> u32 {
    let (value,) = instance
        .get_func("run")
        .expect("the guest exports `run`")
        .typed::<(u32,), (u32,)>()
        .expect("the export's type")
        .call(store, (start,))
        .await
        .expect("call run");
    value
}

#[wcmp_macros::test]
async fn it_crosses_an_imported_resource_through_its_handle() {
    let (mut store, instance, _) = instantiate(HostData::default(), |linker| {
        Resources::add_to_linker::<HostData, HasSelf<HostData>>(linker, |data| data)
            .expect("the registration");
    })
    .await;

    assert_eq!(
        run(&mut store, &instance, 4).await,
        5005,
        "the guest made, incremented, read, and got the counter through its handle"
    );
    let data = store.data();
    assert_eq!(data.made.len(), 1, "the constructor ran once");
    assert!(
        data.seen.iter().all(|rep| *rep == data.made[0]),
        "each method received the guest's borrow of the counter the constructor made: \
         made {:?}, seen {:?}",
        data.made,
        data.seen
    );
    assert_eq!(data.seen.len(), 3, "increment, read, and get each saw it");
    assert_eq!(data.drops, 1, "the guest's drop ran `drop` once");
}

#[wcmp_macros::test]
async fn it_gives_a_store_flag_destructor_an_access() {
    let (mut store, instance, _) = instantiate(StoreDropData::default(), |linker| {
        store_drop::Resources::add_to_linker::<StoreDropData, HasSelf<StoreDropData>>(
            linker,
            |data| data,
        )
        .expect("the registration");
    })
    .await;

    assert_eq!(run(&mut store, &instance, 1).await, 2002);
    assert_eq!(
        store.data().inner.drops,
        1,
        "the destructor with an `Access` ran once"
    );
}

#[wcmp_macros::test]
async fn it_crosses_an_exported_resource_through_resource_any() {
    let (mut store, instance, _) = instantiate(HostData::default(), |linker| {
        Resources::add_to_linker::<HostData, HasSelf<HostData>>(linker, |data| data)
            .expect("the registration");
    })
    .await;
    let world = Resources::new(&mut store, &instance).expect("the world's exports");

    let accumulators = world.test_resources_guest_made().accumulator();
    let accumulator = accumulators
        .call_constructor(&mut store, 5)
        .await
        .expect("call the constructor");
    accumulators
        .call_add(&mut store, accumulator, 3)
        .await
        .expect("call add");
    assert_eq!(
        accumulators
            .call_total(&mut store, accumulator)
            .await
            .expect("call total"),
        8,
        "the method returned what the guest computed from the constructor's argument"
    );
}

#[path = "support/guests.rs"]
mod guests;
#[path = "../../wcmp/tests/support/backend.rs"]
mod test_backend;
