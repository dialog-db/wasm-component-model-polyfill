// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One host core serves one guest under both macros.
//!
//! The host of `test:dual/dual` keeps its logic in a core with no
//! runtime type in it (`dual/host_core.rs`), and one glue module for
//! each runtime implements the generated traits by calling the core:
//! `dual/wasmtime_glue.rs` over `wasmtime::component::bindgen!`, which
//! runs natively, and `dual/polyfill_glue.rs` over `wcmp::bindgen!`,
//! which runs natively and in the browser. One guest (`dual/guest.rs`)
//! runs under each of the three, and each run records each value that
//! crosses and each error. The records are equal.
//!
//! The two glue modules differ where the design lists a difference from
//! Wasmtime:
//!
//! - The crate path of each name: `wasmtime::component::…` and
//!   `wasmtime::…` against `wcmp::…`, `wasmtime::Result` against
//!   `wcmp::Result`.
//! - The invocation of Wasmtime's macro sets `exports: { default: async }`,
//!   because every call into a component is an `async fn` in the
//!   polyfill.
//! - `FutureReader::new` takes the `Access` itself in Wasmtime, which is
//!   a store context through `AsContextMut`, and `Access::store` in the
//!   polyfill.
//!
//! They also differ in three ways the design does not list, filed as
//! card d5c6c1: a host's own error is wrapped in `anyhow::Error` and
//! passed on with `?` in the polyfill where Wasmtime takes
//! `wasmtime::Error::new` directly; the host's error is found by
//! `downcast_ref` on the error in Wasmtime and on its `source()` in the
//! polyfill; and the engine, the store, and the component are made
//! differently (a backend, a `Result` from `Store::new`, an awaited
//! `Component::new`).

#![cfg(test)]

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

#[path = "dual/guest.rs"]
mod guest;
#[path = "dual/host_core.rs"]
mod host_core;
#[path = "dual/polyfill_glue.rs"]
mod polyfill_glue;
#[cfg(not(target_arch = "wasm32"))]
#[path = "dual/wasmtime_glue.rs"]
mod wasmtime_glue;

/// What each run records: each value that crosses, each error, and what
/// the core was given, in order.
const RECORD: &[&str] = &[
    "shape-of -> Circle(7)",
    "call-describe -> (9, 2)",
    "call-counter -> 5",
    "accumulator total -> 6",
    "call-read 0 -> Ok(7)",
    "call-read 1 -> Err(not-found)",
    "call-answer -> 42",
    "call-lost -> 99",
    "call-checked 1 -> 2",
    "call-checked 0 -> error CheckFailed { value: 0 }",
    "core: describe Circle(3) read=true write=true -> (9, 2)",
    "core: counter 5",
    "core: dropped 5",
    "core: read 0",
    "core: read 1",
    "core: answer 21",
    "core: lost",
    "core: checked 1",
    "core: checked 0",
    "call-read 2 -> error Broken",
];

/// The lines of the polyfill glue module.
const POLYFILL_GLUE: &str = include_str!("dual/polyfill_glue.rs");

/// The lines of the Wasmtime glue module.
const WASMTIME_GLUE: &str = include_str!("dual/wasmtime_glue.rs");

#[wcmp_macros::test]
async fn it_runs_the_host_core_under_the_polyfill() {
    assert_eq!(
        polyfill_glue::run(guest::GUEST).await,
        RECORD,
        "the polyfill run records what the other runs record"
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn it_runs_the_same_host_core_under_wasmtime() {
    assert_eq!(
        wasmtime_glue::run(guest::GUEST).await,
        RECORD,
        "the Wasmtime run records what the polyfill runs record"
    );
}

#[wcmp_macros::test]
fn it_states_the_line_count_of_each_glue_module() {
    let polyfill = POLYFILL_GLUE.lines().count();
    let wasmtime = WASMTIME_GLUE.lines().count();
    println!("polyfill glue: {polyfill} lines; Wasmtime glue: {wasmtime} lines");
    assert_eq!(
        (polyfill, wasmtime),
        (218, 219),
        "the glue modules have the line counts this test states"
    );
}

#[path = "../../wcmp/tests/support/backend.rs"]
mod test_backend;
