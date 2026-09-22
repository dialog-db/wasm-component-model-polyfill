//! The per-target clock the suite measures against.

use crate::error::Result;

/// A monotonic clock, in milliseconds since the clock was made.
///
/// The two targets read a different clock — the monotonic system clock
/// natively, `performance.now()` in a browser — and that is the whole
/// of the per-target difference in a measurement. Both report
/// milliseconds as an `f64`, the unit and width `performance.now()`
/// fixes.
///
/// A browser clamps `performance.now()`: Chromium rounds it to 100
/// microseconds unless the page is cross-origin isolated. A single
/// iteration of a fast benchmark is well under that, which is why
/// [`crate::Run`] times a batch of iterations rather than one.
#[cfg(not(target_arch = "wasm32"))]
pub struct Clock {
    start: std::time::Instant,
}

#[cfg(not(target_arch = "wasm32"))]
impl Clock {
    /// Start a clock. Never fails on this target.
    pub fn new() -> Result<Self> {
        Ok(Self {
            start: std::time::Instant::now(),
        })
    }

    /// Milliseconds since the clock was made.
    pub fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64() * 1000.0
    }
}

/// A monotonic clock, in milliseconds since the clock was made.
///
/// See the native half of this type for what the two clocks share.
#[cfg(target_arch = "wasm32")]
pub struct Clock {
    performance: wasm_bindgen::JsValue,
    now: js_sys::Function,
    start: f64,
}

#[cfg(target_arch = "wasm32")]
impl Clock {
    /// Start a clock against the global's `performance.now()`.
    ///
    /// Fails when the global has no `performance` with a callable
    /// `now`, which is the honest answer on a host that cannot be
    /// measured rather than a fabricated zero.
    pub fn new() -> Result<Self> {
        use wasm_bindgen::{JsCast, JsValue};

        let global = js_sys::global();
        let performance = js_sys::Reflect::get(&global, &JsValue::from_str("performance"))
            .map_err(|_| crate::Error::Setup("the global has no `performance`".to_owned()))?;
        let now = js_sys::Reflect::get(&performance, &JsValue::from_str("now"))
            .map_err(|_| crate::Error::Setup("`performance` has no `now`".to_owned()))?
            .dyn_into::<js_sys::Function>()
            .map_err(|_| crate::Error::Setup("`performance.now` is not callable".to_owned()))?;
        let mut clock = Self {
            performance,
            now,
            start: 0.0,
        };
        clock.start = clock.read();
        Ok(clock)
    }

    /// Milliseconds since the clock was made.
    pub fn now(&self) -> f64 {
        self.read() - self.start
    }

    /// One `performance.now()` reading. A call that fails or answers
    /// with something other than a number reads as `NaN`, which the
    /// report carries through as `null` rather than as a plausible
    /// number.
    fn read(&self) -> f64 {
        self.now
            .call0(&self.performance)
            .ok()
            .and_then(|value| value.as_f64())
            .unwrap_or(f64::NAN)
    }
}
