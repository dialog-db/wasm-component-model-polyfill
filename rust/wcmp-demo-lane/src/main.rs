// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The demo lane: the served Zena TodoMVC demo in headless Chrome,
//! driven over WebDriver.
//!
//! `wcmp-demo-lane [--jobs <n>] [--report <file>] [<filter>...]` serves
//! the built demo with `static-web-server`, starts ChromeDriver, and
//! runs each test whose name contains a filter, or every test. Each test
//! gets a browser session with a profile of its own, so it starts with
//! no service worker and an empty IndexedDB. The environment names the
//! tools: `WCMP_DEMO_SITE` the built demo, `CHROME` the browser,
//! `CHROMEDRIVER`, and `STATIC_WEB_SERVER`.
//!
//! The lane prints one line per test and a summary, writes the same to
//! the report file when one is named, and exits with 1 when a test
//! failed.

#[cfg(not(target_arch = "wasm32"))]
mod browser;
#[cfg(not(target_arch = "wasm32"))]
mod lane;
#[cfg(not(target_arch = "wasm32"))]
mod tests;
#[cfg(not(target_arch = "wasm32"))]
mod webdriver;

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    std::process::exit(lane::run(std::env::args().skip(1).collect()));
}

/// The lane drives a browser from the host, so there is nothing to run
/// on the web target.
#[cfg(target_arch = "wasm32")]
fn main() {}
