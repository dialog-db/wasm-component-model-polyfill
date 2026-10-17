// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Tests of the `with` option of `wcmp::bindgen!`.
//!
//! A resource `with` maps is the host's own type: the generated methods
//! take and return `Resource` of it, so the host pushes its value into a
//! `ResourceTable` and returns the handle as it is. An interface `with`
//! maps is the module another invocation wrote: the macro writes none
//! for it. The most specific key that matches wins.

#![cfg(test)]

use wcmp::{Component, Engine, HasSelf, Instance, Linker, Resource, ResourceTable, Store};

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// A host core: types with no runtime in them.
mod core {
    /// The host's counter, which `with` maps the WIT `counter` to.
    pub struct Counter {
        pub value: u32,
    }
}

/// The resource `counter` mapped to [`core::Counter`].
mod mapped_resource {
    wcmp::bindgen!({
        path: "wit-resources",
        with: { "test:resources/counters.counter": crate::core::Counter },
    });
}

/// A first invocation, whose `counters` module the next maps to.
mod first {
    wcmp::bindgen!({ path: "wit-resources" });
}

/// The interface `counters` mapped to the module [`first`] wrote.
mod mapped_interface {
    wcmp::bindgen!({
        path: "wit-resources",
        with: { "test:resources/counters": crate::first::test::resources::counters },
    });
}

/// Another invocation, whose modules are other types than [`first`]'s.
mod other {
    wcmp::bindgen!({ path: "wit-resources" });
}

/// The interface `counters` by its own key, and the rest of the package
/// by the package's key.
mod most_specific {
    wcmp::bindgen!({
        path: "wit-resources",
        with: {
            "test:resources/counters": crate::first::test::resources::counters,
            "test:resources/guest-made": crate::other::exports::test::resources::guest_made,
            "test:resources": crate::other::test::resources,
        },
    });
}

/// The interface key won for `counters`: its resource is [`first`]'s.
fn counters_by_the_interface_key(
    counter: Resource<first::test::resources::counters::Counter>,
) -> Resource<most_specific::test::resources::counters::Counter> {
    counter
}

/// The package key served `users`: it is [`other`]'s module.
fn users_by_the_package_key(
    read: fn(&mut dyn other::test::resources::users::Host),
) -> fn(&mut dyn most_specific::test::resources::users::Host) {
    read
}

/// The host data of [`mapped_resource`].
#[derive(Default)]
struct HostData {
    table: ResourceTable,
    drops: u32,
}

impl mapped_resource::test::resources::counters::HostCounter for HostData {
    fn new(&mut self, start: u32) -> Resource<core::Counter> {
        // The table's handle is the generated method's handle: no
        // conversion between them.
        self.table
            .push(core::Counter { value: start })
            .expect("the table has room")
    }

    fn get(&mut self, self_: Resource<core::Counter>) -> u32 {
        self.table.get(&self_).expect("the counter").value
    }

    fn increment(&mut self, self_: Resource<core::Counter>) {
        self.table.get_mut(&self_).expect("the counter").value += 1;
    }

    fn drop(&mut self, rep: Resource<core::Counter>) -> wcmp::Result<()> {
        self.table.delete(rep)?;
        self.drops += 1;
        Ok(())
    }
}

impl mapped_resource::test::resources::counters::Host for HostData {}

impl mapped_resource::test::resources::users::Host for HostData {
    fn read(&mut self, c: Resource<core::Counter>) -> u32 {
        self.table.get(&c).expect("the counter").value
    }
}

/// The host data of [`mapped_interface`], which implements [`first`]'s
/// traits for `counters` and the second invocation's for `users`.
#[derive(Default)]
struct FirstData {
    table: ResourceTable,
}

impl first::test::resources::counters::HostCounter for FirstData {
    fn new(&mut self, start: u32) -> Resource<first::test::resources::counters::Counter> {
        let pushed = self.table.push(start).expect("the table has room");
        Resource::new_own(pushed.rep())
    }

    fn get(&mut self, self_: Resource<first::test::resources::counters::Counter>) -> u32 {
        *self
            .table
            .get(&Resource::<u32>::new_borrow(self_.rep()))
            .expect("the counter")
    }

    fn increment(&mut self, self_: Resource<first::test::resources::counters::Counter>) {
        *self
            .table
            .get_mut(&Resource::<u32>::new_borrow(self_.rep()))
            .expect("the counter") += 1;
    }

    fn drop(
        &mut self,
        rep: Resource<first::test::resources::counters::Counter>,
    ) -> wcmp::Result<()> {
        self.table.delete(Resource::<u32>::new_own(rep.rep()))?;
        Ok(())
    }
}

impl first::test::resources::counters::Host for FirstData {}

impl mapped_interface::test::resources::users::Host for FirstData {
    fn read(&mut self, c: Resource<first::test::resources::counters::Counter>) -> u32 {
        first::test::resources::counters::HostCounter::get(self, c)
    }
}

/// Instantiate the resources guest with the registrations `register`
/// adds.
async fn instantiate<T: wcmp::MaybeSend + 'static>(
    data: T,
    register: impl FnOnce(&mut Linker<T>),
) -> (Store<T>, Instance) {
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
        .expect("the guest instantiates");
    (store, instance)
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
async fn it_maps_a_resource_to_a_type_of_the_host_core() {
    let (mut store, instance) = instantiate(HostData::default(), |linker| {
        mapped_resource::Resources::add_to_linker::<HostData, HasSelf<HostData>>(linker, |data| {
            data
        })
        .expect("the registration");
    })
    .await;
    assert_eq!(run(&mut store, &instance, 2).await, 3003);
    assert_eq!(
        store.data().drops,
        1,
        "the guest's drop reached the host's type"
    );
}

#[wcmp_macros::test]
async fn it_maps_an_interface_to_the_module_of_another_invocation() {
    let (mut store, instance) = instantiate(FirstData::default(), |linker| {
        mapped_interface::Resources::add_to_linker::<FirstData, HasSelf<FirstData>>(
            linker,
            |data| data,
        )
        .expect("the registration through the first invocation's traits");
    })
    .await;
    assert_eq!(run(&mut store, &instance, 6).await, 7007);
}

#[wcmp_macros::test]
fn it_takes_the_most_specific_key_that_matches() {
    // The two functions above compile only because the interface key
    // won for `counters` and the package key served `users`.
    let _ = counters_by_the_interface_key;
    let _ = users_by_the_package_key;
}

#[path = "support/guests.rs"]
mod guests;
#[path = "../../wcmp/tests/support/backend.rs"]
mod test_backend;
