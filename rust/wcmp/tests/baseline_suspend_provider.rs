// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Baseline tests for the suspend provider an engine selects.
//!
//! An engine selects the provider that fills its suspend capability
//! once, when it is constructed, and answers which one through
//! `Engine::suspend_provider`. The host's opt-out on `EngineConfig`
//! comes ahead of every probe. The switch probe passes on an engine over
//! the Wasmtime backend on x86_64 Linux, where Wasmtime implements the
//! stack-switching proposal, so the engine answers the stack-switching
//! provider there. The browser's backend declares host suspension in a
//! browser that offers JavaScript Promise Integration, which the
//! flake's Chromium does, so the engine answers the host-suspension
//! provider in the web lane. The Wasmi backend declares host suspension
//! on every platform, and the switch probe fails on it, so an engine over
//! it answers the host-suspension provider in the Wasmi lane. An engine
//! over Wasmtime on any other native platform answers that it has no
//! provider.
//!
//! Under the host-suspension provider, every export call in the browser
//! runs its thread as a resumable call. A call that never suspends costs
//! no more than a plain call there: it ends on its first poll, and hooks
//! no promise. One that traps in its first stretch learns the reason only
//! from its promise, so its driver awaits the call, and reports the trap
//! with the engine's reason.

#![cfg(test)]

use wcmp::{Engine, EngineConfig, SuspendProviderKind};

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// What an engine that is allowed a provider answers on this target,
/// over the backend of the run.
fn selected() -> SuspendProviderKind {
    if cfg!(target_arch = "wasm32") || crate::test_backend::name() == "wasmi" {
        SuspendProviderKind::HostSuspension
    } else if cfg!(all(target_arch = "x86_64", target_os = "linux")) {
        SuspendProviderKind::StackSwitching
    } else {
        SuspendProviderKind::None
    }
}

#[wcmp_macros::test]
fn it_answers_no_provider_when_the_config_turns_the_provider_off() {
    let mut config = EngineConfig::new();
    config.suspend_provider(false);
    let engine = Engine::with_backend(crate::test_backend::backend())
        .and_then(|engine| engine.with_config(&config))
        .expect("engine");

    assert_eq!(engine.suspend_provider(), SuspendProviderKind::None);
}

#[wcmp_macros::test]
fn it_answers_the_provider_its_target_and_backend_offer() {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    assert_eq!(
        engine.suspend_provider(),
        selected(),
        "an engine over `{}` selects what the backend and the target offer",
        crate::test_backend::name()
    );

    let engine = Engine::with_backend(crate::test_backend::backend())
        .and_then(|engine| engine.with_config(&EngineConfig::new()))
        .expect("engine");
    assert_eq!(engine.suspend_provider(), selected());
}

#[wcmp_macros::test]
fn it_keeps_the_answer_it_selected_at_construction() {
    let mut config = EngineConfig::new();
    config.suspend_provider(false);
    let engine = Engine::with_backend(crate::test_backend::backend())
        .and_then(|engine| engine.with_config(&config))
        .expect("engine");

    // Turning the provider back on in the host's configuration
    // reaches neither the engine already built from it nor a clone
    // of that engine: the selection ran once, in construction.
    config.suspend_provider(true);
    let clone = engine.clone();

    assert_eq!(engine.suspend_provider(), SuspendProviderKind::None);
    assert_eq!(clone.suspend_provider(), SuspendProviderKind::None);

    // And an engine built from the configuration now selects what
    // this target offers.
    let rebuilt = Engine::with_backend(crate::test_backend::backend())
        .and_then(|engine| engine.with_config(&config))
        .expect("engine");
    assert_eq!(rebuilt.suspend_provider(), selected());
}

/// Poll `future` once, and answer its output where it is ready.
#[cfg(target_arch = "wasm32")]
fn poll_once<F: core::future::Future>(future: F) -> core::task::Poll<F::Output> {
    let mut future = core::pin::pin!(future);
    future.as_mut().poll(&mut core::task::Context::from_waker(
        core::task::Waker::noop(),
    ))
}

