// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The benchmarks themselves: what is measured, on which guest, and
//! with which payload.
//!
//! Each definition is written once. The runner drives it on a native
//! host and, compiled to `wasm32-unknown-unknown`, in a browser;
//! nothing below is written per target.

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::sync::{Arc, Mutex};

use wcmp::{
    Accessor, Component, Engine, EngineConfig, ExternType, Func, FunctionType, HostCall, Instance,
    InterfaceIdentifier, Linker, Store, Val, ValField,
};

use crate::benchmark::Benchmark;
use crate::error::{Error, Result};
use crate::guests;
use crate::run::Run;

/// Every benchmark of the suite, in the order a report lists them:
/// the call floor first, then the canonical ABI's heap values, the
/// typed path's type check, then handles, composition, host calls in
/// flight, and parsing.
pub fn benchmarks() -> Vec<Benchmark> {
    [
        u32_call(),
        u32_call_typed(),
        f64_call(),
        f64_call_typed(),
        string_roundtrip(),
        string_roundtrip_typed(),
        list_u8_roundtrip(),
        list_u8_roundtrip_typed(),
        list_u32_roundtrip(),
        list_u32_roundtrip_typed(),
        list_record_roundtrip(),
        list_record_roundtrip_typed(),
        typed_acquire(),
        instantiate_typed_host(),
        instantiate_untyped_host(),
        resource_handle(),
        composition_call(),
        host_calls_in_flight(),
        yields(),
        component_new(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// The variable that names the backend the suite measures natively:
/// `wasmtime`, the default, or `wasmi`. One native build measures
/// either, so `bench wasmi` runs the binary `bench native` built.
#[cfg(not(target_arch = "wasm32"))]
pub const BACKEND_VARIABLE: &str = "WCMP_BENCH_BACKEND";

/// The backend the suite measures on this run, as a report names it:
/// natively the one `WCMP_BENCH_BACKEND` names, Wasmtime when it is
/// unset, and in a browser the browser's own engine.
#[cfg(not(target_arch = "wasm32"))]
pub fn backend() -> Result<&'static str> {
    match std::env::var(BACKEND_VARIABLE) {
        Err(std::env::VarError::NotPresent) => Ok("wasmtime"),
        Ok(name) if name == "wasmtime" => Ok("wasmtime"),
        Ok(name) if name == "wasmi" => Ok("wasmi"),
        Ok(name) => Err(Error::Setup(format!(
            "{BACKEND_VARIABLE}={name} names no backend (`wasmtime` or `wasmi`)"
        ))),
        Err(error) => Err(Error::Setup(format!("{BACKEND_VARIABLE}: {error}"))),
    }
}

/// The backend the suite measures on this run, as a report names it:
/// natively the one `WCMP_BENCH_BACKEND` names, Wasmtime when it is
/// unset, and in a browser the browser's own engine.
#[cfg(target_arch = "wasm32")]
#[allow(clippy::unnecessary_wraps)]
pub fn backend() -> Result<&'static str> {
    Ok("web")
}

/// An engine over the backend the suite measures on this run:
/// Wasmtime or Wasmi natively, as [`backend`] answers, and the
/// browser's own engine in a browser.
#[cfg(not(target_arch = "wasm32"))]
fn backend_engine() -> Result<Engine> {
    if backend()? == "wasmi" {
        return Ok(Engine::with_backend(wcmp_wasm_core_wasmi::Wasmi::new())?);
    }
    let backend = wcmp_wasm_core_wasmtime::Wasmtime::new()
        .map_err(|error| Error::Setup(format!("Wasmtime makes no engine: {error}")))?;
    Ok(Engine::with_backend(backend)?)
}

/// An engine over the backend the suite measures on this run:
/// Wasmtime or Wasmi natively, as [`backend`] answers, and the
/// browser's own engine in a browser.
#[cfg(target_arch = "wasm32")]
fn backend_engine() -> Result<Engine> {
    Ok(Engine::with_backend(wcmp_wasm_core_web::Web::new())?)
}

/// An engine over the backend the suite measures on this run, with the
/// suspend provider turned off where `run`'s plan says `provider=off`.
fn engine(run: &Run) -> Result<Engine> {
    let engine = backend_engine()?;
    if run.plan().provider {
        return Ok(engine);
    }
    let mut config = EngineConfig::new();
    config.suspend_provider(false);
    Ok(engine.with_config(&config)?)
}

/// Instantiate `bytes` into a store of its own, and hand back the
/// engine, the store, and the instance.
///
/// Every benchmark calls this before its loop, so building a guest is
/// never part of a sample — except in `component-new`, where parsing
/// one is the thing measured.
async fn instantiate(run: &Run, bytes: &[u8]) -> Result<(Engine, Store<()>, Instance)> {
    let engine = engine(run)?;
    let component = Component::new(&engine, bytes).await?;
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ())?;
    let instance = linker.instantiate(&mut store, &component).await?;
    Ok((engine, store, instance))
}

