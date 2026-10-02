// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What the drawer shows of a route.

/// What the drawer shows of a route.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RouteStatus {
    /// The pattern.
    pub pattern: String,
    /// The route's source: the edit when it has one, which a status
    /// keeps, with its diagnostics, even when the route fell back to the
    /// shipped source.
    pub source: String,
    /// The source the demo ships.
    pub shipped: String,
    /// The time of the last compile of the Zena source to a component,
    /// in milliseconds.
    pub compile_ms: Option<f64>,
    /// The time the polyfill took to compile that component, in
    /// milliseconds: its translation and the compiles of its core
    /// modules.
    pub wasm_compile_ms: Option<f64>,
    /// The time of the last link and instantiation, in milliseconds.
    pub instantiate_ms: Option<f64>,
    /// The diagnostics of the last compile, when it failed.
    pub diagnostics: Option<String>,
    /// The trap of the last request, when it trapped.
    pub trapped: Option<String>,
    /// How many times the router compiled the route.
    pub compiles: u32,
    /// When the router last compiled the route, in milliseconds since the
    /// Unix epoch.
    pub compiled_at: Option<f64>,
}
