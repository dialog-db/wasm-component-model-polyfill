//! Baseline tests for the wake a driver arranges after a yield.
//!
//! A yield gives way: the item that yielded resumes after every
//! other ready item, and its resumption first returns control to the
//! host executor. Natively that is a self-wake. In the browser the
//! driver is polled from a microtask, so a self-wake would land back
//! in the microtask queue ahead of every network response and timer
//! and a guest that spins on a yield would starve the page. The
//! browser's wake therefore crosses a macrotask boundary.
//!
//! Which macrotask it crosses is what these tests pin down. A
//! `setTimeout` of zero reaches the next macrotask, but it carries
//! two floors: the HTML timer initialisation steps clamp a timeout
//! nested more than five deep to four milliseconds, and a background
//! tab throttles timers to about one a second. A guest that yields
//! once per event would then run at a few hundred events a second in
//! the foreground and about one a second in the background, which is
//! the page starving the guest rather than the other way round. A
//! message posted to a `MessageChannel` port is a macrotask under
//! neither floor, so that is what the wake uses, and the timeout
//! stays as the fallback for a global that offers no
//! `MessageChannel`.
//!
//! The guest below counts its own resumptions, so a call that comes
//! back with the number of yields it asked for is a call every one
//! of whose wakes landed.

#![cfg(test)]

use wasm_component_model_polyfill::{Component, Engine, Linker, Store, Val};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// A callback export that gives way a given number of times before
/// it returns.
///
/// The export takes the number of yields to make, stores it, and
/// returns the yield status. Each callback run counts itself,
/// decrements the counter, and yields again until the counter is
/// spent; the run that spends it returns the count through
/// `task.return` and exits. A call therefore resolves only after the
/// driver has arranged, and waited for, one wake per yield.
const SPINS: &[u8] = component!(
    r#"
    (component
      (core func $task-return (canon task.return (result u32)))
      (core module $m
        (import "" "task.return" (func $task-return (param i32)))
        (global $left (mut i32) (i32.const 0))
        (global $runs (mut i32) (i32.const 0))
        (func $step (result i32)
          (if (result i32) (i32.eqz (global.get $left))
            (then
              (call $task-return (global.get $runs))
              (i32.const 0))
            (else (i32.const 1))))
        (func (export "spin") (param i32) (result i32)
          (global.set $left (local.get 0))
          (global.set $runs (i32.const 0))
          (call $step))
        (func (export "spin-callback") (param i32 i32 i32) (result i32)
          (global.set $runs (i32.add (global.get $runs) (i32.const 1)))
          (global.set $left (i32.sub (global.get $left) (i32.const 1)))
          (call $step)))
      (core instance $i (instantiate $m
        (with "" (instance (export "task.return" (func $task-return))))))
      (func (export "spin") async (param "times" u32) (result u32)
        (canon lift (core func $i "spin") async
          (callback (core func $i "spin-callback")))))
    "#
);

/// How many yields the tests drive. A few hundred is enough that a
/// per-yield floor of milliseconds shows up as seconds.
const YIELDS: u32 = 300;

/// Call the spinner for `times` yields and hand back the number of
/// resumptions the guest counted.
async fn spin(times: u32) -> u32 {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, SPINS)
        .await
        .expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the instantiation extracts the callback");
    let spin = instance.get_func("spin").expect("export `spin` not found");
    let results = spin
        .call(&mut store, &[Val::U32(times)])
        .await
        .expect("the call resolves at `task.return`");
    match results.as_ref() {
        [Val::U32(runs)] => *runs,
        other => panic!("the call returns one u32, not {other:?}"),
    }
}

#[wcmp_macros::test]
async fn it_resumes_the_guest_once_per_yield_until_the_call_returns() {
    assert_eq!(
        spin(YIELDS).await,
        YIELDS,
        "every yield's wake landed, so the callback ran once per yield"
    );
}

/// What the browser's wake is made of, measured against a global
/// whose members the test replaces for the length of the test.
#[cfg(target_arch = "wasm32")]
mod browser {
    use core::cell::Cell;
    use std::rc::Rc;

    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_test::wasm_bindgen_test;

    use super::{YIELDS, spin};

    /// A sanity bound on the wall-clock cost of [`YIELDS`] yields.
    ///
    /// It is a sanity check and not the evidence: the evidence that
    /// the wake is a port message is the count of `setTimeout` calls
    /// beside it. The bound is generous all the same — a chain of
    /// nested zero timeouts is clamped to four milliseconds each and
    /// could not finish this many yields in under a second and a
    /// half, so even a loaded machine separates the two.
    const SANITY_BOUND_MS: f64 = 2_000.0;

