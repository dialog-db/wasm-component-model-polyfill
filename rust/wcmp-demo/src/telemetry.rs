// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The demo's tracing, in each context: the page and the service
//! worker.
//!
//! Each context installs one subscriber with two layers:
//!
//! - A performance layer, `tracing-web`'s, which turns every span into
//!   user timing marks and a measure. The browser's performance panel
//!   draws them on its timings track, beside the frames and the tasks.
//! - A console layer, which writes each event at `INFO` and above to
//!   the console, at its level.
//!
//! The performance layer records spans at `DEBUG` and above by default:
//! the polyfill's compiles, instantiations, and calls, and the demo's
//! compiles, renders, routes, and storage. The `trace` query parameter
//! of the page's URL names another level, such as `?trace=trace` for
//! the polyfill's turns and its canonical ABI too, or `?trace=off`. The
//! page hands its parameter to the service worker's script URL, so both
//! contexts record at the same level.
//!
//! The browser keeps every mark and measure in its performance timeline
//! until something clears it, and the performance layer never does. So
//! each context clears the timeline every few seconds. A recording in
//! the performance panel keeps what it saw as it saw it, and a span
//! that is open across a clear loses only its own measure.

use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::format::DefaultFields;
use tracing_subscriber::prelude::*;
use tracing_web::{MakeWebConsoleWriter, performance_layer};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

/// The query parameter that names the level the performance layer
/// records at.
pub const PARAMETER: &str = "trace";

/// How often each context clears its performance timeline.
const CLEAR_EVERY_MS: i32 = 5_000;

/// Install the subscriber of the context `context`, `page` or `worker`,
/// at the level the `trace` parameter of the context's own URL names. A
/// second install does nothing.
pub fn install(context: &str) {
    let level = level_of(&search());
    let console = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        // The standard clock does not run in the browser.
        .without_time()
        .with_writer(MakeWebConsoleWriter::new())
        .with_filter(LevelFilter::INFO);
    let performance = performance_layer()
        .with_details_from_fields(DefaultFields::new())
        .with_filter(level);
    if tracing_subscriber::registry()
        .with(console)
        .with(performance)
        .try_init()
        .is_ok()
    {
        clear_timeline_every(CLEAR_EVERY_MS);
        tracing::info!(context, %level, "tracing to the performance panel");
    }
}

/// The query of the context's own URL: the page's, or the service
/// worker's script's.
pub fn search() -> String {
    js_sys::Reflect::get(&js_sys::global(), &"location".into())
        .and_then(|location| js_sys::Reflect::get(&location, &"search".into()))
        .ok()
        .and_then(|search| search.as_string())
        .unwrap_or_default()
}

/// The level the `trace` parameter of `search` names, or `DEBUG`.
pub fn level_of(search: &str) -> LevelFilter {
    parameter(search)
        .and_then(|value| value.parse().ok())
        .unwrap_or(LevelFilter::DEBUG)
}

/// The value of the `trace` parameter of `search`, a URL's query with
/// or without its `?`.
pub fn parameter(search: &str) -> Option<&str> {
    search
        .trim_start_matches('?')
        .split('&')
        .find_map(|pair| pair.strip_prefix(PARAMETER)?.strip_prefix('='))
}

/// Clear the context's performance marks and measures every `millis`
/// milliseconds, on the context's own timer.
fn clear_timeline_every(millis: i32) {
    let global = js_sys::global();
    let Ok(performance) = js_sys::Reflect::get(&global, &"performance".into()) else {
        return;
    };
    let Ok(performance) = performance.dyn_into::<web_sys::Performance>() else {
        return;
    };
    let Ok(set_interval) = js_sys::Reflect::get(&global, &"setInterval".into())
        .and_then(|function| function.dyn_into::<js_sys::Function>())
    else {
        return;
    };
    let clear = Closure::<dyn Fn()>::new(move || {
        performance.clear_marks();
        performance.clear_measures();
    });
    let _ = set_interval.call2(&global, clear.as_ref(), &JsValue::from(millis));
    // The timer lasts as long as the context.
    clear.forget();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    fn it_reads_the_level_from_the_trace_parameter() {
        assert_eq!(level_of("?trace=trace"), LevelFilter::TRACE);
        assert_eq!(level_of("?other=1&trace=off"), LevelFilter::OFF);
        assert_eq!(parameter("trace=info&other=1"), Some("info"));
    }

    #[wcmp_macros::test]
    fn it_records_at_debug_without_a_trace_parameter_or_with_an_unknown_level() {
        assert_eq!(level_of(""), LevelFilter::DEBUG);
        assert_eq!(level_of("?tracer=trace"), LevelFilter::DEBUG);
        assert_eq!(level_of("?trace=loud"), LevelFilter::DEBUG);
    }
}
