//! Something outside the store to wait on, and what the host did
//! while the wait was on.
//!
//! The `run_concurrent` step needs a future the store cannot
//! resolve. Nothing the scheduler owns will do: a turn that finds
//! the store idle has to leave the entry pending because the closure
//! is waiting on the host, not because the store has work left. A
//! timer is the plainest thing of that kind, and it is the one line
//! of the step that differs per target — the same split the
//! polyfill's own wake after a yield makes.
//!
//! Natively the timer is the executor's: `tokio::time::sleep`. The
//! evidence that the wait cost the store nothing is the hand poll
//! the step makes, which finds the entry pending rather than failed
//! with the deadlock cause.
//!
//! In the browser the timer is a `setTimeout`-backed promise,
//! reached through the global rather than through `window`, so it
//! works in a worker as well as on a page. There the extra evidence
//! is that the page kept turning: a `setTimeout` of zero asked for
//! before the wait runs only if the event loop is still delivering
//! callbacks, which a page a wait had frozen would never reach.

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use core::time::Duration;

    /// Something outside the store to wait on, and what the host did
    /// while the wait was on. See the module's documentation.
    pub struct Outside {
        _private: (),
    }

    impl Outside {
        /// Start watching what the host does while a wait is on.
        ///
        /// Natively there is nothing to arrange: the executor's own
        /// timer is what ends the wait, so that it ended at all is
        /// the whole of the observation.
        pub fn watching() -> Self {
            Self { _private: () }
        }

        /// Wait `millis` milliseconds on something the store cannot
        /// resolve.
        pub async fn pause(millis: u32) {
            tokio::time::sleep(Duration::from_millis(millis.into())).await;
        }

        /// What the host did while the wait was on.
        pub fn observed(&self) -> Result<String, String> {
            Ok("the executor's timer ran while nothing polled the entry".to_owned())
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use core::cell::Cell;
    use std::rc::Rc;

    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;

    /// Something outside the store to wait on, and what the host did
    /// while the wait was on. See the module's documentation.
    pub struct Outside {
        /// Whether the `setTimeout` of zero asked for before the
        /// wait has run.
        ran: Rc<Cell<bool>>,
    }

    impl Outside {
        /// Start watching what the host does while a wait is on: ask
        /// for a `setTimeout` of zero straight away, so that a page
        /// still delivering callbacks runs it before the wait ends.
        pub fn watching() -> Self {
            let ran = Rc::new(Cell::new(false));
            let flag = ran.clone();
            let callback = Closure::once_into_js(move || flag.set(true));
            if !set_timeout(&callback, 0.0) {
                // No `setTimeout` on this global, so there is
                // nothing to watch and nothing to report against.
                ran.set(true);
            }
            Self { ran }
        }

        /// Wait `millis` milliseconds on something the store cannot
        /// resolve.
        pub async fn pause(millis: u32) {
            let promise = js_sys::Promise::new(&mut |resolve, _reject| {
                if !set_timeout(&JsValue::from(resolve.clone()), millis.into()) {
                    // Nothing to schedule the wait with: resolve at
                    // once rather than hang the page.
                    let _ = resolve.call0(&JsValue::NULL);
                }
            });
            let _ = JsFuture::from(promise).await;
        }

        /// What the host did while the wait was on.
        pub fn observed(&self) -> Result<String, String> {
            if !self.ran.get() {
                return Err(
                    "the page never ran the `setTimeout` asked for before the wait".to_owned(),
                );
            }
            Ok(
                "the `setTimeout` of zero asked for before the wait ran, so the page kept \
                delivering callbacks across it"
                    .to_owned(),
            )
        }
    }

    /// Ask the global for a `setTimeout` of `millis` that calls
    /// `callback`. `false` when this global has no `setTimeout`.
    fn set_timeout(callback: &JsValue, millis: f64) -> bool {
        let global = js_sys::global();
        let Ok(entry) = js_sys::Reflect::get(&global, &JsValue::from_str("setTimeout")) else {
            return false;
        };
        let Some(set_timeout) = entry.dyn_ref::<js_sys::Function>().cloned() else {
            return false;
        };
        set_timeout
            .call2(&global, callback, &JsValue::from_f64(millis))
            .is_ok()
    }
}

pub use imp::Outside;
