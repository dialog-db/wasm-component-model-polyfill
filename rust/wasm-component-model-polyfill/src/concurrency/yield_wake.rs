//! Returning control to the host executor after a yield.
//!
//! A yield always gives way: the item that yielded resumes after
//! every other ready item, and its resumption first returns control
//! to the host executor. How that return happens is the one line of
//! the scheduler that differs per target.
//!
//! Natively the driver wakes itself and returns pending, which lets
//! the executor run its own timers and sockets before it polls the
//! driver again.
//!
//! In the browser the driver is polled from a microtask. A self-wake
//! lands back in the microtask queue, ahead of every network
//! response and timer, so a guest that spins on a yield would starve
//! the page. The browser's yield therefore crosses a macrotask
//! boundary — a `setTimeout` of zero — before it wakes the driver.

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use core::task::Waker;

    /// The wake a driver arranges when a turn ends in a yield.
    ///
    /// Natively the wake is immediate: the driver wakes itself and
    /// returns pending, so the executor polls it again after it has
    /// run whatever else is ready. There is nothing to wait for
    /// afterwards, so the wake has always landed.
    pub struct YieldWake {
        _private: (),
    }

    impl YieldWake {
        /// Arrange the wake that returns control to the host
        /// executor after a yield.
        pub fn after_yield(waker: &Waker) -> Self {
            waker.wake_by_ref();
            Self { _private: () }
        }

        /// Whether the wake has landed and the driver may run the
        /// item that yielded.
        pub fn landed(&self) -> bool {
            true
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use core::cell::Cell;
    use core::task::Waker;
    use std::rc::Rc;

    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::{JsCast, JsValue};

    /// The wake a driver arranges when a turn ends in a yield.
    ///
    /// In the browser the wake crosses a macrotask boundary: the
    /// driver asks for a `setTimeout` of zero and returns pending,
    /// so every network response and timer the page already has
    /// queued runs before the guest's next item. The flag says
    /// whether that timeout has fired.
    pub struct YieldWake {
        landed: Rc<Cell<bool>>,
    }

    impl YieldWake {
        /// Arrange the wake that returns control to the host
        /// executor after a yield.
        pub fn after_yield(waker: &Waker) -> Self {
            let landed = Rc::new(Cell::new(false));
            if !schedule_macrotask(landed.clone(), waker.clone()) {
                // No `setTimeout` on this global: fall back to the
                // native behaviour rather than stall the driver.
                landed.set(true);
                waker.wake_by_ref();
            }
            Self { landed }
        }

        /// Whether the wake has landed and the driver may run the
        /// item that yielded.
        pub fn landed(&self) -> bool {
            self.landed.get()
        }
    }

    /// Ask the global for a `setTimeout` of zero that sets `landed`
    /// and wakes `waker`. `false` when this global has no
    /// `setTimeout` to schedule it with.
    fn schedule_macrotask(landed: Rc<Cell<bool>>, waker: Waker) -> bool {
        let global = js_sys::global();
        let Ok(entry) = js_sys::Reflect::get(&global, &JsValue::from_str("setTimeout")) else {
            return false;
        };
        let Some(set_timeout) = entry.dyn_ref::<js_sys::Function>().cloned() else {
            return false;
        };
        let callback = Closure::once_into_js(move || {
            landed.set(true);
            waker.wake();
        });
        set_timeout
            .call2(&global, &callback, &JsValue::from_f64(0.0))
            .is_ok()
    }
}

pub use imp::YieldWake;
