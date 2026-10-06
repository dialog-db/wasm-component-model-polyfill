// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A host future that polls the store's scheduler.

use core::future::{Future, poll_fn};
use core::task::{Context, Poll, Waker};

use crate::error::{Error, Result, SchedulerCause};
use crate::store::StoreContext;
use crate::store::StoreContextInternalExt;

use super::outcome::Outcome;
use super::yield_wake::YieldWake;

/// A host future that polls the store's scheduler.
///
/// The scheduler has no thread of its own and no executor of its
/// own: the host drives it through the futures it already awaits.
/// One poll of a driver is a turn. Guest code runs only inside a
/// turn, so every entry point that reaches a guest — a call into an
/// export, an instantiation — is a driver.
///
/// A driver has a condition. It polls turns until the condition
/// holds, and the value the condition yields is what the future
/// resolves to. Five rules hold for every driver:
///
/// - A driver entered while another driver of the same store is
///   inside a turn fails with the recursive-driver cause.
/// - When a turn finds nothing ready, no host task pending, and the
///   condition unmet, the driver fails with the deadlock cause, or
///   with the cannot-block cause when any instance of the store must
///   not suspend.
/// - A failure of a turn, and the idle failure above, are traps, and
///   each poisons the store. What the condition yields is the call's
///   own, and the call decides whether it was a trap.
/// - Dropping the future cancels nothing. Whatever the driver
///   queued stays in the store and runs in the next turn of any
///   driver, unless a trap poisons the store first: the trap
///   discards every queued item and drops every host task.
/// - Dropping the store drops every task, host task, and suspended
///   thread, and no destructor runs.
///
/// This type is the driver of a call into an export. An instantiation
/// runs its plan inside one turn it awaits. The store's
/// `run_concurrent` entry is a driver too, and it keeps every rule but
/// the second: an idle turn leaves it pending rather than failing,
/// because the closure it runs can wait on something outside the store.
///
/// Under a provider that runs a thread once the driver awaits it, a
/// turn that started or resumed such a thread ends there, and the
/// driver awaits the thread, the store's flight, before its next turn.
pub struct Driver;

/// What one run of a driver's turns came to.
enum Step<R> {
    /// The driver is done, with this.
    Done(Result<R>),
    /// A turn left the store a flight, which the driver awaits before
    /// its next turn.
    Fly,
}

