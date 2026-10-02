// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What each context, the page or the service worker, starts with: an
//! engine over the browser backend, and Zena's compiler on it.
//!
//! A context fetches the compiler component and the source bundle once
//! when it starts. The compiler is larger than the 8 MB that Chromium
//! compiles synchronously on a main thread, so the browser backend's
//! asynchronous compile applies. The page compiles on its main thread,
//! and a compile blocks the page for its duration.

use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

#[allow(clippy::module_inception)]
mod context;
mod instantiated;

pub use context::Context;
pub use instantiated::Instantiated;

/// The bytes at `url`, fetched with the global `fetch`, which the page
/// and the service worker both have.
///
/// # Errors
///
/// The exception of the fetch, or a message when the response is not
/// `ok`.
pub async fn fetch_bytes(url: &str) -> Result<Vec<u8>, JsValue> {
    let global: JsValue = js_sys::global().into();
    let fetch = js_sys::Reflect::get(&global, &"fetch".into())?.dyn_into::<js_sys::Function>()?;
    let response = JsFuture::from(js_sys::Promise::from(fetch.call1(&global, &url.into())?))
        .await?
        .dyn_into::<web_sys::Response>()?;
    if !response.ok() {
        return Err(format!("fetching {url} answered {}", response.status()).into());
    }
    let buffer = JsFuture::from(response.array_buffer()?).await?;
    Ok(js_sys::Uint8Array::new(&buffer).to_vec())
}

/// An error as a JavaScript string.
pub fn text(error: impl core::fmt::Display) -> JsValue {
    JsValue::from_str(&error.to_string())
}
