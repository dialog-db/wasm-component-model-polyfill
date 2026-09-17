//! The scheduler's one suspend capability, and the nested turn it
//! falls back to.

use crate::error::{Error, Result};
use crate::store::Store;

use super::SuspendProvider;
use super::outcome::Outcome;

/// The boxed provider of the suspend capability, with the `Send`
/// bound the native target puts on everything a store holds. It is
/// the bound a host task's future carries, and no more: a store is
/// `Send` and not `Sync` today, so nothing in it needs `Sync`.
#[cfg(not(target_arch = "wasm32"))]
type BoxedProvider<T> = Box<dyn SuspendProvider<T> + Send>;

/// The boxed provider of the suspend capability. The browser drops
/// the `Send` bound, as it does for a host task's future: the
/// provider it will hold is a JavaScript object.
#[cfg(target_arch = "wasm32")]
type BoxedProvider<T> = Box<dyn SuspendProvider<T>>;

/// The scheduler's one suspend capability.
///
/// A blocking built-in asks the seam to suspend the current guest
/// thread until a readiness condition holds. The seam is a slot a
/// target fills with a [`SuspendProvider`], and the polyfill fills
/// it on neither target today, so every suspension takes the
/// fallback below.
///
/// The fallback is a nested turn, run from inside the guest call
/// that blocked. It runs the guest work of other tasks that is
/// ready and polls the host tasks the executor woke, with the waker
/// of the outer turn, until the condition holds or nothing can
/// progress. A host task that stays pending inside a nested turn
/// stays in the store for the outer turn, so no wake is lost.
///
/// If the nested turn goes idle with the condition unmet, the
/// built-in traps. A task that must not block gets the cannot-block
/// cause, which is the rule of the reference. A task that is allowed
/// to block gets the stack-switch cause: the reference permits that
/// block, and only the target cannot serve it.
///
/// A nested executor that blocks the native thread is not an option
/// here. It deadlocks under a current-thread executor, tokio forbids
/// it inside a runtime, and it has no browser counterpart.
///
/// A nested turn is not a driver. The rule that refuses a driver
/// entered while another driver of the same store is inside a turn
/// does not apply to it: the seam neither consults the count of
/// running turns nor raises it, and it polls host tasks with the
/// waker the outer turn recorded rather than one of its own.
///
/// That count is the other nesting the scheduler knows about, and
/// it is not this one. A host task body reaches the store through
/// the accessor its poll hands it, and the closure it runs there is
/// guest work, so it enters a turn of its own and raises the count
/// — which is what makes a driver entered from inside that closure
/// refuse itself, and what puts the outer turn's waker back when the
/// closure ends. A nested turn of the seam wants neither: it must
/// not refuse itself, and it must keep the outer turn's waker where
/// a host task it polls can find it. The two therefore stay
/// separate, and they compose: a body's closure that reaches a
/// blocking built-in gets a nested turn, and an item that nested
/// turn runs which reaches the seam again is refused by the rule
/// below.
///
/// Two rules bound what a nested turn may do, and both follow from
/// where it runs — inside a guest call, with no way to hand control
/// back to the host executor and no way to unwind the frames beneath
/// it.
///
/// - **A nested turn leaves every resumption after a yield alone.**
///   It neither takes the resume-after-yield slot nor fills it, and
///   it runs nothing from the low-priority queue. A yield gives way
///   to every other ready item and its resumption first returns
///   control to the host executor; a nested turn cannot return that
///   control, so only the outer turn resumes a yielded item. A
///   nested turn that finds nothing left but deferred work reports
///   [`Outcome::Yield`] and stops, and the suspension traps if its
///   condition is still unmet. The deferred item stays queued and
///   the outer turn defers and runs it as it always would, so
///   nothing is lost.
/// - **A nested turn nests no further.** An item a nested turn runs
///   that itself reaches the seam is refused at once, with the same
///   cause an idle nested turn produces: cannot-block for a task
///   that must not block, and stack-switch otherwise. Each level is
///   a real native frame under the guest call that blocked, nothing
///   in the reference bounds how many levels a guest can ask for,
///   and a deeper level could only reach the same work the level
///   above it already offers. Depth one is therefore the whole of
///   the fallback, and a target that fills the provider slot serves
///   the rest by switching stacks.
pub struct SuspendSeam<T: 'static> {
    provider: Option<BoxedProvider<T>>,
    in_nested_turn: bool,
}

