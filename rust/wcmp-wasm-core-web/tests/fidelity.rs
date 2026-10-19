// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The fidelity suite on the browser backend: the specification test
//! suite at the pinned revision, for the floor and for each capability the
//! browser declares, run through the runtime layer.

#![cfg(target_arch = "wasm32")]

use wcmp_wasm_core::Engine;
use wcmp_wasm_core_web::Web;

#[path = "support/page_engine.rs"]
mod page_engine;

use page_engine::PageEngine;

wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

fn engine() -> Engine {
    Engine::with_backend(Web::new())
}

/// The directives V8, the engine of Chromium, fails, each with what
/// explains it: a defect of the engine, or a limit that the web embedding
/// requires.
const EXPECTED_FAILURES_V8: &str = include_str!("fidelity/expected-failures.txt");

/// The directives JavaScriptCore, the engine of Safari and WebKitGTK,
/// fails, each with what explains it.
const EXPECTED_FAILURES_JAVASCRIPTCORE: &str =
    include_str!("fidelity/expected-failures.javascriptcore.txt");

/// The list of the engine of the page: the web lanes run in Chrome, and
/// the WebKitGTK lanes in MiniBrowser.
fn expected_failures() -> &'static str {
    match PageEngine::of_page() {
        PageEngine::V8 => EXPECTED_FAILURES_V8,
        PageEngine::JavaScriptCore => EXPECTED_FAILURES_JAVASCRIPTCORE,
        PageEngine::SpiderMonkey => panic!("no lane runs Firefox, so it has no list"),
    }
}

wcmp_wasm_core_fidelity::fidelity_tests!(engine, expected_failures());

/// The list of each engine cites what explains every entry, in every lane,
/// so that a lane in one browser checks the other browser's list too.
#[wcmp_macros::test]
fn it_cites_what_explains_each_entry_of_every_engines_list() {
    wcmp_wasm_core_fidelity::check_expected_failures(EXPECTED_FAILURES_V8);
    wcmp_wasm_core_fidelity::check_expected_failures(EXPECTED_FAILURES_JAVASCRIPTCORE);
}