/// The instance's export named `name`, or the reason there is none.
fn export(instance: &Instance, name: &str) -> Result<Func> {
    instance
        .get_func(name)
        .ok_or_else(|| Error::Setup(format!("the guest exports no `{name}`")))
}

#[wcmp_macros::bench(
    guest = "the `guest` corpus fixture: `double: func(x: u32) -> u32`, lifted with neither a memory nor a realloc",
    payload = "one u32 in and one u32 out, as `Val`: the floor, with no memory traffic under it"
)]
async fn u32_call(run: &mut Run) -> Result<()> {
    let (_engine, mut store, instance) = instantiate(run, guests::GUEST).await?;
    let double = export(&instance, "double")?;
    let arguments = [Val::U32(21)];
    while run.iterate() {
        double.call(&mut store, &arguments).await?;
    }
    Ok(())
}

#[wcmp_macros::bench(
    guest = "the `guest` corpus fixture: `double: func(x: u32) -> u32`, lifted with neither a memory nor a realloc",
    payload = "the same u32 in and out through `TypedFunc`, which is the floor without the untyped `Val` path's allocation"
)]
async fn u32_call_typed(run: &mut Run) -> Result<()> {
    let (_engine, mut store, instance) = instantiate(run, guests::GUEST).await?;
    let double = export(&instance, "double")?.typed::<(u32,), (u32,)>()?;
    while run.iterate() {
        double.call(&mut store, (21,)).await?;
    }
    Ok(())
}

#[wcmp_macros::bench(
    guest = "the inline `half` component: `half: func(x: f64) -> f64`, lifted with neither a memory nor a realloc",
    payload = "one f64 in and one f64 out, as `Val`: the `u32` floor with a float, whose bits cross the browser's core call as a `BigInt`"
)]
async fn f64_call(run: &mut Run) -> Result<()> {
    let (_engine, mut store, instance) = instantiate(run, guests::HALF).await?;
    let half = export(&instance, "half")?;
    let arguments = [Val::F64(21.0)];
    while run.iterate() {
        half.call(&mut store, &arguments).await?;
    }
    Ok(())
}

#[wcmp_macros::bench(
    guest = "the inline `half` component: `half: func(x: f64) -> f64`, lifted with neither a memory nor a realloc",
    payload = "the same f64 in and out through `TypedFunc`, the typed floor with a float"
)]
async fn f64_call_typed(run: &mut Run) -> Result<()> {
    let (_engine, mut store, instance) = instantiate(run, guests::HALF).await?;
    let half = export(&instance, "half")?.typed::<(f64,), (f64,)>()?;
    while run.iterate() {
        half.call(&mut store, (21.0,)).await?;
    }
    Ok(())
}

#[wcmp_macros::bench(
    guest = "the inline `echo` component: `echo-string: func(s: string) -> string`, which returns the pointer and length it was given",
    payload = "a UTF-8 string of the case's bytes, lowered into guest memory and lifted back out",
    cases = [64, 4096, 65536]
)]
async fn string_roundtrip(run: &mut Run) -> Result<()> {
    let size = usize::try_from(run.case().number()).unwrap_or(usize::MAX);
    let (_engine, mut store, instance) = instantiate(run, guests::ECHO).await?;
    let echo = export(&instance, "echo-string")?;
    let arguments = [Val::String("a".repeat(size))];
    run.moves_bytes(run.case().number());
    while run.iterate() {
        echo.call(&mut store, &arguments).await?;
    }
    Ok(())
}

