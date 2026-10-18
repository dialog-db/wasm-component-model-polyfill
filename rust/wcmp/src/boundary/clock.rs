// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The clock of the boundary counters, in milliseconds from an origin
//! of its own: `performance.now()` in a browser or a worker, and a
//! monotonic clock natively.

/// The time now, in milliseconds.
#[cfg(all(feature = "boundary-counters", not(target_arch = "wasm32")))]
pub fn now_ms() -> f64 {
    use std::sync::OnceLock;
    use std::time::Instant;

    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    ORIGIN.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

/// The time now, in milliseconds.
///
/// `performance` is a global of a page and of a worker alike, so the
/// clock reads it off the global object, once per thread.
#[cfg(all(feature = "boundary-counters", target_arch = "wasm32"))]
pub fn now_ms() -> f64 {
    use js_sys::{Date, Function, Reflect};
    use wasm_bindgen::{JsCast, JsValue};

    thread_local! {
        static NOW: Option<(JsValue, Function)> = {
            let performance = Reflect::get(&js_sys::global(), &JsValue::from_str("performance"))
                .ok()
                .filter(|performance| performance.is_object());
            performance.and_then(|performance| {
                let now = Reflect::get(&performance, &JsValue::from_str("now")).ok()?;
                Some((performance, now.dyn_into::<Function>().ok()?))
            })
        };
    }
    NOW.with(|now| match now {
        Some((performance, now)) => now
            .call0(performance)
            .ok()
            .and_then(|time| time.as_f64())
            .unwrap_or_else(Date::now),
        None => Date::now(),
    })
}
