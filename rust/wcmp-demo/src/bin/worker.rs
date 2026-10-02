// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The service worker's binary. `sw.js` loads it and calls the two
//! functions it exports, one for each event the worker handles.

#[cfg(target_arch = "wasm32")]
mod web {
    use wasm_bindgen::prelude::*;

    /// Answer a `fetch` event's request. `sw.js` passes the promise to
    /// `respondWith`.
    #[wasm_bindgen]
    pub fn handle_fetch(request: web_sys::Request) -> js_sys::Promise {
        wasm_bindgen_futures::future_to_promise(wcmp_demo::serve_fetch(request))
    }

    /// Handle a `message` event's data, answering on `port`, the
    /// event's first port, when the message asks for an answer. `sw.js`
    /// passes the promise to `waitUntil`.
    #[wasm_bindgen]
    pub fn handle_message(data: JsValue, port: JsValue) -> js_sys::Promise {
        wasm_bindgen_futures::future_to_promise(async move {
            wcmp_demo::serve_message(data, port).await?;
            Ok(JsValue::UNDEFINED)
        })
    }
}

/// The worker's module starts here, as `sw.js` loads it: the worker
/// records its spans from its first event on.
fn main() {
    #[cfg(target_arch = "wasm32")]
    wcmp_demo::install_tracing("worker");
}
