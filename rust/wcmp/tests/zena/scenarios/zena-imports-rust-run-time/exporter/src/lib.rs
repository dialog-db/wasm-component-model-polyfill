// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Scenario 10, the exporter: a Rust partner, built by `cargo` and
//! wit-bindgen's async support, that implements the `greeter` interface
//! `importer.zena` imports. The runner links the two at run time.

wit_bindgen::generate!({
    path: "../wit",
    world: "exporter",
});

use exports::wcmp::rust_link::greeter::Guest;

struct Exporter;

impl Guest for Exporter {
    /// A greeting that names Rust, so the importer's answer shows which
    /// component made it.
    async fn greet(name: String) -> String {
        format!("hello from Rust, {name}")
    }
}

export!(Exporter);