#[wcmp_macros::bench(
    guest = "the inline `echo` component: `echo-string: func(s: string) -> string`, which returns the pointer and length it was given",
    payload = "the same UTF-8 string through `TypedFunc<(String,), (String,)>`, cloned into each call because the call takes it by value",
    cases = [64, 4096, 65536]
)]
async fn string_roundtrip_typed(run: &mut Run) -> Result<()> {
    let size = usize::try_from(run.case().number()).unwrap_or(usize::MAX);
    let (_engine, mut store, instance) = instantiate(run, guests::ECHO).await?;
    let echo = export(&instance, "echo-string")?.typed::<(String,), (String,)>()?;
    let payload = "a".repeat(size);
    run.moves_bytes(run.case().number());
    while run.iterate() {
        echo.call(&mut store, (payload.clone(),)).await?;
    }
    Ok(())
}

#[wcmp_macros::bench(
    guest = "the inline `echo` component: `echo-list-u8: func(xs: list<u8>) -> list<u8>`, which returns the pointer and length it was given",
    payload = "the case's number of u8 elements, the untyped counterpart of a string of the same bytes",
    cases = [64, 4096, 65536]
)]
async fn list_u8_roundtrip(run: &mut Run) -> Result<()> {
    let count = run.case().number();
    let (_engine, mut store, instance) = instantiate(run, guests::ECHO).await?;
    let echo = export(&instance, "echo-list-u8")?;
    let elements: Vec<Val> = (0..count).map(|index| Val::U8(index as u8)).collect();
    let arguments = [Val::List(elements.into_boxed_slice())];
    run.moves_bytes(count);
    while run.iterate() {
        echo.call(&mut store, &arguments).await?;
    }
    Ok(())
}

#[wcmp_macros::bench(
    guest = "the inline `echo` component: `echo-list-u8: func(xs: list<u8>) -> list<u8>`, which returns the pointer and length it was given",
    payload = "the case's number of u8 elements through `TypedFunc<(Vec<u8>,), (Vec<u8>,)>`, cloned into each call because the call takes them by value",
    cases = [64, 4096, 65536]
)]
async fn list_u8_roundtrip_typed(run: &mut Run) -> Result<()> {
    let count = run.case().number();
    let (_engine, mut store, instance) = instantiate(run, guests::ECHO).await?;
    let echo = export(&instance, "echo-list-u8")?.typed::<(Vec<u8>,), (Vec<u8>,)>()?;
    let payload: Vec<u8> = (0..count).map(|index| index as u8).collect();
    run.moves_bytes(count);
    while run.iterate() {
        echo.call(&mut store, (payload.clone(),)).await?;
    }
    Ok(())
}

#[wcmp_macros::bench(
    guest = "the inline `echo` component: `echo-list-u32: func(xs: list<u32>) -> list<u32>`, which returns the pointer and length it was given",
    payload = "the case's number of u32 elements, lowered in one write of the list's bytes and lifted in one read",
    cases = [16, 256, 4096]
)]
async fn list_u32_roundtrip(run: &mut Run) -> Result<()> {
    let count = run.case().number();
    let (_engine, mut store, instance) = instantiate(run, guests::ECHO).await?;
    let echo = export(&instance, "echo-list-u32")?;
    let elements: Vec<Val> = (0..count).map(|index| Val::U32(index as u32)).collect();
    let arguments = [Val::List(elements.into_boxed_slice())];
    run.moves_elements(count);
    while run.iterate() {
        echo.call(&mut store, &arguments).await?;
    }
    Ok(())
}