impl<T: 'static> SuspendSeam<T> {
    /// Construct the seam with its provider slot empty, which is
    /// what both targets start with.
    pub fn new() -> Self {
        Self {
            provider: None,
            in_nested_turn: false,
        }
    }

    /// Whether a target has filled the capability.
    pub fn has_provider(&self) -> bool {
        self.provider.is_some()
    }

    /// Whether a nested turn of this store is running, which is what
    /// bounds the fallback to one level.
    pub fn in_nested_turn(&self) -> bool {
        self.in_nested_turn
    }

    /// Fill the capability with `provider`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn set_provider(&mut self, provider: impl SuspendProvider<T> + Send) {
        self.provider = Some(Box::new(provider));
    }

    /// Fill the capability with `provider`. See the native
    /// definition; the `Send` half of the bound is absent here, as it
    /// is on a host task's future.
    #[cfg(target_arch = "wasm32")]
    pub fn set_provider(&mut self, provider: impl SuspendProvider<T>) {
        self.provider = Some(Box::new(provider));
    }

    /// Suspend the current guest thread until `condition` holds.
    ///
    /// This is the one entry a blocking built-in uses. It returns
    /// `Ok(())` with the condition true, and otherwise the scheduler
    /// error the built-in traps with.
    pub fn suspend(
        store: &mut Store<T>,
        mut condition: impl FnMut(&mut Store<T>) -> bool,
    ) -> Result<()> {
        // The provider slot is consulted first. When a target fills
        // the capability the thread suspends there, and no guest
        // code is entered from the trampoline this runs in — which
        // is what a provider that switches stacks needs. The nested
        // turn runs only when the slot is empty, so the two never
        // meet.
        if store.scheduler.suspend_seam().has_provider() {
            return Self::suspend_with_provider(store, &mut condition);
        }
        Self::run_nested_turns(store, &mut condition)
    }

    /// Hand the suspension to the target's provider. The provider
    /// leaves the slot for the duration of the call, because it runs
    /// against the store the slot sits in, and goes back into it
    /// afterwards — whether the call returned or unwound. A provider
    /// a panic swallowed would leave the seam with an empty slot for
    /// the life of the store, and every later suspension would take
    /// the nested-turn fallback on a target that had a provider.
    fn suspend_with_provider(
        store: &mut Store<T>,
        condition: &mut dyn FnMut(&mut Store<T>) -> bool,
    ) -> Result<()> {
        let Some(mut provider) = store.scheduler.suspend_seam_mut().provider.take() else {
            return Self::run_nested_turns(store, condition);
        };
        // `provider` stays in this frame. The closure only borrows
        // it, so an unwind through the call leaves it here to put
        // back rather than dropping it inside the closure.
        let outcome = Self::caught(|| provider.suspend(&mut *store, condition));
        store.scheduler.suspend_seam_mut().provider = Some(provider);
        Self::resume(outcome)
    }

    /// The fallback: turns of the store's scheduler run from inside
    /// the guest call that blocked, until the condition holds or
    /// nothing can progress.
    fn run_nested_turns(
        store: &mut Store<T>,
        condition: &mut dyn FnMut(&mut Store<T>) -> bool,
    ) -> Result<()> {
        // The fallback is one level deep. An item a nested turn runs
        // that reaches the seam again gets the condition checked and
        // then the refusal, because a second level would add a
        // native frame under the guest call without reaching any
        // work the level above it does not already offer.
        if store.scheduler.suspend_seam().in_nested_turn() {
            if condition(store) {
                return Ok(());
            }
            return Err(Error::Scheduler(store.suspend_cause()));
        }
        store.scheduler.suspend_seam_mut().in_nested_turn = true;
        let outcome = Self::caught(|| Self::nested_turn_loop(&mut *store, condition));
        store.scheduler.suspend_seam_mut().in_nested_turn = false;
        Self::resume(outcome)
    }

    /// Run `body` and hand back what it did, an unwind included.
    ///
    /// The seam cannot pair what it borrows with a guard the way a
    /// turn does. What a turn marks lives behind the store's handle
    /// tables, which a guard can hold a handle to; what the seam
    /// marks lives on the store itself, and the body needs the store
    /// mutably for as long as it runs, so no value can hold both.
    /// The seam therefore catches the unwind, puts back what it
    /// borrowed, and lets the panic carry on from where it was. The
    /// browser aborts on a panic rather than unwinding, so there
    /// nothing is ever caught and the path that returns is the whole
    /// of it.
    fn caught<R>(body: impl FnOnce() -> R) -> std::thread::Result<R> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(body))
    }

    /// Hand back what [`caught`](Self::caught) returned: the value,
    /// or the panic, continuing its unwind.
    fn resume<R>(outcome: std::thread::Result<R>) -> R {
        match outcome {
            Ok(value) => value,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }

    /// The nested turns themselves, with the seam already marked as
    /// running one.
    fn nested_turn_loop(
        store: &mut Store<T>,
        condition: &mut dyn FnMut(&mut Store<T>) -> bool,
    ) -> Result<()> {
        // The waker of the outer turn, so that a host task polled
        // here carries the waker the executor already holds. There
        // is none when no turn is running — a thread resumed outside
        // any poll of a driver — and a waker that does nothing
        // serves instead, as it does for a trampoline that starts a
        // host task outside a turn.
        let waker = store.active_waker();
        loop {
            if condition(store) {
                return Ok(());
            }
            match store.nested_turn(&waker)? {
                Outcome::Progress => continue,
                // Nothing more can progress from inside the guest
                // call. `Waiting` leaves its host tasks in the store
                // for the outer turn to poll again, and `Yield` says
                // the only work left is a resumption after a yield,
                // which the nested turn left queued: that resumption
                // first returns control to the host executor, and
                // only the outer turn can return it.
                Outcome::Yield | Outcome::Waiting | Outcome::Idle => break,
            }
        }
        if condition(store) {
            return Ok(());
        }
        Err(Error::Scheduler(store.suspend_cause()))
    }
}