    /// A member of the global replaced for the length of a test and
    /// put back when the guard drops.
    struct GlobalMember {
        name: &'static str,
        original: JsValue,
    }

    impl GlobalMember {
        /// Replace the global's `name` with `value`.
        fn replace(name: &'static str, value: &JsValue) -> Self {
            let global = js_sys::global();
            let key = JsValue::from_str(name);
            let original =
                js_sys::Reflect::get(&global, &key).expect("the global answers for its members");
            js_sys::Reflect::set(&global, &key, value).expect("the global takes a replacement");
            Self { name, original }
        }
    }

    impl Drop for GlobalMember {
        fn drop(&mut self) {
            let global = js_sys::global();
            let key = JsValue::from_str(self.name);
            let _ = js_sys::Reflect::set(&global, &key, &self.original);
        }
    }

    /// The global's `setTimeout`, replaced by one that counts its
    /// calls and then defers to the original.
    ///
    /// Counting rather than refusing is what lets the fallback test
    /// use the same stub: a wake that reaches for the timeout still
    /// works, and the count is what says whether it did.
    struct CountedTimeout {
        calls: Rc<Cell<u32>>,
        // The member goes back before the closure behind it is
        // freed, so the fields are declared in the order they must
        // drop in.
        _member: GlobalMember,
        _closure: Closure<dyn FnMut(JsValue, JsValue) -> JsValue>,
    }

    impl CountedTimeout {
        /// Install the counting `setTimeout`.
        fn install() -> Self {
            let global = js_sys::global();
            let original = js_sys::Reflect::get(&global, &JsValue::from_str("setTimeout"))
                .expect("the global answers for its members")
                .dyn_into::<js_sys::Function>()
                .expect("the global's `setTimeout` is a function");
            let calls = Rc::new(Cell::new(0));
            let counted = calls.clone();
            let closure = Closure::wrap(Box::new(move |callback: JsValue, delay: JsValue| {
                counted.set(counted.get() + 1);
                original
                    .call2(&js_sys::global(), &callback, &delay)
                    .unwrap_or(JsValue::UNDEFINED)
            })
                as Box<dyn FnMut(JsValue, JsValue) -> JsValue>);
            let member = GlobalMember::replace("setTimeout", closure.as_ref());
            Self {
                calls,
                _member: member,
                _closure: closure,
            }
        }

        /// How many times the global's `setTimeout` was called since
        /// the stub went in.
        fn calls(&self) -> u32 {
            self.calls.get()
        }
    }

    #[wasm_bindgen_test]
    async fn it_wakes_through_a_port_message_and_never_through_a_timeout() {
        let timeouts = CountedTimeout::install();

        let started = js_sys::Date::now();
        let runs = spin(YIELDS).await;
        let elapsed = js_sys::Date::now() - started;

        assert_eq!(runs, YIELDS, "every wake landed");
        assert_eq!(
            timeouts.calls(),
            0,
            "the wake after a yield is a port message, so it never asks the \
             global for a timeout while the global has a `MessageChannel`"
        );
        assert!(
            elapsed < SANITY_BOUND_MS,
            "{YIELDS} yields took {elapsed}ms, past the {SANITY_BOUND_MS}ms \
             sanity bound: a wake under a timer's clamp is the usual reason"
        );
    }

    #[wasm_bindgen_test]
    async fn it_wakes_through_a_timeout_when_the_global_has_no_message_channel() {
        // Four yields, not a few hundred: the fallback is under the
        // timer clamp the other test is there to avoid, and this
        // test asks only whether the driver still comes back.
        const FALLBACK_YIELDS: u32 = 4;

        let timeouts = CountedTimeout::install();
        let _channel = GlobalMember::replace("MessageChannel", &JsValue::UNDEFINED);

        let runs = spin(FALLBACK_YIELDS).await;

        assert_eq!(
            runs, FALLBACK_YIELDS,
            "the fallback still wakes the driver once per yield"
        );
        assert!(
            timeouts.calls() >= FALLBACK_YIELDS,
            "a global without a `MessageChannel` falls back to a timeout per \
             yield, so at least {FALLBACK_YIELDS} timeouts were asked for, \
             not {}",
            timeouts.calls()
        );
    }
}
