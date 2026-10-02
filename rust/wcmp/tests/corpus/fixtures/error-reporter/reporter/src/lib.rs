// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A component that reads an error context it is handed, built by
//! `cargo` and wit-bindgen.
//!
//! wit-bindgen's `ErrorContext` reads the debug message with
//! `error-context.debug-message` and drops the handle with
//! `error-context.drop` when it goes out of scope.

wit_bindgen::generate!({
    path: "../wit",
    world: "reporter",
});

use wit_bindgen::rt::async_support::ErrorContext;

struct Reporter;

impl Guest for Reporter {
    fn describe(error: ErrorContext) -> String {
        error.debug_message()
    }
}

export!(Reporter);
