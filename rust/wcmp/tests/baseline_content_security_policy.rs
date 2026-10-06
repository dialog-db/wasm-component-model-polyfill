// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Baseline tests for the polyfill under a strict content security
//! policy, in the browser.
//!
//! A hardened page sets `script-src 'self' 'wasm-unsafe-eval'`: script
//! only from its own origin, and the grant WebAssembly compilation
//! needs, with nothing that turns source text into a function. The
//! polyfill has to run under exactly that much. It compiles modules of
//! its own at run time — the fused adapters of a composition, and the
//! browser backend's wrapper modules through which a guest calls a
//! host function — and each of those is WebAssembly, which
//! `'wasm-unsafe-eval'` admits. None of it may be JavaScript built from
//! a string, which the policy refuses.
//!
//! Each test installs the policy on its own page before it touches the
//! polyfill, and shows the browser enforces it by building a function
//! from source text and watching the browser refuse. Everything the
//! test does with the polyfill after that point happens under the
//! policy: every engine, component, store, host function, and module
//! the polyfill compiles. What the page loaded before the policy — the
//! test binary and the `wasm-bindgen` glue around it — runs because it
//! loaded first: the runner serves it from an origin of its own, apart
//! from the page's, which `'self'` would refuse had the policy been in
//! place. The policy does not take back script that already runs. The
//! smoke page (`rust/wcmp-smoke/web`) declares the same policy in its
//! markup and loads its script from its own origin, so it covers
//! loading.
//!
//! The runner opens a fresh tab on a fresh origin for every test, so a
//! policy one test installs does not reach another. The entry check of
//! [`install_the_policy`] would say so if that ever changed.

#![cfg(all(test, target_arch = "wasm32"))]

use std::sync::{Arc, Mutex};

use wasm_bindgen::prelude::wasm_bindgen;
use wcmp::{
    Component, Engine, FunctionParameter, FunctionType, Linker, PrimitiveType, Store, Val,
    ValueType,
};
use wcmp_macros::component;

wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// The policy a hardened page sets.
const POLICY: &str = "script-src 'self' 'wasm-unsafe-eval'";

/// A callee lifted asynchronously with a callback, called through a
/// synchronous lower by a sync-typed caller. A call between the two
/// goes through the adapter's prepare intrinsic, a host function of
/// eight fixed parameters and then the caller's one flat argument, and
/// the start intrinsic after it. The caller adds one to what the
/// callee doubled, so the result says the value crossed both ways.
const PREPARES_A_CALL: &[u8] = component!(
    r#"
    (component
      (component $callee
        (core func $task-return (canon task.return (result u32)))
        (core module $m
          (import "" "task.return" (func $task-return (param i32)))
          (func (export "cb") (param i32 i32 i32) (result i32) unreachable)
          (func (export "answer") (param i32) (result i32)
            (call $task-return (i32.mul (local.get 0) (i32.const 2)))
            (i32.const 0)))
        (core instance $i (instantiate $m
          (with "" (instance (export "task.return" (func $task-return))))))
        (func (export "answer") async (param "x" u32) (result u32)
          (canon lift (core func $i "answer") async (callback (core func $i "cb")))))
      (component $caller
        (import "answer" (func $answer async (param "x" u32) (result u32)))
        (core func $lowered (canon lower (func $answer)))
        (core module $m
          (import "" "answer" (func $answer (param i32) (result i32)))
          (func (export "run") (param i32) (result i32)
            (i32.add (call $answer (local.get 0)) (i32.const 1))))
        (core instance $i (instantiate $m
          (with "" (instance (export "answer" (func $lowered))))))
        (func (export "run") (param "x" u32) (result u32)
          (canon lift (core func $i "run"))))
      (instance $a (instantiate $callee))
      (instance $b (instantiate $caller (with "answer" (func $a "answer"))))
      (export "run" (func $b "run")))
    "#
);

