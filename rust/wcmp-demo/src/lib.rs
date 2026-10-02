// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

// The modules are private, and the crate exports only the entry points
// of its two binaries, which run only in the browser. Natively the
// binaries are empty, so nothing but the tests reaches the host
// framework, and the re-exports that name it go unused. The browser
// build keeps both lints.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code, unused_imports))]

//! A TodoMVC demo of the polyfill. Every distinctive part of its user
//! interface is a custom element whose behavior is a Wasm component,
//! and every API route is a Wasm component that the demo's service
//! worker serves. Each component starts as a string of Zena source,
//! which the browser compiles at run time with Zena's compiler, itself
//! a component.
//!
//! The crate has two contexts, each its own binary that Trunk builds:
//!
//! - The page, the main thread of the browser tab.
//! - The service worker, which intercepts the page's requests.
//!
//! The host framework's plain Rust, which compiles, links,
//! instantiates, and drives components through the polyfill, builds on
//! every target, so its tests run natively. What reaches the DOM,
//! `fetch`, or IndexedDB is browser only.
//!
//! The demo is not a framework: what this crate makes public is for its
//! own two binaries, and carries no promise.

mod compiler;
mod glue;
mod http_types;
mod model;
mod model_host;
mod platform;
mod routes;
mod sources;
mod view;
mod wasi;

#[cfg(target_arch = "wasm32")]
mod client;
#[cfg(target_arch = "wasm32")]
mod context;
#[cfg(target_arch = "wasm32")]
mod dom;
#[cfg(target_arch = "wasm32")]
mod drawer;
#[cfg(target_arch = "wasm32")]
mod elements;
#[cfg(target_arch = "wasm32")]
mod idb;
#[cfg(target_arch = "wasm32")]
mod page;
#[cfg(target_arch = "wasm32")]
mod telemetry;
#[cfg(target_arch = "wasm32")]
mod worker;

#[cfg(target_arch = "wasm32")]
pub use page::start as start_page;
#[cfg(target_arch = "wasm32")]
pub use telemetry::install as install_tracing;
#[cfg(target_arch = "wasm32")]
pub use worker::{fetch as serve_fetch, message as serve_message};

// The host framework's tests reach a browser in the web lane.
#[cfg(all(test, target_arch = "wasm32"))]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);