impl<T: 'static> Default for SuspendSeam<T> {
    fn default() -> Self {
        Self::new()
    }
}

// The seam is one code path on both targets, and every test here
// measures it on both. Each carries the pair of attributes the
// crate's cross-target test attribute expands to, because that
// attribute takes an `async fn` and none of these bodies awaits
// anything: a nested turn is a synchronous call from inside a guest
// call. A plain `#[test]` would run natively only, since the browser
// runner collects `wasm_bindgen_test` functions.
#[cfg(test)]
mod tests {
    use core::future::Future;
    use core::pin::Pin;
    use core::task::{Context, Poll, Waker};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use crate::engine::Engine;
    use crate::error::SchedulerCause;
    use crate::value::Val;

    use super::super::driver::Driver;
    use super::super::host_task::HostTask;
    use super::super::item::Item;
    use super::super::item_kind::ItemKind;
    use super::*;

    /// What the items of one test wrote as they ran, in order.
    type Log = Arc<Mutex<Vec<&'static str>>>;

    /// Where a test's host task leaves what its body produced. The
    /// lowering is what fills it, and a lowering runs as a queued
    /// item in a later turn than the poll that saw the body
    /// complete, so the slot is what every test here watches as its
    /// readiness condition.
    type Slot = Arc<Mutex<Option<Result<Vec<Val>>>>>;

    /// Whether each poll of a test's host task saw the waker of the
    /// outer turn.
    type Polls = Arc<Mutex<Vec<bool>>>;

    /// A waker with an identity of its own, so that a test can tell
    /// the waker of the outer turn from any other waker the store
    /// might reach for, and which counts the wakes it receives.
    #[derive(Default)]
    struct Outer(AtomicUsize);

    impl std::task::Wake for Outer {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// A host task's future that records, for every poll, whether
    /// the waker it was polled with is the one the outer turn holds,
    /// and completes on its `ready_on`th poll.
    struct Probe {
        outer: Waker,
        ready_on: usize,
        polls: Polls,
    }

    impl Future for Probe {
        type Output = Result<Vec<Val>>;

        fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
            let this = self.get_mut();
            let matched = context.waker().will_wake(&this.outer);
            let polled = {
                let mut polls = this.polls.lock().expect("polls");
                polls.push(matched);
                polls.len()
            };
            if polled >= this.ready_on {
                Poll::Ready(Ok(Vec::new()))
            } else {
                Poll::Pending
            }
        }
    }