/// A guest that imports a host function of nine `u32` parameters and
/// calls it with one through nine. Nine flat parameters are fewer than
/// the canonical ABI's limit of sixteen, so the lowered core function
/// takes all nine as `i32` parameters, and the browser backend builds
/// its wrapper module for a host function of that type.
const CALLS_NINE_PARAMETERS: &[u8] = component!(
    r#"
    (component
      (import "sum" (func $sum
        (param "a" u32) (param "b" u32) (param "c" u32)
        (param "d" u32) (param "e" u32) (param "f" u32)
        (param "g" u32) (param "h" u32) (param "i" u32)
        (result u32)))
      (core func $lowered (canon lower (func $sum)))
      (core module $m
        (import "" "sum" (func $sum
          (param i32 i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
        (func (export "total") (result i32)
          (call $sum
            (i32.const 1) (i32.const 2) (i32.const 3)
            (i32.const 4) (i32.const 5) (i32.const 6)
            (i32.const 7) (i32.const 8) (i32.const 9))))
      (core instance $i (instantiate $m
        (with "" (instance (export "sum" (func $lowered))))))
      (func (export "total") (result u32)
        (canon lift (core func $i "total"))))
    "#
);

/// How many parameters the host function of [`CALLS_NINE_PARAMETERS`]
/// takes. Eight is the most a `wasm-bindgen` closure takes, so a
/// backend that handed a guest one closure per host function would
/// need a JavaScript shim of its own for nine, and building that shim
/// from source text is what the policy refuses. The browser backend
/// passes a call's arguments one at a time through a wrapper module
/// instead; nine parameters hold it to that.
const PARAMETERS: u32 = 9;

// `install_policy` adds the policy to the document, which the browser
// then enforces for everything the page does from that point on.
// `builds_a_function_from_source` reports whether the page can still
// turn source text into a function, which is what the policy forbids
// and what the tests use to tell an enforced policy from a lost one.
//
// `builds_a_function_from_source` is the same probe the smoke page's
// `boot.js` runs, written the same way on purpose: both build a
// function from source and call it, and read a throw as an enforced
// policy. The two are the only places the repository asks the browser
// that question, and an answer that differed between them would make
// one of the two artifacts measure something else. Change them
// together.
#[wasm_bindgen(inline_js = "export function install_policy(policy) {
    const meta = document.createElement('meta');
    meta.setAttribute('http-equiv', 'Content-Security-Policy');
    meta.setAttribute('content', policy);
    document.head.appendChild(meta);
}

export function builds_a_function_from_source() {
    try {
        return new Function('return 1')() === 1;
    } catch {
        return false;
    }
}
")]
extern "C" {
    fn install_policy(policy: &str);
    fn builds_a_function_from_source() -> bool;
}

/// Installs [`POLICY`] on the page, and fails unless the browser went
/// from admitting source text to refusing it.
///
/// A policy added to a document cannot be taken back, so everything
/// the calling test does after this runs under it.
fn install_the_policy() {
    assert!(
        builds_a_function_from_source(),
        "the page starts without a policy, so the check below measures \
         the policy this test installs and not one the runner brought, \
         or one an earlier test on the same page left behind"
    );
    install_policy(POLICY);
    assert!(
        !builds_a_function_from_source(),
        "the browser enforces the installed policy and refuses to build \
         a function from source text"
    );
}

/// An engine over the browser backend.
fn engine() -> Engine {
    Engine::with_backend(test_backend::backend()).expect("engine")
}

#[wcmp_macros::test]
fn it_refuses_source_text_once_the_policy_is_installed() {
    install_the_policy();
}

#[wcmp_macros::test]
async fn it_runs_a_composition_that_prepares_a_call_under_the_policy() {
    install_the_policy();

    let engine = engine();
    let component = Component::new(&engine, PREPARES_A_CALL)
        .await
        .expect("the composition compiles under the policy");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the composition instantiates under the policy");
    let run = instance.get_func("run").expect("the caller's export");

    let result = run
        .call(&mut store, &[Val::U32(21)])
        .await
        .expect("the prepared call runs under the policy");
    assert_eq!(
        result.as_ref(),
        &[Val::U32(43)],
        "the callee doubled the argument and the caller added one"
    );
}

#[wcmp_macros::test]
async fn it_calls_a_host_function_of_more_than_eight_parameters_under_the_policy() {
    install_the_policy();

    let engine = engine();
    let component = Component::new(&engine, CALLS_NINE_PARAMETERS)
        .await
        .expect("the component compiles under the policy");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = seen.clone();
    let ty = FunctionType {
        parameters: ('a'..)
            .take(PARAMETERS as usize)
            .map(|name| FunctionParameter {
                name: name.to_string(),
                ty: ValueType::Primitive(PrimitiveType::U32),
            })
            .collect(),
        result: Some(ValueType::Primitive(PrimitiveType::U32)),
        async_: false,
    };
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_new("sum", ty, move |_call, args, results| {
            let values: Vec<u32> = args
                .iter()
                .map(|arg| match arg {
                    Val::U32(value) => *value,
                    other => panic!("the type declares `u32` parameters, got {other:?}"),
                })
                .collect();
            results[0] = Val::U32(values.iter().sum());
            *recorded.lock().expect("record") = values;
            Ok(())
        })
        .expect("the registration of `sum`");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the component instantiates under the policy");
    let total = instance.get_func("total").expect("the guest's export");

    let result = total
        .call(&mut store, &[])
        .await
        .expect("the guest calls the host function under the policy");
    assert_eq!(
        *seen.lock().expect("record"),
        (1..=PARAMETERS).collect::<Vec<_>>(),
        "the host function received all nine arguments, in order"
    );
    assert_eq!(
        result.as_ref(),
        &[Val::U32((1..=PARAMETERS).sum())],
        "the guest read the host function's result"
    );
}

/// A stackful export of an `async` function type that lowers the
/// host's `async` function `answer` synchronously. A host future that
/// is not ready blocks the export's thread, which the browser backend
/// suspends through JavaScript Promise Integration.
const BLOCKS_ON_A_HOST_PROMISE: &[u8] = component!(
    r#"
    (component
      (import "answer" (func $answer async (param "x" u32) (result u32)))
      (core func $answer (canon lower (func $answer)))
      (core module $m
        (import "" "answer" (func $answer (param i32) (result i32)))
        (func (export "run") (param i32) (result i32)
          (call $answer (local.get 0))))
      (core instance $i (instantiate $m
        (with "" (instance (export "answer" (func $answer))))))
      (func (export "run") async (param "x" u32) (result u32)
        (canon lift (core func $i "run"))))
    "#
);

#[wcmp_macros::test]
async fn it_suspends_on_an_async_host_function_that_awaits_a_promise_under_the_policy() {
    // The host function awaits a JavaScript promise, which settles only
    // on a microtask, so the guest's synchronous lower blocks and its
    // thread suspends through `WebAssembly.Suspending` and
    // `WebAssembly.promising`, under the policy.
    install_the_policy();

    let mut config = wcmp::EngineConfig::new();
    config.wasm_component_model_async_stackful(true);
    let engine = engine().with_config(&config).expect("the engine");
    let component = Component::new(&engine, BLOCKS_ON_A_HOST_PROMISE)
        .await
        .expect("the component compiles under the policy");
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap_concurrent("answer", |_accessor: &wcmp::Accessor<()>, (x,): (u32,)| {
            let promise =
                js_sys::Promise::resolve(&wasm_bindgen::JsValue::from_f64(f64::from(x * 2)));
            async move {
                let resolved = wasm_bindgen_futures::JsFuture::from(promise)
                    .await
                    .expect("the promise resolves");
                Ok(resolved.as_f64().expect("the promise's value") as u32)
            }
        })
        .expect("the registration of `answer`");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the component instantiates under the policy");
    let result = instance
        .get_func("run")
        .expect("the guest's export")
        .call(&mut store, &[Val::U32(21)])
        .await
        .expect("the blocked call resolves under the policy");
    assert_eq!(
        result.as_ref(),
        &[Val::U32(42)],
        "the promise's value reached the guest"
    );
}

/// A component whose export hands the host's `measure` the string it
/// was given, through one memory and its allocator.
const PASSES_A_STRING: &[u8] = component!(
    r#"
    (component
      (import "measure" (func $measure (param "s" string) (result u32)))
      (core module $libc
        (memory (export "memory") 1)
        (global $next (mut i32) (i32.const 1024))
        (func (export "realloc") (param i32 i32 i32 i32) (result i32)
          (local $at i32)
          (local.set $at (global.get $next))
          (global.set $next (i32.add (global.get $next) (local.get 3)))
          (local.get $at)))
      (core instance $libc (instantiate $libc))
      (core func $measure (canon lower (func $measure) (memory (core memory $libc "memory"))))
      (core module $m
        (import "" "measure" (func $measure (param i32 i32) (result i32)))
        (func (export "run") (param i32 i32) (result i32)
          (call $measure (local.get 0) (local.get 1))))
      (core instance $i (instantiate $m
        (with "" (instance (export "measure" (func $measure))))))
      (func (export "run") (param "s" string) (result u32)
        (canon lift (core func $i "run")
          (memory (core memory $libc "memory"))
          (realloc (core func $libc "realloc")))))
    "#
);

#[wcmp_macros::test]
async fn it_runs_over_a_backend_without_multi_memory_under_the_policy() {
    // A browser without `multi_memory`, as Safari was, makes the
    // backend move bytes with `TypedArray.set` rather than a module of
    // its own. The page cannot take the feature away, so the test
    // refuses the backend's probe for it while the backend is made, and
    // passes a string both ways under the policy. A fused adapter
    // between two memories needs the feature and is refused without it,
    // so a copy between two components' memories is not covered here.
    install_the_policy();

    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;
    let namespace = js_sys::Reflect::get(&js_sys::global(), &"WebAssembly".into())
        .expect("the page has WebAssembly");
    let validate = js_sys::Reflect::get(&namespace, &"validate".into())
        .expect("validate")
        .unchecked_into::<js_sys::Function>();
    let probe = wcmp_macros::wasm!(r#"(module (memory 1) (memory 1))"#);
    let original = validate.clone();
    let refuse = Closure::<
        dyn Fn(wasm_bindgen::JsValue) -> Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue>,
    >::new(move |bytes: wasm_bindgen::JsValue| {
        if js_sys::Uint8Array::new(&bytes).to_vec() == probe {
            return Ok(wasm_bindgen::JsValue::FALSE);
        }
        let namespace = js_sys::Reflect::get(&js_sys::global(), &"WebAssembly".into())?;
        js_sys::Reflect::apply(&original, &namespace, &js_sys::Array::of1(&bytes))
    });
    js_sys::Reflect::set(&namespace, &"validate".into(), refuse.as_ref()).expect("patch validate");
    let backend = wcmp_wasm_core_web::Web::new();
    let capabilities =
        wcmp_wasm_core::Engine::with_backend(wcmp_wasm_core_web::Web::new()).capabilities();
    js_sys::Reflect::set(&namespace, &"validate".into(), &validate).expect("restore validate");
    assert!(
        !capabilities.contains(wcmp_wasm_core::Capability::MultiMemory),
        "the backend was made as in a browser without `multi_memory`"
    );

    let engine = Engine::with_backend(backend).expect("the engine");
    let component = Component::new(&engine, PASSES_A_STRING)
        .await
        .expect("the component compiles under the policy");
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap(
            "measure",
            |_call: wcmp::HostCall<'_, ()>, (text,): (String,)| {
                Ok(u32::try_from(text.len()).expect("a short string"))
            },
        )
        .expect("the registration of `measure`");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the component instantiates under the policy");
    let text = "measured under the policy";
    let result = instance
        .get_func("run")
        .expect("the guest's export")
        .call(&mut store, &[Val::String(text.into())])
        .await
        .expect("the string crosses both ways under the policy");
    assert_eq!(result.as_ref(), &[Val::U32(text.len() as u32)]);
}

#[path = "support/backend.rs"]
mod test_backend;