/// Run `body`, and count the calls of `Promise.prototype.then`
/// meanwhile: the hook through which the browser's backend learns that
/// the promise of a resumable call settled.
#[cfg(target_arch = "wasm32")]
fn promise_hooks<R>(body: impl FnOnce() -> R) -> (R, u32) {
    use std::cell::Cell;
    use std::rc::Rc;

    use js_sys::{Function, Object, Proxy, Reflect};
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::{JsCast, JsValue};

    let property = |target: &JsValue, name: &str| {
        Reflect::get(target, &name.into()).expect("the property reads")
    };
    let prototype = property(&property(&js_sys::global(), "Promise"), "prototype");
    let then = property(&prototype, "then");
    let count = Rc::new(Cell::new(0));
    let apply = Closure::<dyn Fn(JsValue, JsValue, JsValue) -> Result<JsValue, JsValue>>::new({
        let count = count.clone();
        move |target: JsValue, this: JsValue, args: JsValue| {
            count.set(count.get() + 1);
            Reflect::apply(
                target.unchecked_ref::<Function>(),
                &this,
                args.unchecked_ref(),
            )
        }
    });
    let handler = Object::new();
    Reflect::set(&handler, &"apply".into(), apply.as_ref()).expect("the handler is an object");
    let counting = Proxy::new(&then, &handler);
    Reflect::set(&prototype, &"then".into(), &counting).expect("the prototype is writable");
    let result = body();
    Reflect::set(&prototype, &"then".into(), &then).expect("the prototype is writable");
    (result, count.get())
}

/// A component whose export `double` doubles a `u32` and never
/// suspends.
#[cfg(target_arch = "wasm32")]
const DOUBLE: &[u8] = wcmp_macros::component!(
    r#"
    (component
      (core module $m
        (func (export "double") (param i32) (result i32)
          local.get 0
          i32.const 2
          i32.mul))
      (core instance $i (instantiate $m))
      (func (export "double") (param "x" u32) (result u32)
        (canon lift (core func $i "double"))))
    "#
);

#[cfg(target_arch = "wasm32")]
#[wcmp_macros::test]
async fn it_calls_an_export_that_never_suspends_without_a_promise_hook() {
    use wcmp::{Component, Linker, Store, Val};

    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    assert_eq!(
        engine.suspend_provider(),
        SuspendProviderKind::HostSuspension
    );
    let component = Component::new(&engine, DOUBLE)
        .await
        .expect("the component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiation succeeds");
    let double = instance.get_func("double").expect("the export");

    // The call runs its thread under the host-suspension provider, as a
    // resumable call of the browser's backend. A thread that never
    // suspends ends in the turn that started it, on the first poll of
    // the call, and hooks no promise: as a plain call, which hooks none.
    for x in [21, 4] {
        let (outcome, hooks) = promise_hooks(|| poll_once(double.call(&mut store, &[Val::U32(x)])));
        let core::task::Poll::Ready(results) = outcome else {
            panic!("a call that never suspends ends on its first poll");
        };
        let results = results.expect("the call succeeds");
        assert_eq!(results.as_ref(), [Val::U32(x * 2)]);
        assert_eq!(hooks, 0, "a call that never suspends hooks no promise");
    }
}

/// A component whose export `trap` traps at once, and whose export `ok`
/// answers 9.
#[cfg(target_arch = "wasm32")]
const TRAP: &[u8] = wcmp_macros::component!(
    r#"
    (component
      (core module $m
        (func (export "trap") unreachable)
        (func (export "ok") (result i32) i32.const 9))
      (core instance $i (instantiate $m))
      (func (export "trap") (canon lift (core func $i "trap")))
      (func (export "ok") (result u32) (canon lift (core func $i "ok"))))
    "#
);

#[cfg(target_arch = "wasm32")]
#[wcmp_macros::test]
async fn it_reports_the_reason_of_a_trap_in_the_first_stretch_of_a_start_in_place() {
    use core::future::Future as _;
    use core::task::{Context, Waker};

    use wcmp::{Component, Error, Linker, Store, TaskCause};

    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, TRAP)
        .await
        .expect("the component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiation succeeds");
    let trap = instance.get_func("trap").expect("the export");
    let ok = instance.get_func("ok").expect("the export");

    // The store runs no guest code, so the provider starts the thread in
    // place. The browser tells why a call trapped only once it settles
    // the call's promise, so the start is still under way after its
    // first stretch, and the driver awaits it to learn the reason.
    let error = {
        let mut call = core::pin::pin!(trap.call(&mut store, &[]));
        let first = call.as_mut().poll(&mut Context::from_waker(Waker::noop()));
        assert!(
            first.is_pending(),
            "the trap reaches the driver once the browser settles the call"
        );
        call.await.expect_err("the call traps")
    };
    let mut described = format!("{error:?}");
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&error);
    while let Some(link) = source {
        described.push_str(&format!(": {link}"));
        source = link.source();
    }
    assert!(
        described.contains("unreachable"),
        "the driver reports the engine's reason for the trap, got {described}"
    );

    // The trap poisoned the store.
    let refused = ok
        .call(&mut store, &[])
        .await
        .expect_err("a poisoned store refuses the call");
    assert!(
        matches!(refused, Error::Task(TaskCause::CannotEnter)),
        "{refused:?}"
    );
}

#[path = "support/backend.rs"]
mod test_backend;