impl Driver {
    /// Drive `store` until `condition` yields a value.
    ///
    /// The condition is consulted before each turn, and once more
    /// when a turn goes idle or leaves only a host task pending, so
    /// work the driver itself queued before it was polled is seen
    /// and a turn that resolved the condition never parks.
    pub fn run<'a, T: 'static, C, R>(
        store: StoreContext<'a, T>,
        condition: C,
    ) -> impl Future<Output = Result<R>> + 'a
    where
        C: FnMut(&mut StoreContext<'_, T>, &Waker) -> Option<Result<R>> + 'a,
        R: 'a,
    {
        drive(store, condition)
    }
}

/// The body of a driver: turns until `condition` yields, and the
/// store's flight awaited whenever a turn leaves one.
async fn drive<T: 'static, C, R>(mut store: StoreContext<'_, T>, mut condition: C) -> Result<R>
where
    C: FnMut(&mut StoreContext<'_, T>, &Waker) -> Option<Result<R>>,
{
    if store.internal().turn_in_flight() {
        return Err(Error::Scheduler(SchedulerCause::RecursiveDriver));
    }
    let mut yield_wake = YieldWake::new();
    loop {
        let step =
            poll_fn(|context| poll_turns(&mut store, &mut condition, &mut yield_wake, context))
                .await;
        match step {
            Step::Done(done) => return done,
            Step::Fly => store.internal().fly().await,
        }
    }
}

/// Run turns of `store` until `condition` yields, a turn leaves the
/// store a flight, or nothing can go on until a wake.
fn poll_turns<T: 'static, C, R>(
    store: &mut StoreContext<'_, T>,
    condition: &mut C,
    yield_wake: &mut YieldWake,
    context: &mut Context<'_>,
) -> Poll<Step<R>>
where
    C: FnMut(&mut StoreContext<'_, T>, &Waker) -> Option<Result<R>>,
{
    let waker = context.waker();

    // A turn that ended in a yield returns control to the host
    // executor before the item that yielded runs. Natively that
    // wake is immediate; in the browser it crosses a macrotask
    // boundary, and the driver waits here until it lands.
    if yield_wake.waiting() {
        yield_wake.rewake(waker);
        return Poll::Pending;
    }

    loop {
        // A turn that stopped for a thread the driver awaits is not
        // over, and the condition waits until it is: a turn runs with
        // no condition consulted in between.
        if !store.internal().deferred_busy()
            && let Some(done) = condition(store, waker)
        {
            return Poll::Ready(Step::Done(done));
        }
        let outcome = match store.internal().turn(waker) {
            Ok(outcome) => outcome,
            Err(error) => return Poll::Ready(Step::Done(Err(error))),
        };
        match outcome {
            Outcome::Progress => continue,
            Outcome::Yield => {
                yield_wake.after_yield(waker);
                return Poll::Pending;
            }
            Outcome::Resuming => return Poll::Ready(Step::Fly),
            // A turn that leaves a host task pending can have
            // resolved what this driver waits on all the same:
            // it runs the items that are ready before it polls
            // the host tasks. The condition is therefore
            // consulted once more before the driver parks, or a
            // call whose task has returned would wait for a host
            // task it does not wait for — for ever, against a
            // host task that never returns.
            Outcome::Waiting => {
                if let Some(done) = condition(store, waker) {
                    return Poll::Ready(Step::Done(done));
                }
                return Poll::Pending;
            }
            Outcome::Idle => {
                if let Some(done) = condition(store, waker) {
                    return Poll::Ready(Step::Done(done));
                }
                let cause = store.internal().idle_cause();
                // A thread suspended in the provider can never
                // resume in an idle store, so it traps with the
                // cause, and its failure reaches the call it
                // belongs to as the same trap would with no
                // provider.
                match store.internal().fail_parked_threads() {
                    Ok(true) => {
                        if let Some(done) = condition(store, waker) {
                            return Poll::Ready(Step::Done(done));
                        }
                    }
                    Ok(false) => {}
                    Err(error) => {
                        store.internal().poison();
                        return Poll::Ready(Step::Done(Err(error)));
                    }
                }
                // A store that went idle under the call is the
                // deadlock trap, or the cannot-block trap, and a
                // trap poisons the store.
                store.internal().poison();
                return Poll::Ready(Step::Done(Err(Error::Scheduler(cause))));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::store::{StoreContextInternalExt, StoreInternalExt};
    use core::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use crate::engine::Engine;
    use crate::value::Val;

    use super::super::host_task::HostTask;
    use super::super::item::Item;
    use super::super::item_kind::ItemKind;
    use crate::store::Store;

    use super::*;

    /// What the items of one test wrote as they ran, in order.
    type Log = Arc<Mutex<Vec<&'static str>>>;

    /// A waker that counts the wakes it is sent: the native driver's
    /// self-wake after a yield, and the browser's yield wakes the tests
    /// arrange by hand.
    #[derive(Default)]
    struct Wakes(AtomicUsize);

    impl Wakes {
        fn count(&self) -> usize {
            self.0.load(Ordering::Relaxed)
        }
    }

    impl std::task::Wake for Wakes {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn store() -> Store<()> {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        Store::new(&engine, ()).expect("store")
    }

    fn log() -> Log {
        Arc::new(Mutex::new(Vec::new()))
    }

    /// An item that records that it ran.
    fn marker(log: &Log, name: &'static str) -> Item<()> {
        let log = log.clone();
        Item::new(
            ItemKind::TaskStart,
            move |_store: &mut StoreContext<'_, ()>| {
                log.lock().expect("log").push(name);
                Ok(())
            },
        )
    }

    fn entries(log: &Log) -> Vec<&'static str> {
        log.lock().expect("log").clone()
    }

    /// A condition that is never met, so the driver runs until the
    /// scheduler goes idle.
    fn never(_store: &mut StoreContext<'_, ()>, _waker: &Waker) -> Option<Result<()>> {
        None
    }

    /// Poll `future` once, as an executor would.
    fn poll_once<F: Future>(future: &mut Pin<Box<F>>, waker: &Waker) -> Poll<F::Output> {
        let mut context = Context::from_waker(waker);
        future.as_mut().poll(&mut context)
    }

    fn cause(outcome: Poll<Result<()>>) -> String {
        match outcome {
            Poll::Ready(Err(error)) => error.to_string(),
            Poll::Ready(Ok(())) => "the driver succeeded".to_owned(),
            Poll::Pending => "the driver returned pending".to_owned(),
        }
    }

    /// Give `store` a host task that never returns: what a driver
    /// must not wait for once what it waits on has resolved.
    fn never_returning_host_task(store: &mut StoreContext<'_, ()>) {
        let subtask = store
            .internal()
            .tables()
            .lock()
            .expect("tables")
            .tasks
            .insert_subtask()
            .expect("room under the record cap");
        store.internal().push_host_task(HostTask::from_future(
            subtask,
            |_store: &mut StoreContext<'_, ()>, _outcome: Result<Vec<Val>>| Ok(()),
            core::future::pending::<Result<Vec<Val>>>(),
        ));
    }

    #[wcmp_macros::test]
    fn it_returns_when_the_turn_that_met_its_condition_left_a_host_task_pending() {
        let mut store = store();
        let log = log();
        never_returning_host_task(&mut store.internal().context());
        store
            .internal()
            .scheduler_mut()
            .push_high_priority(marker(&log, "resolved"));

        let watched = log.clone();
        let mut driver = Box::pin(Driver::run(
            store.internal().context(),
            move |_store, _waker| (!watched.lock().expect("log").is_empty()).then(|| Ok(())),
        ));

        let outcome = poll_once(&mut driver, Waker::noop());

        assert_eq!(
            cause(outcome),
            "the driver succeeded",
            "the turn ran the item that met the condition before it polled the \
             host tasks, so the driver returns instead of waiting on a host task \
             that never returns"
        );
    }

    #[wcmp_macros::test]
    fn it_refuses_a_driver_entered_from_inside_a_turn() {
        let mut store = store();
        let seen: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let recorded = seen.clone();
        store
            .internal()
            .scheduler_mut()
            .push_high_priority(Item::new(
                ItemKind::TaskStart,
                move |store: &mut StoreContext<'_, ()>| {
                    let mut nested = Box::pin(Driver::run(store.internal().reborrow(), never));
                    let outcome = poll_once(&mut nested, Waker::noop());
                    *recorded.lock().expect("record") = Some(cause(outcome));
                    Ok(())
                },
            ));

        store.internal().turn(Waker::noop()).expect("turn");

        assert_eq!(
            seen.lock().expect("record").clone(),
            Some(Error::Scheduler(SchedulerCause::RecursiveDriver).to_string()),
            "a driver entered while another is inside a turn fails"
        );
    }

    #[wcmp_macros::test]
    fn it_fails_a_driver_that_goes_idle_with_the_deadlock_cause() {
        let mut store = store();
        let mut driver = Box::pin(Driver::run(store.internal().context(), never));

        let outcome = poll_once(&mut driver, Waker::noop());

        assert_eq!(
            cause(outcome),
            Error::Scheduler(SchedulerCause::Deadlock).to_string(),
            "nothing is ready, no host task is pending, and the condition is unmet"
        );
    }

    #[wcmp_macros::test]
    fn it_names_the_cannot_block_cause_when_any_instance_of_an_idle_store_must_not_suspend() {
        // The instance that must not suspend is not the one the
        // driver's task runs in: the idle store's error reads every
        // instance, as Wasmtime names it.
        let mut store = store();
        {
            let mut guard = store.internal().tables().lock().expect("tables");
            let held = guard.tasks.insert_instance();
            guard
                .tasks
                .instance_mut(held)
                .expect("instance record")
                .may_not_suspend = true;
            let other = guard.tasks.insert_instance();
            guard
                .tasks
                .create_task(None, None, other)
                .expect("room under the record cap");
        }
        let mut driver = Box::pin(Driver::run(store.internal().context(), never));

        let outcome = poll_once(&mut driver, Waker::noop());

        assert_eq!(
            cause(outcome),
            Error::Scheduler(SchedulerCause::CannotBlock).to_string(),
            "an instance of the store must not suspend"
        );
    }

    #[wcmp_macros::test]
    fn it_leaves_the_pending_item_in_the_store_when_a_drivers_future_is_dropped() {
        let mut store = store();
        let log = log();
        store
            .internal()
            .scheduler_mut()
            .push_low_priority(marker(&log, "deferred"));

        {
            let mut abandoned = Box::pin(Driver::run(store.internal().context(), never));
            assert!(
                poll_once(&mut abandoned, Waker::noop()).is_pending(),
                "the turn yielded, so the driver returns pending"
            );
            assert!(
                entries(&log).is_empty(),
                "the item that gave way has not run yet"
            );
        }

        let watched = log.clone();
        let mut other = Box::pin(Driver::run(
            store.internal().context(),
            move |_store, _waker| {
                if watched.lock().expect("log").is_empty() {
                    None
                } else {
                    Some(Ok(()))
                }
            },
        ));
        let outcome = poll_once(&mut other, Waker::noop());

        assert!(
            matches!(outcome, Poll::Ready(Ok(()))),
            "dropping a driver's future cancels nothing"
        );
        assert_eq!(
            entries(&log),
            vec!["deferred"],
            "the pending item runs in the next turn of another driver"
        );
    }

    // The wake after a yield is the one thing the driver does
    // differently per target, so this test and the one below it are
    // each written against one target. Natively the driver wakes
    // itself; in the browser it queues a macrotask, which the test
    // after this one measures.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn it_wakes_itself_and_returns_pending_after_a_yield() {
        let mut store = store();
        let log = log();
        store
            .internal()
            .scheduler_mut()
            .push_low_priority(marker(&log, "deferred"));
        let wakes = Arc::new(Wakes::default());
        let waker = Waker::from(wakes.clone());
        let mut driver = Box::pin(Driver::run(store.internal().context(), never));

        let outcome = poll_once(&mut driver, &waker);

        assert!(outcome.is_pending(), "the driver returns pending");
        assert_eq!(
            wakes.count(),
            1,
            "natively the driver wakes itself, so the executor polls it again \
             after it has run whatever else is ready"
        );
        assert!(
            entries(&log).is_empty(),
            "the item that gave way runs only after that wake"
        );
    }

    #[cfg(target_arch = "wasm32")]
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn it_resumes_after_a_queued_macrotask_when_a_turn_yields() {
        use wasm_bindgen::closure::Closure;
        use wasm_bindgen::{JsCast, JsValue};

        let log = log();

        // A macrotask the page already has queued. The driver's
        // resumption must land behind it, or a guest that spins on a
        // yield would starve everything the page is waiting for.
        let queued = log.clone();
        let global = js_sys::global();
        let set_timeout = js_sys::Reflect::get(&global, &JsValue::from_str("setTimeout"))
            .expect("setTimeout")
            .dyn_into::<js_sys::Function>()
            .expect("setTimeout is a function");
        let callback = Closure::once_into_js(move || {
            queued.lock().expect("log").push("macrotask");
        });
        set_timeout
            .call2(&global, &callback, &JsValue::from_f64(0.0))
            .expect("queue the macrotask");

        let mut store = store();
        store
            .internal()
            .scheduler_mut()
            .push_low_priority(marker(&log, "resumed"));

        let watched = log.clone();
        Driver::run(store.internal().context(), move |_store, _waker| {
            if watched.lock().expect("log").contains(&"resumed") {
                Some(Ok(()))
            } else {
                None
            }
        })
        .await
        .expect("drive the yield to its resumption");

        assert_eq!(
            entries(&log),
            vec!["macrotask", "resumed"],
            "the wake after a yield crosses a macrotask boundary, so the \
             resumption runs after the macrotask the page had queued"
        );
    }

    /// Queue a `setTimeout` of zero on the page's global that pushes
    /// `entry` onto `log`.
    #[cfg(target_arch = "wasm32")]
    fn queue_timeout(log: &Log, entry: &'static str) {
        use wasm_bindgen::closure::Closure;
        use wasm_bindgen::{JsCast, JsValue};

        let queued = log.clone();
        let global = js_sys::global();
        let set_timeout = js_sys::Reflect::get(&global, &JsValue::from_str("setTimeout"))
            .expect("setTimeout")
            .dyn_into::<js_sys::Function>()
            .expect("setTimeout is a function");
        let callback = Closure::once_into_js(move || {
            queued.lock().expect("log").push(entry);
        });
        set_timeout
            .call2(&global, &callback, &JsValue::from_f64(0.0))
            .expect("queue the timeout");
    }

    /// Drive `store` until `entry` is on `log`.
    #[cfg(target_arch = "wasm32")]
    async fn drive_until(store: &mut Store<()>, log: &Log, entry: &'static str) {
        let watched = log.clone();
        Driver::run(store.internal().context(), move |_store, _waker| {
            if watched.lock().expect("log").contains(&entry) {
                Some(Ok(()))
            } else {
                None
            }
        })
        .await
        .expect("drive the yield to its resumption");
    }

    #[cfg(target_arch = "wasm32")]
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn it_runs_a_timeout_queued_just_before_a_yield_in_the_order_the_browser_chooses() {
        // The timeout is queued with no work between it and the yield.
        // The HTML event loop leaves the order of two task sources to
        // the browser, so the wake does not promise that the timeout
        // runs first; this pins what Chrome does, which is to run it
        // first.
        let mut store = store();
        let log = log();
        store
            .internal()
            .scheduler_mut()
            .push_low_priority(marker(&log, "resumed"));
        queue_timeout(&log, "timeout");
        drive_until(&mut store, &log, "resumed").await;
        drive_until(&mut store, &log, "timeout").await;

        assert_eq!(entries(&log), vec!["timeout", "resumed"]);
    }

    #[cfg(target_arch = "wasm32")]
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn it_resumes_after_a_message_posted_before_the_yield_so_the_wake_is_a_macrotask() {
        // A message the page posted to a port of its own before the
        // yield is a task in the posted-message source, ahead of the
        // wake's own message in the same source. A wake that were a
        // microtask would resume the item before that task ran.
        use wasm_bindgen::JsValue;
        use wasm_bindgen::closure::Closure;

        let mut store = store();
        let log = log();
        store
            .internal()
            .scheduler_mut()
            .push_low_priority(marker(&log, "resumed"));

        let channel = web_channel();
        let port1 = js_sys::Reflect::get(&channel, &JsValue::from_str("port1")).expect("port1");
        let port2 = js_sys::Reflect::get(&channel, &JsValue::from_str("port2")).expect("port2");
        let delivered = log.clone();
        let handler = Closure::<dyn FnMut()>::new(move || {
            delivered.lock().expect("log").push("message");
        });
        js_sys::Reflect::set(&port1, &JsValue::from_str("onmessage"), handler.as_ref())
            .expect("the handler");
        call(&port2, "postMessage", &JsValue::UNDEFINED);

        drive_until(&mut store, &log, "resumed").await;
        call(&port1, "close", &JsValue::UNDEFINED);
        drop(handler);

        assert_eq!(entries(&log), vec!["message", "resumed"]);
    }

    #[cfg(target_arch = "wasm32")]
    #[wcmp_macros::test]
    fn it_wakes_at_once_on_a_global_with_neither_a_channel_nor_a_timeout() {
        let wakes = Arc::new(Wakes::default());
        let waker = Waker::from(wakes.clone());
        let mut wake = YieldWake::new();
        wake.after_yield_on(&js_sys::Object::new(), &waker);

        assert!(!wake.posted(), "there was no channel to post to");
        assert!(!wake.waiting(), "the wake landed at once");
        assert_eq!(wakes.count(), 1, "and woke the driver, as natively");
    }

    #[cfg(target_arch = "wasm32")]
    #[wcmp_macros::test]
    fn it_falls_back_past_a_channel_with_no_ports() {
        // `Object` constructs an object with no ports, which is a
        // channel the wake cannot post through. With no timeout either,
        // the wake falls through both mechanisms and lands at once.
        use wasm_bindgen::JsValue;

        let global = js_sys::Object::new();
        let object =
            js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("Object")).expect("Object");
        js_sys::Reflect::set(&global, &JsValue::from_str("MessageChannel"), &object)
            .expect("a fake channel constructor");
        let wakes = Arc::new(Wakes::default());
        let waker = Waker::from(wakes.clone());
        let mut wake = YieldWake::new();
        wake.after_yield_on(&global, &waker);

        assert!(!wake.posted(), "a channel with no ports was not posted to");
        assert!(!wake.waiting());
        assert_eq!(wakes.count(), 1);
    }

    #[cfg(target_arch = "wasm32")]
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn it_runs_nothing_for_a_driver_wake_dropped_before_its_message_arrived() {
        let wakes = Arc::new(Wakes::default());
        let waker = Waker::from(wakes.clone());
        let mut wake = YieldWake::new();
        wake.after_yield(&waker);
        assert!(wake.posted(), "the page has a MessageChannel");
        let receiver = wake.receiver().expect("the channel's receiving port");
        drop(wake);
        assert!(
            js_sys::Reflect::get(&receiver, &wasm_bindgen::JsValue::from_str("onmessage"))
                .expect("the port's handler")
                .is_null(),
            "the dropped wake took its handler off the port"
        );

        // Let the posted message's task come and go.
        let log = log();
        queue_timeout(&log, "later");
        let mut store = store();
        store
            .internal()
            .scheduler_mut()
            .push_low_priority(marker(&log, "resumed"));
        drive_until(&mut store, &log, "resumed").await;
        drive_until(&mut store, &log, "later").await;

        assert_eq!(
            wakes.count(),
            0,
            "the dropped wake took its handler off the port and closed it"
        );
    }

    /// A fresh `MessageChannel` of the page's global.
    #[cfg(target_arch = "wasm32")]
    fn web_channel() -> wasm_bindgen::JsValue {
        use wasm_bindgen::{JsCast, JsValue};

        let constructor =
            js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("MessageChannel"))
                .expect("MessageChannel")
                .dyn_into::<js_sys::Function>()
                .expect("MessageChannel is a constructor");
        js_sys::Reflect::construct(&constructor, &js_sys::Array::new()).expect("a channel")
    }

    /// Call the method `name` of `object` with `argument`.
    #[cfg(target_arch = "wasm32")]
    fn call(object: &wasm_bindgen::JsValue, name: &str, argument: &wasm_bindgen::JsValue) {
        use wasm_bindgen::{JsCast, JsValue};

        js_sys::Reflect::get(object, &JsValue::from_str(name))
            .expect("the method")
            .dyn_into::<js_sys::Function>()
            .expect("a function")
            .call1(object, argument)
            .expect("the call");
    }
}