    /// A provider that records that the seam consulted it and
    /// returns at once, as one that suspended and resumed would.
    struct Recorded(Arc<Mutex<usize>>);

    impl SuspendProvider<()> for Recorded {
        fn suspend(
            &mut self,
            _store: &mut Store<()>,
            _condition: &mut dyn FnMut(&mut Store<()>) -> bool,
        ) -> Result<()> {
            *self.0.lock().expect("provider calls") += 1;
            Ok(())
        }
    }

    /// A provider that panics where one that switched stacks would
    /// have suspended.
    #[cfg(not(target_arch = "wasm32"))]
    struct Panics;

    #[cfg(not(target_arch = "wasm32"))]
    impl SuspendProvider<()> for Panics {
        fn suspend(
            &mut self,
            _store: &mut Store<()>,
            _condition: &mut dyn FnMut(&mut Store<()>) -> bool,
        ) -> Result<()> {
            panic!("the provider panicked")
        }
    }

    /// Run `body` and catch the panic it is expected to unwind
    /// with, keeping the report of that panic out of the test's
    /// output.
    #[cfg(not(target_arch = "wasm32"))]
    fn unwind<R>(body: impl FnOnce() -> R) -> std::thread::Result<R> {
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
        std::panic::set_hook(hook);
        outcome
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
        Item::new(ItemKind::TaskStart, move |_store: &mut Store<()>| {
            log.lock().expect("log").push(name);
            Ok(())
        })
    }