#[wcmp_macros::bench(
    guest = "the inline `echo` component: `echo-list-u32: func(xs: list<u32>) -> list<u32>`, which returns the pointer and length it was given",
    payload = "the case's number of u32 elements through `TypedFunc<(Vec<u32>,), (Vec<u32>,)>`, cloned into each call; the cases move the bytes of `list-u8-roundtrip-typed`'s, so the two compare a wide element with a byte at equal size",
    cases = [16, 1024, 16384]
)]
async fn list_u32_roundtrip_typed(run: &mut Run) -> Result<()> {
    let count = run.case().number();
    let (_engine, mut store, instance) = instantiate(run, guests::ECHO).await?;
    let echo = export(&instance, "echo-list-u32")?.typed::<(Vec<u32>,), (Vec<u32>,)>()?;
    let payload: Vec<u32> = (0..count).map(|index| index as u32).collect();
    run.moves_bytes(count * 4);
    while run.iterate() {
        echo.call(&mut store, (payload.clone(),)).await?;
    }
    Ok(())
}

#[wcmp_macros::bench(
    guest = "the inline `echo` component: `echo-list-point: func(xs: list<point>) -> list<point>` over `record point { x: u32, y: u32 }`",
    payload = "the case's number of two-field records, whose field names are carried as owned strings on every crossing",
    cases = [16, 256, 4096]
)]
async fn list_record_roundtrip(run: &mut Run) -> Result<()> {
    let count = run.case().number();
    let (_engine, mut store, instance) = instantiate(run, guests::ECHO).await?;
    let echo = export(&instance, "echo-list-point")?;
    let elements: Vec<Val> = (0..count)
        .map(|index| {
            Val::Record(
                vec![
                    ValField {
                        name: "x".to_owned(),
                        value: Val::U32(index as u32),
                    },
                    ValField {
                        name: "y".to_owned(),
                        value: Val::U32(index as u32),
                    },
                ]
                .into_boxed_slice(),
            )
        })
        .collect();
    let arguments = [Val::List(elements.into_boxed_slice())];
    run.moves_elements(count);
    while run.iterate() {
        echo.call(&mut store, &arguments).await?;
    }
    Ok(())
}

/// The record `point` of the `echo`, `shapes`, and host-import guests,
/// derived rather than written by hand.
#[derive(Clone, wcmp::ComponentType, wcmp::Lift, wcmp::Lower)]
#[component(record)]
struct Point {
    x: u32,
    y: u32,
}

/// The record `wide` of the `shapes` guest: sixteen `u32` fields.
#[derive(wcmp::ComponentType, wcmp::Lift, wcmp::Lower)]
#[component(record)]
struct Wide {
    f0: u32,
    f1: u32,
    f2: u32,
    f3: u32,
    f4: u32,
    f5: u32,
    f6: u32,
    f7: u32,
    f8: u32,
    f9: u32,
    f10: u32,
    f11: u32,
    f12: u32,
    f13: u32,
    f14: u32,
    f15: u32,
}

/// The variant `shape` of the `shapes` guest: a record inside a
/// variant.
#[derive(wcmp::ComponentType, wcmp::Lift, wcmp::Lower)]
#[component(variant)]
enum Shape {
    #[component(name = "dot")]
    Dot(Point),
    #[component(name = "empty")]
    Empty,
}

#[wcmp_macros::bench(
    guest = "the inline `echo` component: `echo-list-point: func(xs: list<point>) -> list<point>` over `record point { x: u32, y: u32 }`",
    payload = "the case's number of two-field records as a derived `Point` through `TypedFunc<(Vec<Point>,), (Vec<Point>,)>`, cloned into each call because the call takes it by value",
    cases = [16, 256, 4096]
)]
async fn list_record_roundtrip_typed(run: &mut Run) -> Result<()> {
    let count = run.case().number();
    let (_engine, mut store, instance) = instantiate(run, guests::ECHO).await?;
    let echo = export(&instance, "echo-list-point")?.typed::<(Vec<Point>,), (Vec<Point>,)>()?;
    let points: Vec<Point> = (0..count)
        .map(|index| Point {
            x: index as u32,
            y: index as u32,
        })
        .collect();
    run.moves_elements(count);
    while run.iterate() {
        echo.call(&mut store, (points.clone(),)).await?;
    }
    Ok(())
}

