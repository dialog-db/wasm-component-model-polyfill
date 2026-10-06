// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

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
//! background. A port message is a macrotask under neither clamp.
//! The timeout stays as the fallback for a global that has no
//! `MessageChannel`, and a global with neither wakes the driver at
//! once, as the native target does.
//!
//! What the wake guarantees is that the guest's next item runs in a
//! later task than the one that yielded: every microtask queued
//! before it runs first, and so does every message the page posted
//! to a port before the yield, since the wake's message joins the
//! same posted-message task source behind them. The HTML event loop
//! lets the browser choose which task source it serves next, so a
//! timer or a network response the page has queued may run before
//! the guest's next item or after it. Chrome serves a timeout of
//! zero queued just before a yield before the guest's next item.
//!
//! The channel belongs to one driver, which makes it at its first
//! yield and posts every later wake through it: a driver has at most
//! one wake on its way, so one port and one flag serve them all.
//! Building and closing a channel costs Chrome about 8 µs, and the
//! `yields` benchmark measured a yield at 16 µs end to end with a
//! channel per wake and at 7 µs with one per driver, so the driver
//! keeps it. A channel per store would also save the build of each
//! later driver's channel, but several drivers of one store can each
//! have a wake on its way at once, so it would need a queue of them;
//! the channel per driver needs none, and is the one taken.
//! A driver that drops before its message arrives takes the handler
//! off the port and closes the channel, so nothing runs for it
//! afterwards and nothing leaks. A wake whose message is posted but
//! never delivered leaves the driver pending until something else
//! wakes it. No browser loses a message posted to an open port, so
//! the wake arms no timer to watch for it, which would cost a timer
//! per yield.

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use core::task::Waker;

    /// The wake a driver arranges each time a turn ends in a yield.
    ///
    /// Natively the wake is immediate: the driver wakes itself and
    /// returns pending, so the executor polls it again after it has
    /// run whatever else is ready. There is nothing to wait for
    /// afterwards, so no wake is ever on its way.
    pub struct YieldWake {
        _private: (),
    }

    impl YieldWake {
        /// The wake of a driver that has not yielded yet.
        pub fn new() -> Self {
            Self { _private: () }
        }

        /// Arrange the wake that returns control to the host
        /// executor after a yield.
        pub fn after_yield(&mut self, waker: &Waker) {
            waker.wake_by_ref();
        }

        /// Whether a wake is on its way, so that the driver may not
        /// run the item that yielded yet.
        pub fn waiting(&self) -> bool {
            false
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

    /// The wake a driver arranges each time a turn ends in a yield.
    ///
    /// In the browser the wake crosses a macrotask boundary: the
    /// driver posts a message to a port of its own and returns
    /// pending, so the guest's next item runs in a later task. The
    /// flag says whether the message of the last wake has arrived.
    pub struct YieldWake {
        landed: Rc<Cell<bool>>,
        waker: Rc<RefCell<Waker>>,
        /// The driver's channel, from its first wake that posts.
        channel: Option<Channel>,
    }

    /// The two ports of a driver's own `MessageChannel`, the handler
    /// the receiving port runs, and the sending port's `postMessage`.
    struct Channel {
        receiver: JsValue,
        sender: JsValue,
        post: js_sys::Function,
        handler: Closure<dyn FnMut()>,
    }

    impl Channel {
        /// Post one message through the channel, answering whether
        /// the post was made.
        fn post(&self) -> bool {
            self.post.call1(&self.sender, &JsValue::UNDEFINED).is_ok()
        }
    }

    impl Drop for Channel {
        /// Take the handler off the receiving port and close both
        /// ports, so that a message still on its way is never
        /// delivered to a handler that is gone.
        fn drop(&mut self) {
            let _ = js_sys::Reflect::set(
                &self.receiver,
                &JsValue::from_str("onmessage"),
                &JsValue::NULL,
            );
            for port in [&self.receiver, &self.sender] {
                if let Some(close) = member(port, "close") {
                    let _ = close.call0(port);
                }
            }
        }
    }

    impl YieldWake {
        /// The wake of a driver that has not yielded yet.
        pub fn new() -> Self {
            Self {
                landed: Rc::new(Cell::new(true)),
                waker: Rc::new(RefCell::new(Waker::noop().clone())),
                channel: None,
            }
        }

        /// Arrange the wake that returns control to the host
        /// executor after a yield.
        pub fn after_yield(&mut self, waker: &Waker) {
            self.after_yield_on(&js_sys::global(), waker);
        }

        /// Arrange the wake through the `MessageChannel` or the
        /// `setTimeout` of `global`. A global with neither wakes
        /// `waker` at once, as the native target does, rather than
        /// stall the driver. A channel whose post fails is given up,
        /// and the wake falls back as it would with none.
        pub fn after_yield_on(&mut self, global: &JsValue, waker: &Waker) {
            self.landed.set(false);
            *self.waker.borrow_mut() = waker.clone();
            if self.channel.is_none() {
                self.channel = open_channel(global, &self.landed, &self.waker);
            }
            if self.channel.as_ref().is_some_and(Channel::post) {
                return;
            }
            self.channel = None;
            if !schedule_timeout(global, &self.landed, &self.waker) {
                self.landed.set(true);
                waker.wake_by_ref();
            }
        }

        /// Whether a wake is on its way, so that the driver may not
        /// run the item that yielded yet.
        pub fn waiting(&self) -> bool {
            !self.landed.get()
        }

        /// Whether the driver's wakes cross a channel of its own,
        /// rather than a timeout or nothing.
        #[cfg(test)]
        pub fn posted(&self) -> bool {
            self.channel.is_some()
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

    /// A fresh `MessageChannel` of `global`, whose receiving port sets
    /// `landed` and wakes `waker` each time a message arrives. `None`
    /// when the global offers no `MessageChannel`, or when building one
    /// from it did not work.
    fn open_channel(
        global: &JsValue,
        landed: &Rc<Cell<bool>>,
        waker: &Rc<RefCell<Waker>>,
    ) -> Option<Channel> {
        let constructor = member(global, "MessageChannel")?;
        let channel = js_sys::Reflect::construct(&constructor, &js_sys::Array::new()).ok()?;
        let receiver = js_sys::Reflect::get(&channel, &JsValue::from_str("port1")).ok()?;
        let sender = js_sys::Reflect::get(&channel, &JsValue::from_str("port2")).ok()?;
        let post = member(&sender, "postMessage")?;

        let landed = landed.clone();
        let waker = waker.clone();
        let handler = Closure::<dyn FnMut()>::new(move || {
            landed.set(true);
            waker.borrow().wake_by_ref();
        });
        let channel = Channel {
            receiver,
            sender,
            post,
            handler,
        };
        // Assigning the handler is what starts the receiving port.
        js_sys::Reflect::set(
            &channel.receiver,
            &JsValue::from_str("onmessage"),
            channel.handler.as_ref(),
        )
        .ok()
        .filter(|set| *set)?;
        Some(channel)
    }

    /// Ask `global` for a `setTimeout` of zero that sets `landed` and
    /// wakes `waker`. `false` when the global has no `setTimeout` to
    /// schedule it with.
    fn schedule_timeout(
        global: &JsValue,
        landed: &Rc<Cell<bool>>,
        waker: &Rc<RefCell<Waker>>,
    ) -> bool {
        let Some(set_timeout) = member(global, "setTimeout") else {
            return false;
        };
        let landed = landed.clone();
        let waker = waker.clone();
        let callback = Closure::once_into_js(move || {
            landed.set(true);
            waker.borrow().wake_by_ref();
        });
        set_timeout
            .call2(global, &callback, &JsValue::from_f64(0.0))
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

impl Default for YieldWake {
    fn default() -> Self {
        Self::new()
    }
}
