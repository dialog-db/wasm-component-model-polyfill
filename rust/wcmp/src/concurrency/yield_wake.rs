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
//! boundary before it wakes the driver.
//!
//! The macrotask is a message posted to a `MessageChannel` port of
//! the wake's own. A `setTimeout` of zero is the other way to reach
//! the next macrotask, and it carries two floors a yield cannot
//! afford. The HTML timer initialisation steps clamp a timeout
//! nested more than five deep to four milliseconds, and a background
//! tab throttles timers to about one a second; a callback export
//! that yields once per event would run at a few hundred events a
//! second in the foreground and about one a second in the
//! background. A port message is a macrotask under neither clamp, so
//! the rule the yield is there for — the responses and timers the
//! page already has queued run before the guest's next item — holds
//! at the speed of the event loop. The timeout stays as the fallback
//! for a global that has no `MessageChannel`.

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

        /// Wake `waker` rather than the waker the wake was arranged
        /// with, when it lands. Natively it has landed already.
        pub fn rewake(&self, _waker: &Waker) {}
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use core::cell::{Cell, RefCell};
    use core::task::Waker;
    use std::rc::Rc;

    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::{JsCast, JsValue};

    /// The wake a driver arranges when a turn ends in a yield.
    ///
    /// In the browser the wake crosses a macrotask boundary: the
    /// driver posts a message to a port of its own and returns
    /// pending, so every network response and timer the page already
    /// has queued runs before the guest's next item. The flag says
    /// whether that message has arrived.
    pub struct YieldWake {
        landed: Rc<Cell<bool>>,
        waker: Rc<RefCell<Waker>>,
    }

    impl YieldWake {
        /// Arrange the wake that returns control to the host
        /// executor after a yield.
        pub fn after_yield(waker: &Waker) -> Self {
            let landed = Rc::new(Cell::new(false));
            let wakes = Rc::new(RefCell::new(waker.clone()));
            if !post_message(&landed, &wakes) && !schedule_timeout(&landed, &wakes) {
                // Neither mechanism on this global: fall back to the
                // native behaviour rather than stall the driver.
                landed.set(true);
                waker.wake_by_ref();
            }
            Self {
                landed,
                waker: wakes,
            }
        }

        /// Whether the wake has landed and the driver may run the
        /// item that yielded.
        pub fn landed(&self) -> bool {
            self.landed.get()
        }

        /// Wake `waker` rather than the waker the wake was arranged
        /// with, when it lands: the driver was polled again, with the
        /// waker of its latest poll, before the wake landed.
        pub fn rewake(&self, waker: &Waker) {
            if !self.waker.borrow().will_wake(waker) {
                *self.waker.borrow_mut() = waker.clone();
            }
        }
    }

    /// Post a message to one end of a fresh `MessageChannel`, whose
    /// other end sets `landed` and wakes `waker` when the message
    /// arrives. `false` when this global offers no `MessageChannel`,
    /// or when building one from it did not work.
    ///
    /// The channel belongs to this one wake. Its receiving end is
    /// reachable from the handler, so the pair lives until the
    /// message is delivered, and the handler closes it before it
    /// wakes anything.
    fn post_message(landed: &Rc<Cell<bool>>, waker: &Rc<RefCell<Waker>>) -> bool {
        let global = js_sys::global();
        let Some(constructor) = member(&global, "MessageChannel") else {
            return false;
        };
        let Ok(channel) = js_sys::Reflect::construct(&constructor, &js_sys::Array::new()) else {
            return false;
        };
        let (Some(receiver), Some(sender)) = (
            js_sys::Reflect::get(&channel, &JsValue::from_str("port1")).ok(),
            js_sys::Reflect::get(&channel, &JsValue::from_str("port2")).ok(),
        ) else {
            return false;
        };
        let Some(post) = member(&sender, "postMessage") else {
            return false;
        };

        let port = receiver.clone();
        let landed = landed.clone();
        let waker = waker.clone();
        let handler = Closure::once_into_js(move || {
            if let Some(close) = member(&port, "close") {
                let _ = close.call0(&port);
            }
            landed.set(true);
            waker.borrow().wake_by_ref();
        });
        // Assigning the handler is what starts the receiving port.
        if js_sys::Reflect::set(&receiver, &JsValue::from_str("onmessage"), &handler).is_err() {
            return false;
        }
        post.call1(&sender, &JsValue::UNDEFINED).is_ok()
    }

    /// Ask the global for a `setTimeout` of zero that sets `landed`
    /// and wakes `waker`. `false` when this global has no
    /// `setTimeout` to schedule it with.
    fn schedule_timeout(landed: &Rc<Cell<bool>>, waker: &Rc<RefCell<Waker>>) -> bool {
        let global = js_sys::global();
        let Some(set_timeout) = member(&global, "setTimeout") else {
            return false;
        };
        let landed = landed.clone();
        let waker = waker.clone();
        let callback = Closure::once_into_js(move || {
            landed.set(true);
            waker.borrow().wake_by_ref();
        });
        set_timeout
            .call2(&global, &callback, &JsValue::from_f64(0.0))
            .is_ok()
    }

    /// The function `object` holds under `name`, or `None` when it
    /// holds none.
    fn member(object: &JsValue, name: &str) -> Option<js_sys::Function> {
        js_sys::Reflect::get(object, &JsValue::from_str(name))
            .ok()?
            .dyn_into::<js_sys::Function>()
            .ok()
    }
}

pub use imp::YieldWake;