#[wcmp_macros::bench(
    guest = "one export per case: the `guest` fixture's `double: func(x: u32) -> u32` (`lookup` and `flat`), and the inline `shapes` component's `take-wide: func(w: wide)` over a record of sixteen `u32` fields (`record-16`) and `take-shapes: func(xs: list<shape>)` over `variant shape { dot(point), empty }` (`nested`)",
    payload = "one `Instance::get_func` and one `Func::typed` per iteration, which runs the type check of the Rust signature against the export's; `lookup` runs the `get_func` alone, since `Func::typed` takes the `Func` and each iteration needs one",
    cases = ["lookup", "flat", "record-16", "nested"]
)]
async fn typed_acquire(run: &mut Run) -> Result<()> {
    let shape = run.case().name();
    let (bytes, name) = match shape {
        "lookup" | "flat" => (guests::GUEST, "double"),
        "record-16" => (guests::SHAPES, "take-wide"),
        "nested" => (guests::SHAPES, "take-shapes"),
        other => return Err(Error::Setup(format!("`{other}` is not a shape"))),
    };
    let (_engine, _store, instance) = instantiate(run, bytes).await?;
    while run.iterate() {
        let func = export(&instance, name)?;
        match shape {
            "lookup" => drop(func),
            "flat" => drop(func.typed::<(u32,), (u32,)>()?),
            "record-16" => drop(func.typed::<(Wide,), ()>()?),
            _ => drop(func.typed::<(Vec<Shape>,), ()>()?),
        }
    }
    Ok(())
}

/// The interface the host-import guests import their functions from.
const POINTS: &str = "wcmp-bench:host/points@0.1.0";

/// The host-import guest of `count` imports, `f0` to `f(count - 1)`.
fn hosts(count: u64) -> Result<&'static [u8]> {
    match count {
        1 => Ok(guests::HOSTS_1),
        16 => Ok(guests::HOSTS_16),
        other => Err(Error::Setup(format!("no guest imports {other} functions"))),
    }
}

#[wcmp_macros::bench(
    guest = "the inline host-import component of the case's number of imports, each `f<i>: func(p: point) -> point` over `record point { x: u32, y: u32 }`, with no core module",
    payload = "one instantiation into a fresh store per iteration, against a linker whose imports are `func_wrap` host functions over a derived `Point`: the type check of each typed host function at link time",
    cases = [1, 16]
)]
async fn instantiate_typed_host(run: &mut Run) -> Result<()> {
    let count = run.case().number();
    let engine = engine(run)?;
    let component = Component::new(&engine, hosts(count)?).await?;
    let mut linker: Linker<()> = Linker::new(&engine);
    let points: InterfaceIdentifier = POINTS
        .parse()
        .map_err(|error| Error::Setup(format!("{POINTS}: {error}")))?;
    for index in 0..count {
        linker.instance(&points).func_wrap(
            format!("f{index}"),
            |_: HostCall<'_, ()>, (point,): (Point,)| Ok((point,)),
        )?;
    }
    run.moves_elements(count);
    while run.iterate() {
        let mut store: Store<()> = Store::new(&engine, ())?;
        linker.instantiate(&mut store, &component).await?;
    }
    Ok(())
}

#[wcmp_macros::bench(
    guest = "the inline host-import component of the case's number of imports, each `f<i>: func(p: point) -> point` over `record point { x: u32, y: u32 }`, with no core module",
    payload = "one instantiation into a fresh store per iteration, against a linker whose imports are `func_new` host functions of the declared type: the untyped counterpart of `instantiate-typed-host`",
    cases = [1, 16]
)]
async fn instantiate_untyped_host(run: &mut Run) -> Result<()> {
    let count = run.case().number();
    let engine = engine(run)?;
    let component = Component::new(&engine, hosts(count)?).await?;
    let ty = import_type(&component, "f0")?;
    let mut linker: Linker<()> = Linker::new(&engine);
    let points: InterfaceIdentifier = POINTS
        .parse()
        .map_err(|error| Error::Setup(format!("{POINTS}: {error}")))?;
    for index in 0..count {
        linker.instance(&points).func_new(
            format!("f{index}"),
            ty.clone(),
            |_: HostCall<'_, ()>, args: &[Val], results: &mut [Val]| {
                results[0] = args[0].clone();
                Ok(())
            },
        )?;
    }
    run.moves_elements(count);
    while run.iterate() {
        let mut store: Store<()> = Store::new(&engine, ())?;
        linker.instantiate(&mut store, &component).await?;
    }
    Ok(())
}

