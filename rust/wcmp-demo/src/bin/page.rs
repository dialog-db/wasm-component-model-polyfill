// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The page's binary. `main` runs when Trunk's loader instantiates the
//! module, and schedules the page's start on the browser's event loop.

#[cfg(target_arch = "wasm32")]
fn main() {
    wcmp_demo::install_tracing("page");
    wasm_bindgen_futures::spawn_local(async {
        if let Err(error) = wcmp_demo::start_page().await {
            web_sys::console::error_2(&"the demo page failed to start:".into(), &error);
        }
    });
}

/// The page runs only in the browser.
#[cfg(not(target_arch = "wasm32"))]
fn main() {}
