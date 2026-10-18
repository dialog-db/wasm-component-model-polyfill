// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The polyfill glue: `wcmp::bindgen!` over `test:dual/dual`, and the
//! trait impls that call the host core, on both targets.

use wcmp::{
    Access, Accessor, Component, Engine, Error, FutureReader, HasSelf, Linker, Resource,
    ResourceTable, Store,
};

use crate::host_core::{CheckFailed, Core, Counter, PendingOnce, ReadError, Shape};

mod bindings {
    wcmp::bindgen!({
        path: "wit-dual",
        imports: {
            "test:dual/host.checked": trappable,
            "test:dual/host.read": trappable,
            "test:dual/host.lost": store | trappable,
        },
        with: { "test:dual/host.counter": crate::host_core::Counter },
        trappable_error_type: { "test:dual/host.fs-error" => crate::host_core::ReadError },
    });
}

use bindings::Dual;
use bindings::test::dual::{host, types};

/// The host data: the core, and the table the counters live in.
#[derive(Default)]
pub struct HostData {
    core: Core,
    table: ResourceTable,
}

impl types::Host for HostData {}

impl host::HostCounter for HostData {
    fn new(&mut self, start: u32) -> Resource<Counter> {
        let counter = self.core.counter(start);
        self.table.push(counter).expect("the table has room")
    }

    fn get(&mut self, self_: Resource<Counter>) -> u32 {
        self.table.get(&self_).map_or(0, |counter| counter.value)
    }

    fn drop(&mut self, rep: Resource<Counter>) -> wcmp::Result<()> {
        let counter = self.table.delete(rep)?;
        self.core.dropped(&counter);
        Ok(())
    }
}

impl host::Host for HostData {
    fn describe(&mut self, shape: types::Shape, perms: types::Perms) -> types::Point {
        let shape = core_shape(shape);
        let read = perms.contains(types::Perms::READ);
        let write = perms.contains(types::Perms::WRITE);
        let (x, y) = self.core.describe(shape, read, write);
        types::Point { x, y }
    }

    fn checked(&mut self, x: u32) -> wcmp::Result<u32> {
        Ok(self.core.checked(x).map_err(anyhow::Error::new)?)
    }

    fn read(&mut self, code: u32) -> Result<u32, ReadError> {
        self.core.read(code)
    }

    fn convert_fs_error(&mut self, err: ReadError) -> wcmp::Result<host::FsError> {
        match err {
            ReadError::Missing => Ok(host::FsError::NotFound),
            ReadError::Broken => Err(anyhow::Error::new(err))?,
        }
    }
}

impl host::HostWithStore<HostData> for HasSelf<HostData> {
    async fn answer(accessor: &Accessor<HostData, Self>, x: u32) -> u32 {
        PendingOnce::default().await;
        accessor.with(|mut access| access.get().core.answer(x))
    }

    fn lost(mut host: Access<'_, HostData, Self>) -> wcmp::Result<FutureReader<u32>> {
        let value = host.get().core.lost();
        FutureReader::new(host.store(), async move { Ok::<u32, Error>(value) })
    }
}

/// The core's form of a generated shape.
fn core_shape(shape: types::Shape) -> Shape {
    match shape {
        types::Shape::Circle(r) => Shape::Circle(r),
        types::Shape::Square(point) => Shape::Square(point.x, point.y),
        types::Shape::None => Shape::None,
    }
}

/// A read's result, as a run records it.
fn read_result(read: Result<u32, host::FsError>) -> String {
    match read {
        Ok(value) => format!("Ok({value})"),
        Err(host::FsError::NotFound) => "Err(not-found)".to_owned(),
        Err(host::FsError::Denied) => "Err(denied)".to_owned(),
    }
}

/// The core's error a call failed with, if it failed with one.
fn core_error(error: &Error) -> String {
    let source = std::error::Error::source(error);
    match (
        source.and_then(|source| source.downcast_ref::<CheckFailed>()),
        source.and_then(|source| source.downcast_ref::<ReadError>()),
    ) {
        (Some(failed), _) => format!("error {failed:?}"),
        (_, Some(failed)) => format!("error {failed:?}"),
        _ => format!("error {error}"),
    }
}

/// Instantiate the guest into a store of its own.
async fn instantiate(engine: &Engine, component: &Component) -> (Store<HostData>, Dual) {
    let mut linker: Linker<HostData> = Linker::new(engine);
    Dual::add_to_linker::<HostData, HasSelf<HostData>>(&mut linker, |data| data)
        .expect("the registrations");
    let mut store = Store::new(engine, HostData::default()).expect("store");
    let world = Dual::instantiate_async(&mut store, component, &linker)
        .await
        .expect("the guest instantiates");
    (store, world)
}

/// Run the guest, and record each value that crosses and each error.
pub async fn run(guest: &[u8]) -> Vec<String> {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, guest)
        .await
        .expect("the guest parses");
    let mut record = Vec::new();

    let (mut store, world) = instantiate(&engine, &component).await;
    let calls = world.test_dual_guest();
    let shape = calls
        .call_shape_of(&mut store, types::Point { x: 3, y: 4 })
        .await
        .expect("shape-of");
    record.push(format!("shape-of -> {:?}", core_shape(shape)));
    let point = calls
        .call_call_describe(&mut store, 3)
        .await
        .expect("call-describe");
    record.push(format!("call-describe -> ({}, {})", point.x, point.y));
    let counted = calls
        .call_call_counter(&mut store, 5)
        .await
        .expect("call-counter");
    record.push(format!("call-counter -> {counted}"));
    let accumulators = calls.accumulator();
    let accumulator = accumulators
        .call_constructor(&mut store, 6)
        .await
        .expect("the constructor");
    let total = accumulators
        .call_total(&mut store, accumulator)
        .await
        .expect("total");
    record.push(format!("accumulator total -> {total}"));
    for code in [0, 1] {
        let read = calls
            .call_call_read(&mut store, code)
            .await
            .expect("call-read");
        record.push(format!("call-read {code} -> {}", read_result(read)));
    }
    let (answer, lost) = store
        .run_concurrent(async |accessor| {
            let answer = calls.call_call_answer(accessor, 21).await;
            let lost = calls.call_call_lost(accessor).await;
            (answer, lost)
        })
        .await
        .expect("run the calls");
    record.push(format!("call-answer -> {}", answer.expect("call-answer")));
    record.push(format!("call-lost -> {}", lost.expect("call-lost")));
    let checked = calls
        .call_call_checked(&mut store, 1)
        .await
        .expect("call-checked");
    record.push(format!("call-checked 1 -> {checked}"));
    let refused = calls
        .call_call_checked(&mut store, 0)
        .await
        .expect_err("the core refuses zero");
    record.push(format!("call-checked 0 -> {}", core_error(&refused)));
    record.extend(
        store
            .data()
            .core
            .log
            .iter()
            .map(|line| format!("core: {line}")),
    );

    let (mut store, world) = instantiate(&engine, &component).await;
    let broken = world
        .test_dual_guest()
        .call_call_read(&mut store, 2)
        .await
        .expect_err("`convert_fs_error` traps");
    record.push(format!("call-read 2 -> {}", core_error(&broken)));
    record
}