/// The type of the function `name` the host-import guest imports from
/// [`POINTS`], as the component declares it.
fn import_type(component: &Component, name: &str) -> Result<FunctionType> {
    component
        .imports
        .iter()
        .find_map(|import| match &import.ty {
            ExternType::Instance(instance) => {
                instance
                    .items
                    .iter()
                    .find_map(|item| match (&item.ty, item.name == name) {
                        (ExternType::Function(ty), true) => Some(ty.clone()),
                        _ => None,
                    })
            }
            _ => None,
        })
        .ok_or_else(|| Error::Setup(format!("the guest imports no `{name}`")))
}

#[wcmp_macros::bench(
    guest = "the inline `resource` component: `make: func(rep: u32) -> own<thing>` and `dispose: func(h: own<thing>)`, with a destructor that counts",
    payload = "one owned handle per iteration: minted by the guest, held by the host, handed back, and dropped"
)]
async fn resource_handle(run: &mut Run) -> Result<()> {
    let (_engine, mut store, instance) = instantiate(run, guests::RESOURCE).await?;
    let make = export(&instance, "make")?;
    let dispose = export(&instance, "dispose")?;
    let arguments = [Val::U32(7)];
    run.moves_elements(1);
    while run.iterate() {
        let results = make.call(&mut store, &arguments).await?;
        let handle = match results.first() {
            Some(Val::Resource(handle)) if handle.owned() => *handle,
            _ => {
                return Err(Error::Setup(
                    "`make` did not return an owned handle".to_owned(),
                ));
            }
        };
        dispose.call(&mut store, &[Val::Resource(handle)]).await?;
    }
    Ok(())
}

#[wcmp_macros::bench(
    guest = "the `composition` corpus fixture: a socket's `run: func(x: u32) -> u32` reaching a plug's `double` through the adapter `wac plug` wrote between them",
    payload = "one u32 in and one u32 out, across two component instances instead of one"
)]
async fn composition_call(run: &mut Run) -> Result<()> {
    let (_engine, mut store, instance) = instantiate(run, guests::COMPOSITION).await?;
    let call_run = export(&instance, "run")?;
    let arguments = [Val::U32(20)];
    while run.iterate() {
        call_run.call(&mut store, &arguments).await?;
    }
    Ok(())
}

/// How many times one call of [`yields`] gives way.
const YIELDS: u32 = 16;

#[wcmp_macros::bench(
    guest = "a callback export assembled from text, `spin: async func(n: u32)`, whose core function and callbacks answer `YIELD` until `n` runs out",
    payload = "one call that yields 16 times, so a sample is 16 rounds through the driver's wake after a yield, which in a browser posts a message to the driver's own channel each time"
)]
async fn yields(run: &mut Run) -> Result<()> {
    let (_engine, mut store, instance) = instantiate(run, guests::YIELDS).await?;
    let spin = export(&instance, "spin")?;
    let arguments = [Val::U32(YIELDS)];
    while run.iterate() {
        spin.call(&mut store, &arguments).await?;
    }
    Ok(())
}

/// The host side of [`host_calls_in_flight`]: the calls of one
/// iteration resolve one at a time, the last one the guest made
/// first, each one waking the call made before it as it completes.
///
/// That is the shape that tells a store which polls every host task
/// on every turn from one that polls only the tasks that were woken.
/// A call is woken by the one made after it, which a store that polls
/// its tasks in the order they joined has already polled in that
/// pass, so every turn has one call to finish and all the others
/// still pending: the first kind of store does work in the square of
/// the number of calls and the second in the number itself.
#[derive(Clone, Default)]
struct Relay(Arc<Mutex<RelayState>>);

