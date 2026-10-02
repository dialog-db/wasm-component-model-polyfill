// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Baseline tests for the may-leave flag: what a guest may not do
//! while the polyfill is inside a call of its own into that guest.
//!
//! The polyfill makes two calls into a guest that the guest never
//! asked for: the `cabi_realloc` a crossing asks for memory with,
//! and the `post-return` an export runs once the caller has observed
//! the return value. The reference clears the instance's may-leave
//! flag around both, and every operation that would leave the
//! instance traps with the cannot-leave cause while it is clear: a
//! lowered import, `resource.new`, `resource.drop`, and a call into
//! another component's export.
//!
//! The flag is one core global per component instance. The fused
//! adapters read and write it themselves, and the built-ins read the
//! same global, so a guest-to-guest call made from a post-return the
//! polyfill ran meets the adapter's own check with the flag the
//! polyfill cleared.
//!
//! A destructor is the call the reference does *not* clear the flag
//! around; that a destructor may call the host is proved beside the
//! destructor's own task, in `baseline_destructor_task`.

#![cfg(test)]

use wcmp::{Component, Engine, Error, HostCall, Instance, Linker, Result, Store, Val};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// Which operation the guest performs from inside the call the
/// polyfill made, chosen by the `select` export before the call.
/// Zero, the value the guest starts with, performs none of them.
const NOTHING: u32 = 0;
/// Call out to a host import.
const HOST_IMPORT: u32 = 1;
/// Mint a handle with `resource.new`.
const RESOURCE_NEW: u32 = 2;
/// Give a handle back with `resource.drop`.
const RESOURCE_DROP: u32 = 3;

