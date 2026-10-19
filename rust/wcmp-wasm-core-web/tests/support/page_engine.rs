// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The JavaScript engine of the page a test runs in.
//!
//! The backend does not know which engine it runs on, and does not need
//! to. A test that holds the backend to what one engine does, such as
//! the capabilities it declares or the words of its traps, asks here.
//! The web lanes run in headless Chrome, whose engine is V8, and the
//! WebKitGTK lanes in MiniBrowser, whose engine is JavaScriptCore.

#![allow(dead_code)]

use js_sys::Reflect;

/// The engine of a page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageEngine {
    /// The engine of Chromium.
    V8,
    /// The engine of Safari and of WebKitGTK.
    JavaScriptCore,
    /// The engine of Firefox.
    SpiderMonkey,
}

impl PageEngine {
    /// The engine of this page, read from the user agent: Chromium's
    /// names `Chrome/`, Firefox's names `Firefox/`, and WebKit's names
    /// `AppleWebKit/` alone. A user agent that names none panics, so a
    /// test never holds the backend to the wrong engine.
    ///
    /// The user agent is read rather than a trap's words, because a test
    /// of the page's Content Security Policy can leave the page unable
    /// to compile a module.
    pub fn of_page() -> Self {
        let agent = Reflect::get(&js_sys::global(), &"navigator".into())
            .and_then(|navigator| Reflect::get(&navigator, &"userAgent".into()))
            .ok()
            .and_then(|agent| agent.as_string())
            .expect("the page has a user agent");
        if agent.contains("Chrome/") {
            PageEngine::V8
        } else if agent.contains("Firefox/") {
            PageEngine::SpiderMonkey
        } else if agent.contains("AppleWebKit/") {
            PageEngine::JavaScriptCore
        } else {
            panic!("the user agent names no engine the tests know: {agent}")
        }
    }
}
