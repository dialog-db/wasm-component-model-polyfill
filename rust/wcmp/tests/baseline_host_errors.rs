// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Baseline tests for a host's own errors: `Error::Host`.
//!
//! A host function that fails with an error of the host's own traps the
//! guest, the trap poisons the store, and the host's call returns the
//! same error, with the host's error as its source and the rest of the
//! host's chain beneath it. A polyfill error that a host function
//! passes on with `?`, directly or through an `anyhow::Error`, comes
//! back as itself.

#![cfg(test)]

use anyhow::Context as _;
use wcmp::{
    Component, Engine, Error, HostCall, Instance, Linker, ResourceTableError, Store, TaskCause,
    TypedFunc,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// A component that imports a synchronous `touch` and exports `run`,
/// which calls it with its argument and returns what it returned.
const CALLS_A_HOST_FUNCTION: &[u8] = component!(
    r#"
    (component
      (import "touch" (func $touch (param "x" u32) (result u32)))
      (core func $lowered (canon lower (func $touch)))
      (core module $m
        (import "" "touch" (func $touch (param i32) (result i32)))
        (func (export "run") (param i32) (result i32)
          (call $touch (local.get 0))))
      (core instance $i (instantiate $m
        (with "" (instance (export "touch" (func $lowered))))))
      (func (export "run") (param "x" u32) (result u32)
        (canon lift (core func $i "run"))))
    "#
);

/// An error of the host's own type.
#[derive(Debug, PartialEq)]
struct HostFailure {
    code: u32,
}

impl core::fmt::Display for HostFailure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "the host failed with code {}", self.code)
    }
}

impl std::error::Error for HostFailure {}

/// Instantiate [`CALLS_A_HOST_FUNCTION`] with a `touch` that runs
/// `body` on its argument.
async fn instantiate(
    body: impl Fn(u32) -> wcmp::Result<u32> + Send + Sync + 'static,
) -> (Store<()>, TypedFunc<(u32,), (u32,)>) {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, CALLS_A_HOST_FUNCTION)
        .await
        .expect("the component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap("touch", move |_call: HostCall<'_, ()>, (x,): (u32,)| {
            Ok((body(x)?,))
        })
        .expect("the registration");
    let mut store = Store::new(&engine, ()).expect("store");
    let instance: Instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the component instantiates");
    let run = instance
        .get_func("run")
        .expect("the component exports `run`")
        .typed::<(u32,), (u32,)>()
        .expect("the export's type");
    (store, run)
}

/// Each layer of `error`'s source chain, as its message, from `error`
/// itself down.
fn chain(error: &(dyn std::error::Error + 'static)) -> Vec<String> {
    let mut layers = vec![error.to_string()];
    let mut source = error.source();
    while let Some(layer) = source {
        layers.push(layer.to_string());
        source = layer.source();
    }
    layers
}

#[wcmp_macros::test]
async fn it_returns_the_hosts_own_error_with_a_source_that_downcasts() {
    let (mut store, run) =
        instantiate(|_x| Err(anyhow::Error::new(HostFailure { code: 7 }))?).await;

    let error = run
        .call(&mut store, (1,))
        .await
        .expect_err("the host function fails the guest's call");

    assert!(
        matches!(error, Error::Host(_)),
        "the call returns the host's error, got {error:?}"
    );
    let source = std::error::Error::source(&error).expect("the host's error is the source");
    assert_eq!(
        source.downcast_ref::<HostFailure>(),
        Some(&HostFailure { code: 7 }),
        "the source downcasts to the host's own type"
    );

    let after = run
        .call(&mut store, (1,))
        .await
        .expect_err("the trap poisoned the store");
    assert!(
        matches!(after, Error::Task(TaskCause::CannotEnter)),
        "a later call is refused, got {after:?}"
    );
}

#[wcmp_macros::test]
async fn it_returns_a_resource_table_error_a_host_function_passed_on() {
    // The host function passes on a polyfill error with `?`, directly
    // and through an `anyhow::Error`. Each comes back as itself.
    let (mut store, run) = instantiate(|_x| Err(ResourceTableError::NotPresent)?).await;
    let direct = run
        .call(&mut store, (1,))
        .await
        .expect_err("the host function fails the guest's call");
    assert!(
        matches!(direct, Error::ResourceTable(ResourceTableError::NotPresent)),
        "the error passed on directly comes back as itself, got {direct:?}"
    );

    let (mut store, run) = instantiate(|_x| {
        Err(anyhow::Error::new(Error::ResourceTable(
            ResourceTableError::NotPresent,
        )))?
    })
    .await;
    let through = run
        .call(&mut store, (1,))
        .await
        .expect_err("the host function fails the guest's call");
    assert!(
        matches!(
            through,
            Error::ResourceTable(ResourceTableError::NotPresent)
        ),
        "the error passed on through an `anyhow::Error` comes back as itself, \
         got {through:?}"
    );
}

#[wcmp_macros::test]
async fn it_keeps_every_layer_of_context_on_a_hosts_error() {
    let (mut store, run) = instantiate(|x| {
        let failed: Result<u32, HostFailure> = Err(HostFailure { code: x });
        let touched = failed
            .context("touching the value")
            .with_context(|| format!("serving `touch` for {x}"))?;
        Ok(touched)
    })
    .await;

    let error = run
        .call(&mut store, (3,))
        .await
        .expect_err("the host function fails the guest's call");

    assert!(
        matches!(error, Error::Host(_)),
        "the call returns the host's error, got {error:?}"
    );
    assert_eq!(
        chain(&error),
        [
            "host error: serving `touch` for 3",
            "serving `touch` for 3",
            "touching the value",
            "the host failed with code 3",
        ],
        "each layer the host added is visible from the call, down to the \
         host's own error"
    );
}

#[path = "support/backend.rs"]
mod test_backend;
