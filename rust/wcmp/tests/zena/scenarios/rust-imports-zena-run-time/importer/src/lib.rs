// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Scenario 11, the importer: a Rust partner, built by `cargo` and
//! wit-bindgen's async support, whose `welcome` passes its string to
//! the Zena exporter's `greet` across the link and passes the answer
//! back. The runner links the two at run time.

wit_bindgen::generate!({
    path: "../wit",
    world: "importer",
});

use wcmp::rust_link::greeter::greet;

struct Importer;

impl Guest for Importer {
    async fn welcome(name: String) -> String {
        let greeting = greet(name).await;
        format!("{greeting}!")
    }
}

export!(Importer);