/// Where the relay stands within one iteration.
#[derive(Default)]
struct RelayState {
    /// The index of the call whose turn it is to complete.
    next: u32,
    /// The waker each pending call left, by the index of the call.
    wakers: Vec<Option<Waker>>,
}

impl Relay {
    /// Make ready for an iteration of `calls` calls, the last of
    /// which is the next to complete.
    fn reset(&self, calls: u32) {
        if let Ok(mut state) = self.0.lock() {
            state.next = calls.saturating_sub(1);
            state.wakers.clear();
            state.wakers.resize(calls as usize, None);
        }
    }

    /// The future of the call with index `index`.
    fn call(&self, index: u32) -> RelayCall {
        RelayCall {
            relay: self.clone(),
            index,
            polled: false,
        }
    }
}

/// One call's future: pending on its first poll whatever its turn,
/// so the guest sees every call start, and ready on a later poll once
/// every call made after it has completed.
struct RelayCall {
    relay: Relay,
    index: u32,
    polled: bool,
}

impl Future for RelayCall {
    type Output = core::result::Result<(u32,), wcmp::Error>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let Ok(mut state) = this.relay.0.lock() else {
            return Poll::Pending;
        };
        if this.polled && state.next == this.index {
            let woken = match this.index.checked_sub(1) {
                Some(before) => {
                    state.next = before;
                    state.wakers.get_mut(before as usize).and_then(Option::take)
                }
                None => None,
            };
            drop(state);
            if let Some(waker) = woken {
                waker.wake();
            }
            return Poll::Ready(Ok((this.index,)));
        }
        let first = !this.polled;
        this.polled = true;
        if let Some(slot) = state.wakers.get_mut(this.index as usize) {
            *slot = Some(context.waker().clone());
        }
        let turn = state.next == this.index;
        drop(state);
        if first && turn {
            context.waker().wake_by_ref();
        }
        Poll::Pending
    }
}

#[wcmp_macros::bench(
    guest = "the inline `fan-out` component: `fan-out: async func(n: u32) -> u32`, which starts `n` calls of the async-typed host import `answer` and waits on them through one waitable set",
    payload = "the case's number of host calls in flight at once, completing one per turn, the last one made first, each waking the call made before it",
    cases = [8, 64, 512]
)]
async fn host_calls_in_flight(run: &mut Run) -> Result<()> {
    let calls = u32::try_from(run.case().number()).unwrap_or(u32::MAX);
    let relay = Relay::default();
    let engine = engine(run)?;
    let component = Component::new(&engine, guests::FAN_OUT).await?;
    let mut linker: Linker<()> = Linker::new(&engine);
    let answering = relay.clone();
    linker.root().func_wrap_concurrent(
        "answer",
        move |_accessor: &Accessor<()>, (index,): (u32,)| answering.call(index),
    )?;
    let mut store: Store<()> = Store::new(&engine, ())?;
    let instance = linker.instantiate(&mut store, &component).await?;
    let fan_out = export(&instance, "fan-out")?;
    let arguments = [Val::U32(calls)];
    run.moves_elements(u64::from(calls));
    while run.iterate() {
        relay.reset(calls);
        fan_out.call(&mut store, &arguments).await?;
    }
    Ok(())
}

#[wcmp_macros::bench(
    guest = "the smoke test's guests, one per case: the `guest`, `composition`, `maps`, and `fixed-lists` corpus fixtures",
    payload = "one component binary, parsed and translated by `Component::new`; the bytes are the binary's own size",
    cases = ["guest", "composition", "maps", "fixed-lists"]
)]
async fn component_new(run: &mut Run) -> Result<()> {
    let bytes = match run.case().name() {
        "guest" => guests::GUEST,
        "composition" => guests::COMPOSITION,
        "maps" => guests::MAPS,
        "fixed-lists" => guests::FIXED_LISTS,
        other => {
            return Err(Error::Setup(format!(
                "`{other}` is not one of the smoke test's guests"
            )));
        }
    };
    let engine = engine(run)?;
    run.moves_bytes(bytes.len() as u64);
    while run.iterate() {
        Component::new(&engine, bytes).await?;
    }
    Ok(())
}
