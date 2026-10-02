// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! How long a story took, on either target.
//!
//! `std::time::Instant` is unavailable on `wasm32-unknown-unknown`,
//! so the browser reads the JavaScript clock instead.

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use std::time::Instant;

    /// A stopwatch started when a story starts.
    pub struct Clock(Instant);

    impl Clock {
        pub fn start() -> Self {
            Self(Instant::now())
        }

        /// Milliseconds since the clock started.
        pub fn elapsed_millis(&self) -> f64 {
            self.0.elapsed().as_secs_f64() * 1000.0
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    /// A stopwatch started when a story starts.
    pub struct Clock(f64);

    impl Clock {
        pub fn start() -> Self {
            Self(js_sys::Date::now())
        }

        /// Milliseconds since the clock started.
        pub fn elapsed_millis(&self) -> f64 {
            js_sys::Date::now() - self.0
        }
    }
}

pub use imp::Clock;
