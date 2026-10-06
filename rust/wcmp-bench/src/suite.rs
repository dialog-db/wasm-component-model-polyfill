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

use wcmp::{Accessor, Component, Engine, Func, Instance, Linker, Store, Val, ValField};

use crate::benchmark::Benchmark;
use crate::error::{Error, Result};
use crate::guests;
use crate::run::Run;

/// Every benchmark of the suite, in the order a report lists them:
/// the call floor first, then the canonical ABI's heap values, then
/// handles, composition, host calls in flight, and parsing.
pub fn benchmarks() -> Vec<Benchmark> {
    [
        u32_call(),
        u32_call_typed(),
        string_roundtrip(),
        string_roundtrip_typed(),
        list_u8_roundtrip(),
        list_u8_roundtrip_typed(),
        list_u32_roundtrip(),
        list_record_roundtrip(),
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
fn engine() -> Result<Engine> {
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
fn engine() -> Result<Engine> {
    Ok(Engine::with_backend(wcmp_wasm_core_web::Web::new())?)
}

/// Instantiate `bytes` into a store of its own, and hand back the
/// engine, the store, and the instance.
///
/// Every benchmark calls this before its loop, so building a guest is
/// never part of a sample — except in `component-new`, where parsing
/// one is the thing measured.
async fn instantiate(bytes: &[u8]) -> Result<(Engine, Store<()>, Instance)> {
    let engine = engine()?;
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
    let (_engine, mut store, instance) = instantiate(guests::GUEST).await?;
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
    let (_engine, mut store, instance) = instantiate(guests::GUEST).await?;
    let double = export(&instance, "double")?.typed::<(u32,), u32>()?;
    while run.iterate() {
        double.call(&mut store, (21,)).await?;
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
    let (_engine, mut store, instance) = instantiate(guests::ECHO).await?;
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
    payload = "the same UTF-8 string through `TypedFunc<(String,), String>`, cloned into each call because the call takes it by value",
    cases = [64, 4096, 65536]
)]
async fn string_roundtrip_typed(run: &mut Run) -> Result<()> {
    let size = usize::try_from(run.case().number()).unwrap_or(usize::MAX);
    let (_engine, mut store, instance) = instantiate(guests::ECHO).await?;
    let echo = export(&instance, "echo-string")?.typed::<(String,), String>()?;
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
    let (_engine, mut store, instance) = instantiate(guests::ECHO).await?;
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
    payload = "the case's number of u8 elements through `TypedFunc<(Vec<u8>,), Vec<u8>>`, cloned into each call because the call takes them by value",
    cases = [64, 4096, 65536]
)]
async fn list_u8_roundtrip_typed(run: &mut Run) -> Result<()> {
    let count = run.case().number();
    let (_engine, mut store, instance) = instantiate(guests::ECHO).await?;
    let echo = export(&instance, "echo-list-u8")?.typed::<(Vec<u8>,), Vec<u8>>()?;
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
    let (_engine, mut store, instance) = instantiate(guests::ECHO).await?;
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
    guest = "the inline `echo` component: `echo-list-point: func(xs: list<point>) -> list<point>` over `record point { x: u32, y: u32 }`",
    payload = "the case's number of two-field records, whose field names are carried as owned strings on every crossing",
    cases = [16, 256, 4096]
)]
async fn list_record_roundtrip(run: &mut Run) -> Result<()> {
    let count = run.case().number();
    let (_engine, mut store, instance) = instantiate(guests::ECHO).await?;
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

#[wcmp_macros::bench(
    guest = "the inline `resource` component: `make: func(rep: u32) -> own<thing>` and `dispose: func(h: own<thing>)`, with a destructor that counts",
    payload = "one owned handle per iteration: minted by the guest, held by the host, handed back, and dropped"
)]
async fn resource_handle(run: &mut Run) -> Result<()> {
    let (_engine, mut store, instance) = instantiate(guests::RESOURCE).await?;
    let make = export(&instance, "make")?;
    let dispose = export(&instance, "dispose")?;
    let arguments = [Val::U32(7)];
    run.moves_elements(1);
    while run.iterate() {
        let results = make.call(&mut store, &arguments).await?;
        let handle = match results.first() {
            Some(Val::Own(handle)) => *handle,
            _ => {
                return Err(Error::Setup(
                    "`make` did not return an owned handle".to_owned(),
                ));
            }
        };
        dispose.call(&mut store, &[Val::Own(handle)]).await?;
    }
    Ok(())
}

#[wcmp_macros::bench(
    guest = "the `composition` corpus fixture: a socket's `run: func(x: u32) -> u32` reaching a plug's `double` through the adapter `wac plug` wrote between them",
    payload = "one u32 in and one u32 out, across two component instances instead of one"
)]
async fn composition_call(run: &mut Run) -> Result<()> {
    let (_engine, mut store, instance) = instantiate(guests::COMPOSITION).await?;
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
    payload = "one call that yields 16 times, so a sample is 16 rounds through the driver's wake after a yield, which in a browser posts a message to a channel of its own each time"
)]
async fn yields(run: &mut Run) -> Result<()> {
    let (_engine, mut store, instance) = instantiate(guests::YIELDS).await?;
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
    type Output = core::result::Result<u32, wcmp::Error>;

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
            return Poll::Ready(Ok(this.index));
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
    let engine = engine()?;
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
    let engine = engine()?;
    run.moves_bytes(bytes.len() as u64);
    while run.iterate() {
        Component::new(&engine, bytes).await?;
    }
    Ok(())
}