    fn entries(log: &Log) -> Vec<&'static str> {
        log.lock().expect("log").clone()
    }

    /// An item that records that it ran, reaches the seam with a
    /// condition nothing will ever meet, and records that its call
    /// returned. The pair of entries brackets everything the
    /// suspension ran from inside this item's guest call.
    fn blocker(log: &Log, enter: &'static str, leave: &'static str) -> Item<()> {
        let log = log.clone();
        Item::new(ItemKind::TaskStart, move |store: &mut Store<()>| {
            log.lock().expect("log").push(enter);
            let _ = SuspendSeam::suspend(store, |_| false);
            log.lock().expect("log").push(leave);
            Ok(())
        })
    }

    /// Give `store` a host task whose body completes on its
    /// `ready_on`th poll. The slot it returns is the one the
    /// lowering of that body fills, and it is what every test here
    /// uses as its readiness condition.
    fn host_task(store: &mut Store<()>, outer: &Waker, ready_on: usize) -> (Slot, Polls) {
        let subtask = store.tables.lock().expect("tables").tasks.insert_subtask();
        let slot: Slot = Arc::new(Mutex::new(None));
        let polls: Polls = Arc::new(Mutex::new(Vec::new()));
        let filled = slot.clone();
        store.push_host_task(HostTask::from_future(
            subtask,
            move |_store: &mut Store<()>, outcome: Result<Vec<Val>>| {
                *filled.lock().expect("slot") = Some(outcome);
                Ok(())
            },
            Probe {
                outer: outer.clone(),
                ready_on,
                polls: polls.clone(),
            },
        ));
        (slot, polls)
    }

    /// Make a task of a fresh instance the current scope, as a
    /// blocking built-in would find it. `may_not_suspend` is the
    /// instance flag an adapter's enter intrinsic sets for the
    /// duration of a synchronous call.
    fn current_task(store: &Store<()>, may_not_suspend: bool) {
        let mut guard = store.tables.lock().expect("tables");
        let instance = guard.tasks.insert_instance();
        guard
            .tasks
            .instance_mut(instance)
            .expect("instance record")
            .may_not_suspend = may_not_suspend;
        let task = guard.tasks.create_task(None, None, instance);
        guard.tasks.push_task_scope(task);
    }

    /// Run turns of `store` until it goes idle, as a driver that
    /// loops on `Progress` and returns control to the host executor
    /// on `Yield` would. The bound is there so a store that would
    /// never settle fails the test rather than hanging it.
    fn drain(store: &mut Store<()>) {
        for _ in 0..8 {
            if store.turn(Waker::noop()).expect("turn") == Outcome::Idle {
                return;
            }
        }
        panic!("the store never went idle");
    }

    /// Poll `future` once, as an executor would.
    fn poll_once<F: Future>(future: &mut Pin<Box<F>>, waker: &Waker) -> Poll<F::Output> {
        let mut context = Context::from_waker(waker);
        future.as_mut().poll(&mut context)
    }

    fn cause(outcome: Result<()>) -> String {
        match outcome {
            Err(error) => error.to_string(),
            Ok(()) => "the seam returned with the condition held".to_owned(),
        }
    }

    #[cfg_attr(not(target_arch = "wasm32"), test)]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    fn it_has_an_empty_provider_slot_on_both_targets() {
        let store = store();

        assert!(
            !store.scheduler.suspend_seam().has_provider(),
            "the capability is filled on neither target, so every suspension \
             takes the nested turn"
        );
    }

    #[cfg_attr(not(target_arch = "wasm32"), test)]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    fn it_polls_a_host_task_with_the_outer_waker_and_returns_when_the_condition_holds() {
        let mut store = store();
        let outer = Waker::from(Arc::new(Outer::default()));
        let (slot, polls) = host_task(&mut store, &outer, 1);

        let watched = slot.clone();
        let outcome = store
            .run_in_turn(&outer, move |store| {
                SuspendSeam::suspend(store, move |_| watched.lock().expect("slot").is_some())
            })
            .expect("the outer turn runs");

        assert_eq!(
            cause(outcome),
            "the seam returned with the condition held",
            "the nested turn ran the host task to its completion"
        );
        assert_eq!(
            polls.lock().expect("polls").clone(),
            vec![true],
            "the nested turn polled the host task with the waker of the outer turn"
        );
        assert!(
            slot.lock().expect("slot").is_some(),
            "the completed future's value reached the slot the condition watches"
        );
        assert_eq!(
            store.scheduler.host_task_count(),
            0,
            "the host task that completed left the store"
        );
    }

    #[cfg_attr(not(target_arch = "wasm32"), test)]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    fn it_runs_ready_work_of_another_task_while_the_condition_is_unmet() {
        let mut store = store();
        let outer = Waker::from(Arc::new(Outer::default()));
        let (slot, _polls) = host_task(&mut store, &outer, 1);

        // The ready work of another task. It records the condition
        // as it saw it, so the test can show that it ran before the
        // condition held rather than after.
        let log = log();
        let seen: Arc<Mutex<Option<bool>>> = Arc::new(Mutex::new(None));
        let recorded = seen.clone();
        let written = log.clone();
        let watched = slot.clone();
        store.scheduler.push_high_priority(Item::new(
            ItemKind::TaskStart,
            move |_store: &mut Store<()>| {
                *recorded.lock().expect("record") = Some(watched.lock().expect("slot").is_some());
                written.lock().expect("log").push("other task");
                Ok(())
            },
        ));

        let watched = slot.clone();
        let outcome = store
            .run_in_turn(&outer, move |store| {
                SuspendSeam::suspend(store, move |_| watched.lock().expect("slot").is_some())
            })
            .expect("the outer turn runs");

        assert_eq!(cause(outcome), "the seam returned with the condition held");
        assert_eq!(
            entries(&log),
            vec!["other task"],
            "the nested turn ran the ready guest work of another task"
        );
        assert_eq!(
            *seen.lock().expect("record"),
            Some(false),
            "it ran while the condition was still unmet"
        );
    }

    #[cfg_attr(not(target_arch = "wasm32"), test)]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    fn it_leaves_a_host_task_it_left_pending_in_the_store_for_the_outer_driver() {
        let mut store = store();
        let outer = Waker::from(Arc::new(Outer::default()));
        // The future is ready on its second poll, so the one poll
        // the nested turn makes leaves it pending.
        let (slot, polls) = host_task(&mut store, &outer, 2);

        // What the nested turn's condition watches is the ready work
        // of another task, not the host task, so the nested turn
        // returns with the host task still pending.
        let released = Arc::new(Mutex::new(false));
        let flag = released.clone();
        store.scheduler.push_high_priority(Item::new(
            ItemKind::TaskStart,
            move |_store: &mut Store<()>| {
                *flag.lock().expect("flag") = true;
                Ok(())
            },
        ));

        let watched = released.clone();
        let outcome = store
            .run_in_turn(&outer, move |store| {
                SuspendSeam::suspend(store, move |_| *watched.lock().expect("flag"))
            })
            .expect("the outer turn runs");

        assert_eq!(cause(outcome), "the seam returned with the condition held");
        assert_eq!(
            polls.lock().expect("polls").clone(),
            vec![true],
            "the nested turn polled the host task once, with the outer waker"
        );
        assert_eq!(
            store.scheduler.host_task_count(),
            1,
            "the host task that stayed pending stayed in the store"
        );
        assert!(
            slot.lock().expect("slot").is_none(),
            "nothing has completed it yet"
        );

        // A later turn of a driver of the same store polls it again.
        let watched = slot.clone();
        let mut driver = Box::pin(Driver::new(&mut store, None, move |_store, _waker| {
            watched.lock().expect("slot").is_some().then(|| Ok(()))
        }));
        let done = poll_once(&mut driver, &outer);

        assert!(
            matches!(done, Poll::Ready(Ok(()))),
            "the host task completed in the outer driver's later turn"
        );
        assert_eq!(
            polls.lock().expect("polls").clone(),
            vec![true, true],
            "the second poll carried the driver's waker too, so no wake was lost"
        );
    }

    #[cfg_attr(not(target_arch = "wasm32"), test)]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    fn it_traps_with_the_cannot_block_cause_when_the_task_must_not_block() {
        let mut store = store();
        current_task(&store, true);

        let outcome = store
            .run_in_turn(Waker::noop(), |store| {
                SuspendSeam::suspend(store, |_| false)
            })
            .expect("the outer turn runs");

        assert_eq!(
            cause(outcome),
            Error::Scheduler(SchedulerCause::CannotBlock).to_string(),
            "the nested turn went idle with the condition unmet, and the \
             reference forbids this task to block"
        );
    }

    #[cfg_attr(not(target_arch = "wasm32"), test)]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    fn it_traps_with_the_stack_switch_cause_when_the_task_may_block() {
        let mut store = store();
        current_task(&store, false);

        let outcome = store
            .run_in_turn(Waker::noop(), |store| {
                SuspendSeam::suspend(store, |_| false)
            })
            .expect("the outer turn runs");

        assert_eq!(
            cause(outcome),
            Error::Scheduler(SchedulerCause::StackSwitchNeeded).to_string(),
            "the reference permits this block, and only the target has no \
             provider to serve it"
        );
    }

    #[cfg_attr(not(target_arch = "wasm32"), test)]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    fn it_does_not_raise_the_recursive_driver_cause_from_inside_a_drivers_turn() {
        let mut store = store();
        let outer = Waker::from(Arc::new(Outer::default()));
        let (slot, polls) = host_task(&mut store, &outer, 1);

        let seen: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let recorded = seen.clone();
        let watched = slot.clone();
        store.scheduler.push_high_priority(Item::new(
            ItemKind::TaskStart,
            move |store: &mut Store<()>| {
                let outcome =
                    SuspendSeam::suspend(store, move |_| watched.lock().expect("slot").is_some());
                *recorded.lock().expect("record") = Some(cause(outcome));
                Ok(())
            },
        ));

        let watched = seen.clone();
        let mut driver = Box::pin(Driver::new(&mut store, None, move |_store, _waker| {
            watched.lock().expect("record").is_some().then(|| Ok(()))
        }));
        let done = poll_once(&mut driver, &outer);

        assert!(
            matches!(done, Poll::Ready(Ok(()))),
            "the driver's turn ran the item that suspended"
        );
        assert_eq!(
            seen.lock().expect("record").clone(),
            Some("the seam returned with the condition held".to_owned()),
            "a nested turn is not a driver, so a driver already inside a turn \
             does not refuse it"
        );
        assert_eq!(
            polls.lock().expect("polls").clone(),
            vec![true],
            "the nested turn polled with the driver's own waker"
        );
    }

    #[cfg_attr(not(target_arch = "wasm32"), test)]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    fn it_consults_the_provider_slot_before_it_falls_back_to_a_nested_turn() {
        let mut store = store();
        let calls = Arc::new(Mutex::new(0usize));
        store
            .scheduler
            .suspend_seam_mut()
            .set_provider(Recorded(calls.clone()));

        // Ready guest work a nested turn would have run.
        let log = log();
        store
            .scheduler
            .push_high_priority(marker(&log, "the nested turn"));

        let outcome = store
            .run_in_turn(Waker::noop(), |store| {
                SuspendSeam::suspend(store, |_| false)
            })
            .expect("the outer turn runs");

        assert_eq!(
            cause(outcome),
            "the seam returned with the condition held",
            "the provider served the suspension"
        );
        assert_eq!(
            *calls.lock().expect("provider calls"),
            1,
            "the seam consulted the provider slot"
        );
        assert!(
            entries(&log).is_empty(),
            "no nested turn ran, so no guest work was entered from the frame \
             that blocked"
        );
        assert_eq!(
            store.scheduler.queued_items(),
            1,
            "the ready item is still queued for a turn of the scheduler"
        );
        assert!(
            store.scheduler.suspend_seam().has_provider(),
            "the provider went back into its slot"
        );
    }

    #[cfg_attr(not(target_arch = "wasm32"), test)]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    fn it_runs_every_deferred_item_exactly_once_after_a_suspension_gave_up() {
        let mut store = store();
        let log = log();
        store.scheduler.push_low_priority(marker(&log, "A"));
        store.scheduler.push_low_priority(marker(&log, "B"));

        // The one item that is ready reaches the seam with a
        // condition nothing will meet, so its nested turn finds
        // nothing but the low-priority queue.
        let seen: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let recorded = seen.clone();
        store.scheduler.push_high_priority(Item::new(
            ItemKind::TaskStart,
            move |store: &mut Store<()>| {
                let outcome = SuspendSeam::suspend(store, |_| false);
                *recorded.lock().expect("record") = Some(cause(outcome));
                Ok(())
            },
        ));

        drain(&mut store);

        assert_eq!(
            seen.lock().expect("record").clone(),
            Some(Error::Scheduler(SchedulerCause::StackSwitchNeeded).to_string()),
            "the nested turn had nothing it was allowed to run, so the \
             suspension gave up"
        );
        assert_eq!(
            entries(&log),
            vec!["A", "B"],
            "every deferred item ran, each exactly once: the nested turn \
             neither took the resume-after-yield slot nor filled it, so the \
             outer turn's own deferral overwrote nothing"
        );
        assert_eq!(store.scheduler.queued_items(), 0, "nothing was left queued");
    }

    #[cfg_attr(not(target_arch = "wasm32"), test)]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    fn it_never_resumes_a_yielded_item_inside_the_guest_call_that_blocked() {
        let mut store = store();
        let log = log();
        store.scheduler.push_low_priority(marker(&log, "yielded"));

        // Both ready items block. The second one reaches the seam
        // from inside the first one's nested turn, which is where a
        // turn nested one deeper used to find the resumption sitting
        // in the resume-after-yield slot and run it.
        store
            .scheduler
            .push_high_priority(blocker(&log, "outer enter", "outer leave"));
        store
            .scheduler
            .push_high_priority(blocker(&log, "inner enter", "inner leave"));

        drain(&mut store);

        assert_eq!(
            entries(&log),
            vec![
                "outer enter",
                "inner enter",
                "inner leave",
                "outer leave",
                "yielded"
            ],
            "the resumption after the yield ran last, outside every guest \
             call that blocked, and only once the driver had returned control \
             to the host executor"
        );
    }

    #[cfg_attr(not(target_arch = "wasm32"), test)]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    fn it_refuses_a_suspension_reached_from_inside_a_nested_turn() {
        let mut store = store();
        let log = log();
        let seen: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

        // The item that blocks first. Its nested turn runs the two
        // items queued behind it.
        store
            .scheduler
            .push_high_priority(blocker(&log, "outer enter", "outer leave"));

        // The item the nested turn runs, which reaches the seam
        // itself and would be a second level.
        let recorded = seen.clone();
        let written = log.clone();
        store.scheduler.push_high_priority(Item::new(
            ItemKind::TaskStart,
            move |store: &mut Store<()>| {
                written.lock().expect("log").push("inner enter");
                let outcome = SuspendSeam::suspend(store, |_| false);
                *recorded.lock().expect("record") = Some(cause(outcome));
                written.lock().expect("log").push("inner leave");
                Ok(())
            },
        ));

        // Ready work behind it. A second level would have run this
        // from inside the inner item's own guest call.
        store.scheduler.push_high_priority(marker(&log, "behind"));

        drain(&mut store);

        assert_eq!(
            seen.lock().expect("record").clone(),
            Some(Error::Scheduler(SchedulerCause::StackSwitchNeeded).to_string()),
            "the fallback is one level deep, so the item the nested turn ran \
             was refused rather than given a nested turn of its own"
        );
        assert_eq!(
            entries(&log),
            vec![
                "outer enter",
                "inner enter",
                "inner leave",
                "behind",
                "outer leave"
            ],
            "the refusal returned at once, and the one nested turn went on to \
             the work behind it rather than running that work one frame deeper"
        );
    }

    #[cfg_attr(not(target_arch = "wasm32"), test)]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    fn it_refuses_a_nested_suspension_with_the_cannot_block_cause_in_a_sync_task() {
        let mut store = store();
        current_task(&store, true);

        let seen: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        store.scheduler.push_high_priority(Item::new(
            ItemKind::TaskStart,
            |store: &mut Store<()>| {
                let _ = SuspendSeam::suspend(store, |_| false);
                Ok(())
            },
        ));
        let recorded = seen.clone();
        store.scheduler.push_high_priority(Item::new(
            ItemKind::TaskStart,
            move |store: &mut Store<()>| {
                let outcome = SuspendSeam::suspend(store, |_| false);
                *recorded.lock().expect("record") = Some(cause(outcome));
                Ok(())
            },
        ));

        drain(&mut store);

        assert_eq!(
            seen.lock().expect("record").clone(),
            Some(Error::Scheduler(SchedulerCause::CannotBlock).to_string()),
            "the refusal takes the cause of the current task, so a task the \
             reference forbids to block still gets the cannot-block trap"
        );
    }

    #[cfg_attr(not(target_arch = "wasm32"), test)]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    fn it_runs_a_nested_turn_again_once_an_earlier_one_has_returned() {
        let mut store = store();
        let outer = Waker::from(Arc::new(Outer::default()));

        let first = store
            .run_in_turn(&outer, |store| SuspendSeam::suspend(store, |_| false))
            .expect("the outer turn runs");

        assert_eq!(
            cause(first),
            Error::Scheduler(SchedulerCause::StackSwitchNeeded).to_string(),
            "nothing could progress, so the first suspension gave up"
        );

        // A host task whose future completes on its first poll, so a
        // second nested turn has something to make the condition
        // hold.
        let (slot, polls) = host_task(&mut store, &outer, 1);
        let watched = slot.clone();
        let second = store
            .run_in_turn(&outer, move |store| {
                SuspendSeam::suspend(store, move |_| watched.lock().expect("slot").is_some())
            })
            .expect("the outer turn runs");

        assert_eq!(
            cause(second),
            "the seam returned with the condition held",
            "the seam released its nested-turn marker when the first \
             suspension returned, so the next one is not refused as a second \
             level"
        );
        assert_eq!(polls.lock().expect("polls").clone(), vec![true]);
    }

    // The two tests below are native only: the browser aborts on a
    // panic instead of unwinding, so there is nothing to catch there
    // and nothing the seam could be left holding.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn it_puts_the_provider_back_when_the_suspension_panicked() {
        let mut store = store();
        store.scheduler.suspend_seam_mut().set_provider(Panics);

        let unwound = unwind(|| SuspendSeam::suspend(&mut store, |_| false));

        assert!(
            unwound.is_err(),
            "the provider's panic unwound the suspension"
        );
        assert!(
            store.scheduler.suspend_seam().has_provider(),
            "the provider went back into its slot, so the target still has the \
             capability it filled"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn it_clears_the_nested_turn_marker_when_a_nested_turn_panicked() {
        let mut store = store();

        let unwound = unwind(|| {
            SuspendSeam::suspend(&mut store, |_store: &mut Store<()>| -> bool {
                panic!("the condition panicked")
            })
        });

        assert!(unwound.is_err(), "the condition's panic unwound the seam");
        assert!(
            !store.scheduler.suspend_seam().in_nested_turn(),
            "the seam is no longer marked as running a nested turn"
        );

        let again = store
            .run_in_turn(Waker::noop(), |store| SuspendSeam::suspend(store, |_| true))
            .expect("the outer turn runs");

        assert_eq!(
            cause(again),
            "the seam returned with the condition held",
            "the next suspension is served rather than refused as a second level"
        );
    }
}
