//! A host future that polls the store's scheduler.

use core::future::Future;
use core::marker::PhantomData;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};

use crate::error::{Error, Result, SchedulerCause};
use crate::store::StoreContext;
use crate::store::StoreContextInternalExt;

use super::outcome::Outcome;
use super::task_id::TaskId;
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
/// resolves to. Four rules hold for every driver:
///
/// - A driver entered while another driver of the same store is
///   inside a turn fails with the recursive-driver cause.
/// - When a turn finds nothing ready, no host task pending, and the
///   condition unmet, the driver fails with the deadlock cause, or
///   with the cannot-block cause when the task it waits on must not
///   block.
/// - Dropping the future cancels nothing. Whatever the driver
///   queued stays in the store and runs in the next turn of any
///   driver.
/// - Dropping the store drops every task, host task, and suspended
///   thread, and no destructor runs.
///
/// This type is the driver of a call into an export and of an
/// instantiation. The store's `run_concurrent` entry is a driver
/// too, and it keeps every rule but the second: an idle turn leaves
/// it pending rather than failing, because the closure it runs can
/// wait on something outside the store.
pub struct Driver<'a, T: 'static, C, R> {
    store: StoreContext<'a, T>,
    condition: C,
    task: Option<TaskId>,
    entered: bool,
    yield_wake: Option<YieldWake>,
    _result: PhantomData<fn() -> R>,
}

impl<'a, T: 'static, C, R> Driver<'a, T, C, R>
where
    C: FnMut(&mut StoreContext<'_, T>, &Waker) -> Option<Result<R>>,
{
    /// Drive `store` until `condition` yields a value.
    ///
    /// The condition is consulted before each turn, and once more
    /// when a turn goes idle or leaves only a host task pending, so
    /// work the driver itself queued before it was polled is seen
    /// and a turn that resolved the condition never parks. `task`
    /// is the task the driver waits on, when there is one: it
    /// decides whether an idle turn is a deadlock or a task that
    /// must not block.
    pub fn new(store: StoreContext<'a, T>, task: Option<TaskId>, condition: C) -> Self {
        Self {
            store,
            condition,
            task,
            entered: false,
            yield_wake: None,
            _result: PhantomData,
        }
    }
}

impl<'a, T: 'static, C, R> Future for Driver<'a, T, C, R>
where
    C: FnMut(&mut StoreContext<'_, T>, &Waker) -> Option<Result<R>> + Unpin,
{
    type Output = Result<R>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let waker = context.waker();

        if !this.entered {
            this.entered = true;
            if this.store.internal().turn_in_flight() {
                return Poll::Ready(Err(Error::Scheduler(SchedulerCause::RecursiveDriver)));
            }
        }

        // A turn that ended in a yield returns control to the host
        // executor before the item that yielded runs. Natively that
        // wake is immediate; in the browser it crosses a macrotask
        // boundary, and the driver waits here until it lands.
        if let Some(wake) = &this.yield_wake {
            if !wake.landed() {
                return Poll::Pending;
            }
            this.yield_wake = None;
        }

        loop {
            if let Some(done) = (this.condition)(&mut this.store, waker) {
                return Poll::Ready(done);
            }
            let outcome = match this.store.internal().turn(waker) {
                Ok(outcome) => outcome,
                Err(error) => return Poll::Ready(Err(error)),
            };
            match outcome {
                Outcome::Progress => continue,
                Outcome::Yield => {
                    this.yield_wake = Some(YieldWake::after_yield(waker));
                    return Poll::Pending;
                }
                // A turn that leaves a host task pending can have
                // resolved what this driver waits on all the same:
                // it runs the items that are ready before it polls
                // the host tasks. The condition is therefore
                // consulted once more before the driver parks, or a
                // call whose task has returned would wait for a host
                // task it does not wait for — for ever, against a
                // host task that never returns.
                Outcome::Waiting => {
                    if let Some(done) = (this.condition)(&mut this.store, waker) {
                        return Poll::Ready(done);
                    }
                    return Poll::Pending;
                }
                Outcome::Idle => {
                    if let Some(done) = (this.condition)(&mut this.store, waker) {
                        return Poll::Ready(done);
                    }
                    return Poll::Ready(Err(Error::Scheduler(
                        this.store.internal().idle_cause(this.task),
                    )));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::store::{StoreContextInternalExt, StoreInternalExt};
    #[cfg(not(target_arch = "wasm32"))]
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

    /// A waker that counts the wakes a driver sends itself. Only the
    /// native wake after a yield is a self-wake, so only the native
    /// test needs to count them.
    #[cfg(not(target_arch = "wasm32"))]
    #[derive(Default)]
    struct Wakes(AtomicUsize);

    #[cfg(not(target_arch = "wasm32"))]
    impl Wakes {
        fn count(&self) -> usize {
            self.0.load(Ordering::Relaxed)
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    impl std::task::Wake for Wakes {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn store() -> Store<()> {
        let engine = Engine::new().expect("engine");
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
            .insert_subtask();
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
        let mut driver = Box::pin(Driver::new(
            store.internal().context(),
            None,
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
                    let mut nested =
                        Box::pin(Driver::new(store.internal().reborrow(), None, never));
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
        let mut driver = Box::pin(Driver::new(store.internal().context(), None, never));

        let outcome = poll_once(&mut driver, Waker::noop());

        assert_eq!(
            cause(outcome),
            Error::Scheduler(SchedulerCause::Deadlock).to_string(),
            "nothing is ready, no host task is pending, and the condition is unmet"
        );
    }

    #[wcmp_macros::test]
    fn it_fails_a_driver_whose_task_must_not_block_with_the_cannot_block_cause() {
        let mut store = store();
        let task = {
            let mut guard = store.internal().tables().lock().expect("tables");
            let instance = guard.tasks.insert_instance();
            guard
                .tasks
                .instance_mut(instance)
                .expect("instance record")
                .may_not_suspend = true;
            guard.tasks.create_task(None, None, instance)
        };
        let mut driver = Box::pin(Driver::new(store.internal().context(), Some(task), never));

        let outcome = poll_once(&mut driver, Waker::noop());

        assert_eq!(
            cause(outcome),
            Error::Scheduler(SchedulerCause::CannotBlock).to_string(),
            "the task the driver waits on is one that must not block"
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
            let mut abandoned = Box::pin(Driver::new(store.internal().context(), None, never));
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
        let mut other = Box::pin(Driver::new(
            store.internal().context(),
            None,
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
        let mut driver = Box::pin(Driver::new(store.internal().context(), None, never));

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
        Driver::new(store.internal().context(), None, move |_store, _waker| {
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
}