/// A component whose `cabi_realloc` performs the operation `select`
/// chose: a call out to a host import, a `resource.new`, or a
/// `resource.drop` of the handle `make` minted earlier.
///
/// The `run` export takes a `string`, so the host's lowering of that
/// argument calls the `realloc` before the export itself runs.
const REALLOC_CALLS_OUT: &[u8] = component!(
    r#"
    (component
      (import "probe" (func $probe (param "x" u32) (result u32)))
      (core func $probe' (canon lower (func $probe)))
      (type $r (resource (rep i32)))
      (core func $new (canon resource.new $r))
      (core func $drop (canon resource.drop $r))
      (core module $m
        (import "" "probe" (func $probe (param i32) (result i32)))
        (import "" "new" (func $new (param i32) (result i32)))
        (import "" "drop" (func $drop (param i32)))
        (memory (export "memory") 1)
        (global $which (mut i32) (i32.const 0))
        (global $handle (mut i32) (i32.const 0))
        (func (export "select") (param i32) (global.set $which (local.get 0)))
        (func (export "make") (result i32)
          (global.set $handle (call $new (i32.const 7)))
          (global.get $handle))
        (func (export "realloc") (param i32 i32 i32 i32) (result i32)
          (if (i32.eq (global.get $which) (i32.const 1))
            (then (drop (call $probe (i32.const 1)))))
          (if (i32.eq (global.get $which) (i32.const 2))
            (then (drop (call $new (i32.const 9)))))
          (if (i32.eq (global.get $which) (i32.const 3))
            (then (call $drop (global.get $handle))))
          (i32.const 16))
        (func (export "run") (param i32 i32)))
      (core instance $m (instantiate $m (with "" (instance
        (export "probe" (func $probe'))
        (export "new" (func $new))
        (export "drop" (func $drop))))))
      (func (export "select") (param "w" u32) (canon lift (core func $m "select")))
      (func (export "make") (result u32) (canon lift (core func $m "make")))
      (func (export "run") (param "x" string)
        (canon lift (core func $m "run")
          (realloc (core func $m "realloc"))
          (memory (core memory $m "memory")))))
    "#
);

/// The same three operations, performed from the `post-return` of
/// the `run` export instead. `select` and `make` are lifted with no
/// `post-return` of their own, so only `run` reaches the switch.
const POST_RETURN_CALLS_OUT: &[u8] = component!(
    r#"
    (component
      (import "probe" (func $probe (param "x" u32) (result u32)))
      (core func $probe' (canon lower (func $probe)))
      (type $r (resource (rep i32)))
      (core func $new (canon resource.new $r))
      (core func $drop (canon resource.drop $r))
      (core module $m
        (import "" "probe" (func $probe (param i32) (result i32)))
        (import "" "new" (func $new (param i32) (result i32)))
        (import "" "drop" (func $drop (param i32)))
        (global $which (mut i32) (i32.const 0))
        (global $handle (mut i32) (i32.const 0))
        (func (export "select") (param i32) (global.set $which (local.get 0)))
        (func (export "make") (result i32)
          (global.set $handle (call $new (i32.const 7)))
          (global.get $handle))
        (func (export "call-probe") (result i32) (call $probe (i32.const 3)))
        (func (export "release") (call $drop (global.get $handle)))
        (func (export "run") (result i32) (i32.const 5))
        (func (export "post-return") (param i32)
          (if (i32.eq (global.get $which) (i32.const 1))
            (then (drop (call $probe (i32.const 1)))))
          (if (i32.eq (global.get $which) (i32.const 2))
            (then (drop (call $new (i32.const 9)))))
          (if (i32.eq (global.get $which) (i32.const 3))
            (then (call $drop (global.get $handle))))))
      (core instance $m (instantiate $m (with "" (instance
        (export "probe" (func $probe'))
        (export "new" (func $new))
        (export "drop" (func $drop))))))
      (func (export "select") (param "w" u32) (canon lift (core func $m "select")))
      (func (export "make") (result u32) (canon lift (core func $m "make")))
      (func (export "call-probe") (result u32) (canon lift (core func $m "call-probe")))
      (func (export "release") (canon lift (core func $m "release")))
      (func (export "run") (result u32)
        (canon lift (core func $m "run")
          (post-return (core func $m "post-return")))))
    "#
);

/// Two inner components, where the `post-return` of `$B`'s export
/// calls the export of `$A`. The call goes through the adapter the
/// translator emits, which reads the same flag the polyfill cleared
/// for the post-return and traps on its own.
const POST_RETURN_CALLS_ANOTHER_COMPONENT: &[u8] = component!(
    r#"
    (component
      (component $A
        (core module $m
          (func (export "f") (result i32) (i32.const 7)))
        (core instance $i (instantiate $m))
        (func (export "f") (result u32) (canon lift (core func $i "f"))))
      (component $B
        (import "f" (func $f (result u32)))
        (core func $f' (canon lower (func $f)))
        (core module $m
          (import "" "f" (func $f (result i32)))
          (func (export "run") (result i32) (i32.const 5))
          (func (export "post-return") (param i32) (drop (call $f))))
        (core instance $i (instantiate $m (with "" (instance
          (export "f" (func $f'))))))
        (func (export "run") (result u32)
          (canon lift (core func $i "run")
            (post-return (core func $i "post-return")))))
      (instance $a (instantiate $A))
      (instance $b (instantiate $B (with "f" (func $a "f"))))
      (export "run" (func $b "run")))
    "#
);

/// Instantiate `bytes` with a host `probe` function registered, so a
/// guest that calls out to the host has something to reach.
async fn instantiate(bytes: &[u8]) -> (Store<()>, Instance) {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, bytes)
        .await
        .expect("component parses");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap(
            "probe",
            |_: HostCall<'_, ()>, (x,): (u32,)| -> Result<u32> { Ok(x) },
        )
        .expect("the registration");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    (store, instance)
}

/// Every message in an error's source chain, joined so that a trap
/// is matched wherever the substrate put it.
fn chain(error: &Error) -> String {
    let mut out = String::new();
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(link) = current {
        if !out.is_empty() {
            out.push_str(": ");
        }
        out.push_str(&link.to_string());
        current = link.source();
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Call `name` with `args` and report the message of the trap it
/// raised.
async fn call_trap(store: &mut Store<()>, instance: &Instance, name: &str, args: &[Val]) -> String {
    let func = instance.get_func(name).expect("the export is declared");
    match func.call(store, args).await {
        Err(error) => chain(&error),
        Ok(values) => panic!("{name} returned {values:?} rather than trapping"),
    }
}

/// Call `name` with `args` and expect it to return.
async fn call(store: &mut Store<()>, instance: &Instance, name: &str, args: &[Val]) {
    let func = instance.get_func(name).expect("the export is declared");
    func.call(store, args)
        .await
        .unwrap_or_else(|error| panic!("{name} returned: {}", chain(&error)));
}

/// Prepare an instance of `bytes` that will perform `which` from
/// inside the call the polyfill makes: the switch is set, and a
/// handle is minted for the `resource.drop` case while the instance
/// may still be left.
async fn prepared(bytes: &[u8], which: u32) -> (Store<()>, Instance) {
    let (mut store, instance) = instantiate(bytes).await;
    call(&mut store, &instance, "select", &[Val::U32(which)]).await;
    call(&mut store, &instance, "make", &[]).await;
    (store, instance)
}

#[wcmp_macros::test]
async fn it_refuses_a_host_import_a_realloc_calls() {
    let (mut store, instance) = prepared(REALLOC_CALLS_OUT, HOST_IMPORT).await;

    // Lowering the string argument calls the `realloc`, and the
    // instance may not be left while it runs.
    let message = call_trap(
        &mut store,
        &instance,
        "run",
        &[Val::String("hi".to_owned())],
    )
    .await;

    assert!(
        message.contains("cannot leave component instance"),
        "a lowered import called from a realloc must fail with the \
         cannot-leave cause: {message}"
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_host_import_a_post_return_calls() {
    let (mut store, instance) = prepared(POST_RETURN_CALLS_OUT, HOST_IMPORT).await;

    let message = call_trap(&mut store, &instance, "run", &[]).await;

    assert!(
        message.contains("cannot leave component instance"),
        "a lowered import called from a post-return must fail with the \
         cannot-leave cause: {message}"
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_resource_new_a_realloc_calls() {
    let (mut store, instance) = prepared(REALLOC_CALLS_OUT, RESOURCE_NEW).await;

    let message = call_trap(
        &mut store,
        &instance,
        "run",
        &[Val::String("hi".to_owned())],
    )
    .await;

    assert!(
        message.contains("cannot leave component instance"),
        "`resource.new` called from a realloc must fail with the \
         cannot-leave cause: {message}"
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_resource_drop_a_realloc_calls() {
    let (mut store, instance) = prepared(REALLOC_CALLS_OUT, RESOURCE_DROP).await;

    let message = call_trap(
        &mut store,
        &instance,
        "run",
        &[Val::String("hi".to_owned())],
    )
    .await;

    assert!(
        message.contains("cannot leave component instance"),
        "`resource.drop` called from a realloc must fail with the \
         cannot-leave cause: {message}"
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_resource_new_a_post_return_calls() {
    let (mut store, instance) = prepared(POST_RETURN_CALLS_OUT, RESOURCE_NEW).await;

    let message = call_trap(&mut store, &instance, "run", &[]).await;

    assert!(
        message.contains("cannot leave component instance"),
        "`resource.new` called from a post-return must fail with the \
         cannot-leave cause: {message}"
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_resource_drop_a_post_return_calls() {
    let (mut store, instance) = prepared(POST_RETURN_CALLS_OUT, RESOURCE_DROP).await;

    let message = call_trap(&mut store, &instance, "run", &[]).await;

    assert!(
        message.contains("cannot leave component instance"),
        "`resource.drop` called from a post-return must fail with the \
         cannot-leave cause: {message}"
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_call_into_another_component_a_post_return_makes() {
    let (mut store, instance) = instantiate(POST_RETURN_CALLS_ANOTHER_COMPONENT).await;

    let message = call_trap(&mut store, &instance, "run", &[]).await;

    assert!(
        message.contains("cannot leave component instance"),
        "a guest-to-guest call made from a post-return must fail with the \
         cannot-leave cause, because the adapter reads the flag the \
         post-return cleared: {message}"
    );
}

#[wcmp_macros::test]
async fn it_lets_the_guest_call_out_while_no_call_of_the_polyfills_runs() {
    // The same three operations, performed from an export of the
    // guest's own rather than from the post-return: each one runs,
    // so what the tests above refuse is the call the polyfill made
    // and not the operation itself.
    let (mut store, instance) = instantiate(POST_RETURN_CALLS_OUT).await;

    call(&mut store, &instance, "select", &[Val::U32(NOTHING)]).await;
    call(&mut store, &instance, "make", &[]).await;
    call(&mut store, &instance, "call-probe", &[]).await;
    call(&mut store, &instance, "release", &[]).await;
    call(&mut store, &instance, "run", &[]).await;
}

#[path = "support/backend.rs"]
mod test_backend;
