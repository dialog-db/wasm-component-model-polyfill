//! The scheduler's one suspend capability, and the nested turn it
//! falls back to.

use crate::error::{Error, Result, SchedulerCause};
use crate::store::StoreContext;
use crate::store::StoreContextInternalExt;

use super::SuspendProvider;
use super::outcome::Outcome;
use super::scheduler::SPIN_BUDGET;

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
/// thread until a readiness condition holds, through
/// [`suspend`](Self::suspend). `thread.yield` asks it for one
/// chance to give way instead, through
/// [`give_way`](Self::give_way). The seam is a slot a target fills
/// with a [`SuspendProvider`], and the polyfill fills it on neither
/// target today, so every suspension takes the fallback below.
///
/// The fallback is a nested turn, run from inside the guest call
/// that blocked. It runs the guest work of other tasks that is
/// ready and polls the host tasks the executor woke, with the waker
/// of the outer turn, until the condition holds or nothing can
/// progress. A host task that stays pending inside a nested turn
/// stays in the store for the outer turn, so no wake is lost.
///
/// Five rules hold for the fallback. The first three say what a
/// task that blocks waits for and what it is told when the wait
/// cannot end. The fourth bounds how long any suspension goes on,
/// a yield included. The fifth says what happens when the work the
/// wait runs blocks in its turn.
///
/// - **A yielded item runs inside a nested turn once no other item
///   is ready.** A yield gives way to every other ready item, so
///   the nested turn runs a resumption after a yield only when it
///   has run every other ready item and polled every woken host
///   task. A driver's turn keeps the rule it always had: it defers
///   the resumption, ends, and returns control to the host executor
///   before the item runs. A nested turn has no control to return,
///   and a task waiting on what a yielded item will produce would
///   otherwise wait for ever on work the store was holding back, so
///   the nested turn runs the item where it stands.
/// - **The cause says why the wait cannot end.** When the nested
///   turns cannot progress and the condition is still unmet, the
///   built-in traps. The cause is the cannot-block cause when any
///   instance of the store has a synchronous call in progress,
///   which is the may-not-suspend flag of the instance record: some
///   call has not returned, and the callee blocking for ever is
///   that caller failing to return, so the cause names the caller's
///   rule. It is the stack-switch cause when a host task is still
///   pending — the future of a call that blocked on one of its own
///   included — because the reference permits that block and only
///   the target has no provider to serve it. It is the deadlock
///   cause when the store is idle, because nothing left in the
///   store can ever meet the condition.
/// - **A task that must not block gives way to its own instance
///   alone.** The rule is lazy, as the reference and Wasmtime state
///   it. A task whose instance may not suspend runs the ready work
///   of that instance and nothing else — no item of another
///   instance, and no host task — and the built-in then fails with
///   the cannot-block cause when the condition still does not hold.
///   That is the case of a start function, of a host call into a
///   synchronous export, of a synchronous call between two
///   components, and of a resource destructor. The rule holds
///   whether or not the provider slot is filled: a provider would
///   let the store run on while the task is suspended, which is the
///   block the reference forbids it. A task that is allowed to block
///   runs every ready item and polls every woken host task.
/// - **The seam keeps one budget, and past it the call fails with
///   the stack-switch cause.** This is the polyfill's one departure
///   from the reference, which bounds neither the yielded item the
///   first rule runs nor the number of times a thread gives way.
///   The seam counts the nested turns in a row in which the store
///   did nothing of its own: a turn that ran nothing, or nothing
///   but a resumption after a yield, against a store with no host
///   future that can still resolve. A turn that ran any other item,
///   or that ran against a store whose host future wants another
///   poll, starts the count over. Once the run passes
///   [`SPIN_BUDGET`] the seam gives up and the call the suspended
///   thread is inside fails with
///   [`SchedulerCause::StackSwitchNeeded`], because the one thread
///   that could release it is a guest frame on the real stack that
///   the store cannot reach and only a stack switch could resume.
///   Two shapes reach the budget and they are one shape. A callee
///   that spin-waits in its event loop until its caller unblocks it
///   gives way, is re-queued, runs again and gives way again, and
///   the caller's block runs it every turn. A callee whose core
///   function calls `thread.yield` in a loop against a store that
///   holds nothing asks the seam over and over for what it was
///   refused the time before. Two corpus directives depend on the
///   budget, and without it both run for ever rather than failing.
///   The bound is a budget and not a proof: a yielder that
///   converges after more than [`SPIN_BUDGET`] turns of its own
///   would be cut short by it, which is why the number is drawn
///   generously. A target that fills the provider slot never
///   consults it.
///
///   [`SchedulerCause::StackSwitchNeeded`]: crate::error::SchedulerCause::StackSwitchNeeded
/// - **Nested turns nest.** An item a nested turn runs can block
///   and open a nested turn of its own, one real frame further down
///   the stack. Each level runs what the level above it has not
///   reached, and the guest's own call nesting bounds the depth: a
///   level exists only for an item the level above took out of the
///   store, so the work the store holds is what the depth is drawn
///   from.
///
/// One thing the fallback does not run the same way on both
/// targets. A nested turn runs from inside the lowered import the
/// guest blocked in, so that import's host function is on the stack
/// for as long as the block lasts. An item the turn runs which
/// calls that same import is therefore a second call of the same
/// host function. A native engine serves it. The browser refuses
/// it: a host function there is one JavaScript function object over
/// one Rust closure, and the arguments and results of a call belong
/// to that call alone. The browser backend detects the second call
/// and fails it with [`SchedulerCause::ReentrantHostCall`], so the
/// item traps with a message naming the limitation and the call the
/// item was made from reports the cause. The backend refused the
/// second call before it had a guard of its own, too: `wasm_bindgen`
/// threw a catchable JS exception that named its own mechanism and
/// left nothing for the outer call to read. Neither took the page
/// down; what the guard added is a cause a host can act on. The
/// component itself is sound either way, and the same one runs
/// natively.
///
/// The import the block is inside is not the only one at risk. Every
/// level of nesting leaves its own import's host function on the
/// stack, so an item of a turn three levels down which calls the
/// import a level above it is refused on the same rule. A call to an
/// import no level has entered, or to the same import of another
/// component instance, runs the same way on both targets: each of
/// those is a host function of its own.
///
/// A provider in the slot serves the whole of that instead, for a
/// task that is allowed to block. It suspends the thread and ends
/// the turn, and no nested turn runs.
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
/// turn runs which reaches the seam again gets one of its own.
///
/// [`SchedulerCause::ReentrantHostCall`]: crate::error::SchedulerCause::ReentrantHostCall
pub struct SuspendSeam<T: 'static> {
    provider: Option<BoxedProvider<T>>,
    blocked_call_futures: usize,
    unserved_turns: u32,
    served_mark: (u64, u64),
}

impl<T: 'static> SuspendSeam<T> {
    /// Construct the seam with its provider slot empty, which is
    /// what both targets start with.
    pub fn new() -> Self {
        Self {
            provider: None,
            blocked_call_futures: 0,
            unserved_turns: 0,
            served_mark: (0, 0),
        }
    }

    /// Whether a target has filled the capability.
    pub fn has_provider(&self) -> bool {
        self.provider.is_some()
    }

    /// Whether a call of this store blocked on a host future of its
    /// own: one the store does not hold and no turn can poll, which
    /// only the frame that started it can carry forward.
    pub fn blocked_on_a_call_future(&self) -> bool {
        self.blocked_call_futures > 0
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
    ///
    /// The store it takes is a [`StoreContext`], which is what the
    /// frame a blocking built-in runs in can produce: a host
    /// trampoline is handed the core store's context and nothing
    /// else, and the scheduler rides in that store's data. The seam
    /// is therefore reachable with no driver on the stack, which is
    /// what a provider that resumes a thread outside any poll of a
    /// driver needs.
    pub fn suspend(
        store: &mut StoreContext<'_, T>,
        mut condition: impl FnMut(&mut StoreContext<'_, T>) -> bool,
    ) -> Result<()> {
        // The provider slot is consulted first. When a target fills
        // the capability the thread suspends there, and no guest
        // code is entered from the trampoline this runs in — which
        // is what a provider that switches stacks needs. The nested
        // turn runs only when the slot is empty or the task must not
        // block, so the two never meet.
        //
        // A task that must not block never reaches the provider. A
        // provider suspends the thread and lets the store run on,
        // which is the block the reference forbids such a task; the
        // nested turn gives way to the task's own instance alone and
        // then fails with the cannot-block cause, which is the whole
        // of what the reference allows it.
        if Self::provider_serves(store) {
            return Self::suspend_with_provider(store, &mut condition);
        }
        Self::run_nested_turns(store, &mut condition)
    }

    /// Give way once, which is the whole of what `thread.yield`
    /// asks of the seam.
    ///
    /// A yield waits for one chance to be given back control and
    /// for nothing else, so this is not a wait with a condition. A
    /// target with a provider suspends the thread once and resumes
    /// it, unless the task must not block. Otherwise the seam runs
    /// exactly one nested turn: the ready work of the store, or of
    /// the calling task's own instance alone when that task must not
    /// block. A task that must not block with no ready work of its
    /// own instance gives way to nothing, as Wasmtime runs it.
    ///
    /// The seam's budget is the one thing that can fail this. A
    /// thread whose turns the store has not served past
    /// [`SPIN_BUDGET`] times over is spin-waiting for a guest frame
    /// on the real stack, and the failure is the stack-switch cause
    /// that ends the call the thread is inside. Everything else
    /// answers `Ok(())`, which is what makes the built-in return
    /// zero whenever it returns at all.
    pub fn give_way(store: &mut StoreContext<'_, T>) -> Result<()> {
        if Self::provider_serves(store) {
            let mut given_back = false;
            return Self::suspend_with_provider(store, &mut |_| {
                std::mem::replace(&mut given_back, true)
            });
        }
        let waker = store.internal().active_waker();
        let only = store.internal().must_not_block_instance();
        store.internal().nested_turn(&waker, only)?;
        if Self::note_turn(store) {
            return Err(Error::Scheduler(SchedulerCause::StackSwitchNeeded));
        }
        Ok(())
    }

    /// Whether the target's provider serves a suspension of the
    /// current task: the slot is filled and the task is allowed to
    /// block. A task whose instance may not suspend — a synchronous
    /// call that has not returned, a start function, or a resource
    /// destructor — takes the nested turn whatever the slot holds.
    fn provider_serves(store: &mut StoreContext<'_, T>) -> bool {
        store.internal().scheduler().suspend_seam().has_provider()
            && store.internal().must_not_block_instance().is_none()
    }

    /// Hand the suspension to the target's provider. The provider
    /// leaves the slot for the duration of the call, because it runs
    /// against the store the slot sits in, and goes back into it
    /// afterwards — whether the call returned or unwound. A provider
    /// a panic swallowed would leave the seam with an empty slot for
    /// the life of the store, and every later suspension would take
    /// the nested-turn fallback on a target that had a provider.
    fn suspend_with_provider(
        store: &mut StoreContext<'_, T>,
        condition: &mut dyn FnMut(&mut StoreContext<'_, T>) -> bool,
    ) -> Result<()> {
        let Some(mut provider) = store
            .internal()
            .scheduler_mut()
            .suspend_seam_mut()
            .provider
            .take()
        else {
            return Self::run_nested_turns(store, condition);
        };
        // `provider` stays in this frame. The closure only borrows
        // it, so an unwind through the call leaves it here to put
        // back rather than dropping it inside the closure.
        let outcome = Self::caught(|| provider.suspend(&mut *store, condition));
        store.internal().scheduler_mut().suspend_seam_mut().provider = Some(provider);
        Self::resume(outcome)
    }

    /// Run `body` with the store marked as holding a call that
    /// blocked on a host future of its own.
    ///
    /// Such a future stays in the frame that started it rather than
    /// joining the store's host tasks, because the call it belongs
    /// to is still on the guest's stack. Without the mark the store
    /// would look idle while a future that can still resolve is
    /// pending, and a block that gave up would name the deadlock
    /// cause where the stack-switch cause is the true one. The mark
    /// is what the cause of a suspension reads to tell the two
    /// apart.
    ///
    /// The mark comes back off through an unwind, as the provider
    /// does: one a panic left raised would turn every later
    /// deadlock of the store into a stack switch.
    pub fn while_blocked_on_a_call_future<R>(
        store: &mut StoreContext<'_, T>,
        body: impl FnOnce(&mut StoreContext<'_, T>) -> R,
    ) -> R {
        store
            .internal()
            .scheduler_mut()
            .suspend_seam_mut()
            .blocked_call_futures += 1;
        let outcome = Self::caught(|| body(&mut *store));
        let seam = store.internal().scheduler_mut().suspend_seam_mut();
        seam.blocked_call_futures = seam.blocked_call_futures.saturating_sub(1);
        Self::resume(outcome)
    }

    /// The fallback: turns of the store's scheduler run from inside
    /// the guest call that blocked, until the condition holds or
    /// nothing can progress.
    ///
    /// Nothing marks the seam as running one. Nested turns nest: an
    /// item this loop runs that reaches the seam again gets a loop
    /// of its own, one real frame further down the stack, and the
    /// work the store holds is what bounds the depth.
    fn run_nested_turns(
        store: &mut StoreContext<'_, T>,
        condition: &mut dyn FnMut(&mut StoreContext<'_, T>) -> bool,
    ) -> Result<()> {
        // The waker of the outer turn, so that a wake of a host task
        // polled here reaches the waker the executor already holds.
        // There is none when no turn is running — a thread resumed
        // outside any poll of a driver — and a waker that does
        // nothing serves instead, as it does for a trampoline that
        // starts a host task outside a turn.
        let waker = store.internal().active_waker();
        // A task that must not block gives way only to the ready
        // work of its own instance. The instance is read once: what
        // the turn is allowed to run cannot change under it, because
        // the flag is set for the length of the call this thread is
        // inside.
        let only = store.internal().must_not_block_instance();
        // Whether the seam's budget is what ended the loop. The
        // budget is the run of turns the store did not serve, and
        // it is kept on the seam rather than here, so that a thread
        // which asks again and again in separate frames — a yield
        // loop — is one run and not a fresh one every time.
        let past_budget;
        loop {
            if condition(store) {
                return Ok(());
            }
            let outcome = store.internal().nested_turn(&waker, only)?;
            let noted = Self::note_turn(store);
            match outcome {
                Outcome::Progress if !noted => continue,
                // Nothing more can progress from inside the guest
                // call. `Waiting` leaves its host tasks in the store
                // for the outer turn to poll again. `Yield` is the
                // answer a driver's turn gives for a resumption it
                // deferred, and a nested turn gives it for nothing:
                // it runs that resumption itself and reports
                // progress. Ending the loop is what it would mean
                // here all the same, since a turn that ran nothing
                // and deferred nothing has nothing left to offer.
                //
                // Progress ends it too once the run of turns the
                // store did not serve has gone past the budget. A
                // callee that spin-waits in its event loop until its
                // caller unblocks it re-queues itself every time it
                // runs, and this block is that caller, so no number
                // of further turns would change anything; the block
                // has to reach its failure rather than run for ever.
                // The budget is what the polyfill spends before it
                // decides that is what it is looking at, and
                // [`SPIN_BUDGET`] says why it is a budget rather
                // than a proof.
                Outcome::Progress | Outcome::Yield | Outcome::Waiting | Outcome::Idle => {
                    past_budget = noted;
                    break;
                }
            }
        }
        // A last turn that met the condition is the wait ending,
        // whatever the budget stands at: the block got what it was
        // waiting for and the call goes on.
        if condition(store) {
            return Ok(());
        }
        if past_budget {
            return Err(Error::Scheduler(SchedulerCause::StackSwitchNeeded));
        }
        Err(Error::Scheduler(store.internal().suspend_cause()))
    }

    /// Record what the store did for the suspended thread over the
    /// nested turn that just ran, and answer whether the run of
    /// turns the store did not serve has passed [`SPIN_BUDGET`].
    ///
    /// The store served the thread when it ran an item that was not
    /// a resumption after a yield, or when it still holds a host
    /// future that can resolve on a later poll. Either says the
    /// store can get somewhere on its own. A turn that ran nothing,
    /// or nothing but a resumption the yield rule was holding back,
    /// against a store that holds no such future did not: the
    /// thread is asking again for what it was refused the time
    /// before.
    ///
    /// What the store ran is measured from the last turn the seam
    /// noted, not from the top of this suspension. The run is
    /// therefore one run across every suspension of the store: a
    /// thread that gives way in one frame after another builds a
    /// single run, and whatever the store ran in between — a
    /// driver's turn included — ends it.
    fn note_turn(store: &mut StoreContext<'_, T>) -> bool {
        let items_run = store.internal().scheduler().items_run();
        let resumptions = store.internal().scheduler().resumptions();
        let pending = store.internal().scheduler().host_future_pending();
        let seam = store.internal().scheduler_mut().suspend_seam_mut();
        let ran = items_run - seam.served_mark.0;
        let resumed = resumptions - seam.served_mark.1;
        seam.served_mark = (items_run, resumptions);
        seam.unserved_turns = if ran > resumed || pending {
            0
        } else {
            seam.unserved_turns.saturating_add(1)
        };
        seam.unserved_turns > SPIN_BUDGET
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
}

impl<T: 'static> Default for SuspendSeam<T> {
    fn default() -> Self {
        Self::new()
    }
}

// The seam is one code path on both targets, and every test here
// measures it on both, under the crate's cross-target test attribute.
// None of these bodies awaits anything — a nested turn is a
// synchronous call from inside a guest call — so they take the
// attribute's synchronous arm. A plain `#[test]` would run natively
// only, since the browser runner collects `wasm_bindgen_test`
// functions.
#[cfg(test)]
mod tests {
    use core::future::Future;
    use core::pin::Pin;
    use core::task::{Context, Poll, Waker};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use wcmp_macros::component;

    use crate::component::Component;
    use crate::engine::Engine;
    use crate::error::SchedulerCause;
    use crate::linker::{HostCall, Linker};
    use crate::store::Store;
    use crate::value::Val;

    use super::super::driver::Driver;
    use super::super::host_task::HostTask;
    use super::super::instance_id::InstanceId;
    use super::super::item::Item;
    use super::super::item_kind::ItemKind;

    use super::*;
    use crate::internal::ErrorInternal;
    use crate::store::StoreInternalExt;

    /// What the items of one test wrote as they ran, in order.
    type Log = Arc<Mutex<Vec<&'static str>>>;

    /// Where a test's host task leaves what its body produced. The
    /// lowering is what fills it, and a lowering runs as a queued
    /// item in a later turn than the poll that saw the body
    /// complete, so the slot is what every test here watches as its
    /// readiness condition.
    type Slot = Arc<Mutex<Option<Result<Vec<Val>>>>>;

    /// Whether a wake of the waker each poll of a test's host task was
    /// handed reached the waker of the outer turn.
    type Polls = Arc<Mutex<Vec<bool>>>;

    /// The waker of an outer turn, which counts the wakes it receives,
    /// so that a test can tell whether a wake sent to the waker a
    /// host task was polled with reached the outer turn.
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

    /// The waker of an outer turn, with the count behind it.
    fn outer_waker() -> (Arc<Outer>, Waker) {
        let outer = Arc::new(Outer::default());
        let waker = Waker::from(outer.clone());
        (outer, waker)
    }

    /// A host task's future that records, for every poll, whether a
    /// wake of the waker it was polled with reaches the waker the
    /// outer turn holds, and completes on its `ready_on`th poll. It
    /// wakes that waker on every poll to find out, which is also what
    /// asks for the poll after a pending one.
    struct Probe {
        outer: Arc<Outer>,
        ready_on: usize,
        polls: Polls,
    }

    impl Future for Probe {
        type Output = Result<Vec<Val>>;

        fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
            let this = self.get_mut();
            let before = this.outer.0.load(Ordering::Relaxed);
            context.waker().wake_by_ref();
            let matched = this.outer.0.load(Ordering::Relaxed) > before;
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
            _store: &mut StoreContext<'_, ()>,
            _condition: &mut dyn FnMut(&mut StoreContext<'_, ()>) -> bool,
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
            _store: &mut StoreContext<'_, ()>,
            _condition: &mut dyn FnMut(&mut StoreContext<'_, ()>) -> bool,
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

    /// An item that records that it ran, reaches the seam with a
    /// condition nothing will ever meet, and records that its call
    /// returned. The pair of entries brackets everything the
    /// suspension ran from inside this item's guest call.
    fn blocker(log: &Log, enter: &'static str, leave: &'static str) -> Item<()> {
        let log = log.clone();
        Item::new(
            ItemKind::TaskStart,
            move |store: &mut StoreContext<'_, ()>| {
                log.lock().expect("log").push(enter);
                let _ = SuspendSeam::suspend(store, |_| false);
                log.lock().expect("log").push(leave);
                Ok(())
            },
        )
    }

    /// An item that records that it ran, blocks until `flag` is
    /// set, records what the suspension reported, and records that
    /// its call returned. The pair of log entries brackets
    /// everything the suspension ran, and `flag` is what an item
    /// the suspension runs sets.
    fn waiter(
        log: &Log,
        enter: &'static str,
        leave: &'static str,
        flag: &Arc<Mutex<bool>>,
        reported: &Arc<Mutex<Option<String>>>,
    ) -> Item<()> {
        let log = log.clone();
        let flag = flag.clone();
        let reported = reported.clone();
        Item::new(
            ItemKind::TaskStart,
            move |store: &mut StoreContext<'_, ()>| {
                log.lock().expect("log").push(enter);
                let watched = flag.clone();
                let outcome = SuspendSeam::suspend(store, move |_| *watched.lock().expect("flag"));
                *reported.lock().expect("record") = Some(cause(outcome));
                log.lock().expect("log").push(leave);
                Ok(())
            },
        )
    }

    /// An item that sets `flag`, which is what a waiter above
    /// blocked on, and records that it ran.
    fn releases(log: &Log, name: &'static str, flag: &Arc<Mutex<bool>>) -> Item<()> {
        let log = log.clone();
        let flag = flag.clone();
        Item::new(
            ItemKind::TaskStart,
            move |_store: &mut StoreContext<'_, ()>| {
                *flag.lock().expect("flag") = true;
                log.lock().expect("log").push(name);
                Ok(())
            },
        )
    }

    /// Give `store` a host task whose body completes on its
    /// `ready_on`th poll. The slot it returns is the one the
    /// lowering of that body fills, and it is what every test here
    /// uses as its readiness condition.
    fn host_task(
        store: &mut StoreContext<'_, ()>,
        outer: &Arc<Outer>,
        ready_on: usize,
    ) -> (Slot, Polls) {
        let subtask = store
            .internal()
            .tables()
            .lock()
            .expect("tables")
            .tasks
            .insert_subtask();
        let slot: Slot = Arc::new(Mutex::new(None));
        let polls: Polls = Arc::new(Mutex::new(Vec::new()));
        let filled = slot.clone();
        store.internal().push_host_task(HostTask::from_future(
            subtask,
            move |_store: &mut StoreContext<'_, ()>, outcome: Result<Vec<Val>>| {
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
    /// blocking built-in would find it, and report that instance.
    /// `may_not_suspend` is the instance flag an adapter's enter
    /// intrinsic sets for the duration of a synchronous call.
    fn current_task(store: &StoreContext<'_, ()>, may_not_suspend: bool) -> InstanceId {
        let mut guard = store.internal_ref().tables().lock().expect("tables");
        let instance = guard.tasks.insert_instance();
        guard
            .tasks
            .instance_mut(instance)
            .expect("instance record")
            .may_not_suspend = may_not_suspend;
        let task = guard.tasks.create_task(None, None, instance);
        guard.tasks.push_task_scope(task);
        instance
    }

    /// Queue `item` as ready work of `instance`, the way the entry
    /// gate queues the start of a task's implicit thread. That is
    /// where an item learns which instance's work it is.
    fn queue(store: &mut StoreContext<'_, ()>, instance: InstanceId, item: Item<()>) {
        let task = store
            .internal()
            .tables()
            .lock()
            .expect("tables")
            .tasks
            .create_task(None, None, instance);
        store
            .internal()
            .start_export_thread(task, instance, false, true, item)
            .expect("queue the item");
    }

    /// Run turns of `store` until it goes idle, as a driver that
    /// loops on `Progress` and returns control to the host executor
    /// on `Yield` would. The bound is there so a store that would
    /// never settle fails the test rather than hanging it.
    fn drain(store: &mut StoreContext<'_, ()>) {
        for _ in 0..8 {
            if store.internal().turn(Waker::noop()).expect("turn") == Outcome::Idle {
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

    #[wcmp_macros::test]
    fn it_has_an_empty_provider_slot_on_both_targets() {
        let mut owner = store();
        let mut store = owner.internal().context();

        assert!(
            !store.internal().scheduler().suspend_seam().has_provider(),
            "the capability is filled on neither target, so every suspension \
             takes the nested turn"
        );
    }

    #[wcmp_macros::test]
    fn it_polls_a_host_task_with_the_outer_waker_and_returns_when_the_condition_holds() {
        let mut owner = store();
        let mut store = owner.internal().context();
        let (reached, outer) = outer_waker();
        let (slot, polls) = host_task(&mut store, &reached, 1);

        let watched = slot.clone();
        let outcome = store
            .internal()
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
            "the nested turn polled the host task with a waker that wakes the outer turn's"
        );
        assert!(
            slot.lock().expect("slot").is_some(),
            "the completed future's value reached the slot the condition watches"
        );
        assert_eq!(
            store.internal().scheduler().host_task_count(),
            0,
            "the host task that completed left the store"
        );
    }

    #[wcmp_macros::test]
    fn it_runs_ready_work_of_another_task_while_the_condition_is_unmet() {
        let mut owner = store();
        let mut store = owner.internal().context();
        let (reached, outer) = outer_waker();
        let (slot, _polls) = host_task(&mut store, &reached, 1);

        // The ready work of another task. It records the condition
        // as it saw it, so the test can show that it ran before the
        // condition held rather than after.
        let log = log();
        let seen: Arc<Mutex<Option<bool>>> = Arc::new(Mutex::new(None));
        let recorded = seen.clone();
        let written = log.clone();
        let watched = slot.clone();
        store
            .internal()
            .scheduler_mut()
            .push_high_priority(Item::new(
                ItemKind::TaskStart,
                move |_store: &mut StoreContext<'_, ()>| {
                    *recorded.lock().expect("record") =
                        Some(watched.lock().expect("slot").is_some());
                    written.lock().expect("log").push("other task");
                    Ok(())
                },
            ));

        let watched = slot.clone();
        let outcome = store
            .internal()
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

    #[wcmp_macros::test]
    fn it_leaves_a_host_task_it_left_pending_in_the_store_for_the_outer_driver() {
        let mut owner = store();
        let mut store = owner.internal().context();
        let (reached, outer) = outer_waker();
        // The future is ready on its second poll, so the one poll
        // the nested turn makes leaves it pending.
        let (slot, polls) = host_task(&mut store, &reached, 2);

        // What the nested turn's condition watches is the ready work
        // of another task, not the host task, so the nested turn
        // returns with the host task still pending.
        let released = Arc::new(Mutex::new(false));
        let flag = released.clone();
        store
            .internal()
            .scheduler_mut()
            .push_high_priority(Item::new(
                ItemKind::TaskStart,
                move |_store: &mut StoreContext<'_, ()>| {
                    *flag.lock().expect("flag") = true;
                    Ok(())
                },
            ));

        let watched = released.clone();
        let outcome = store
            .internal()
            .run_in_turn(&outer, move |store| {
                SuspendSeam::suspend(store, move |_| *watched.lock().expect("flag"))
            })
            .expect("the outer turn runs");

        assert_eq!(cause(outcome), "the seam returned with the condition held");
        assert_eq!(
            polls.lock().expect("polls").clone(),
            vec![true],
            "the nested turn polled the host task once, with a waker that wakes the outer one"
        );
        assert_eq!(
            store.internal().scheduler().host_task_count(),
            1,
            "the host task that stayed pending stayed in the store"
        );
        assert!(
            slot.lock().expect("slot").is_none(),
            "nothing has completed it yet"
        );

        // A later turn of a driver of the same store polls it again.
        let watched = slot.clone();
        let mut driver = Box::pin(Driver::new(
            store.internal().reborrow(),
            None,
            move |_store, _waker| watched.lock().expect("slot").is_some().then(|| Ok(())),
        ));
        let done = poll_once(&mut driver, &outer);

        assert!(
            matches!(done, Poll::Ready(Ok(()))),
            "the host task completed in the outer driver's later turn"
        );
        assert_eq!(
            polls.lock().expect("polls").clone(),
            vec![true, true],
            "the second poll carried a waker that wakes the driver's too, so no wake was lost"
        );
    }

    #[wcmp_macros::test]
    fn it_traps_with_the_cannot_block_cause_when_the_task_must_not_block() {
        let mut owner = store();
        let mut store = owner.internal().context();
        current_task(&store, true);

        let outcome = store
            .internal()
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

    #[wcmp_macros::test]
    fn it_runs_only_the_ready_work_of_its_own_instance_before_it_cannot_block() {
        let mut owner = store();
        let mut store = owner.internal().context();
        let log = log();

        let mine = current_task(&store, true);
        let other = store
            .internal()
            .tables()
            .lock()
            .expect("tables")
            .tasks
            .insert_instance();
        queue(&mut store, mine, marker(&log, "mine"));
        queue(&mut store, other, marker(&log, "other"));

        let outcome = store
            .internal()
            .run_in_turn(Waker::noop(), |store| {
                SuspendSeam::suspend(store, |_| false)
            })
            .expect("the outer turn runs");

        assert_eq!(
            cause(outcome),
            Error::Scheduler(SchedulerCause::CannotBlock).to_string(),
            "the ready work of the blocking task's own instance did not meet \
             the condition, and the reference forbids this task to block"
        );
        assert_eq!(
            entries(&log),
            vec!["mine"],
            "the rule is lazy: the task gave way to the ready work of its own \
             instance, and to nothing else"
        );
        assert_eq!(
            store.internal().scheduler().queued_items(),
            1,
            "the other instance's item is still queued for a turn of the \
             scheduler"
        );
    }

    #[wcmp_macros::test]
    fn it_traps_with_the_deadlock_cause_when_the_store_goes_idle() {
        let mut owner = store();
        let mut store = owner.internal().context();
        current_task(&store, false);

        let outcome = store
            .internal()
            .run_in_turn(Waker::noop(), |store| {
                SuspendSeam::suspend(store, |_| false)
            })
            .expect("the outer turn runs");

        assert_eq!(
            cause(outcome),
            Error::Scheduler(SchedulerCause::Deadlock).to_string(),
            "this task may block, and nothing left in the store can ever meet \
             the condition it blocked on"
        );
    }

    #[wcmp_macros::test]
    fn it_traps_with_the_stack_switch_cause_while_a_host_task_is_still_pending() {
        let mut owner = store();
        let mut store = owner.internal().context();
        let (reached, outer) = outer_waker();
        current_task(&store, false);
        // A body that never completes, so the nested turn polls it,
        // leaves it in the store, and gives up with the condition
        // unmet.
        let (_slot, polls) = host_task(&mut store, &reached, usize::MAX);

        let outcome = store
            .internal()
            .run_in_turn(&outer, |store| SuspendSeam::suspend(store, |_| false))
            .expect("the outer turn runs");

        assert_eq!(
            cause(outcome),
            Error::Scheduler(SchedulerCause::StackSwitchNeeded).to_string(),
            "the store still holds a host task, so the reference permits this \
             block and only the target has no provider to serve it"
        );
        assert_eq!(
            polls.lock().expect("polls").clone(),
            vec![true],
            "the nested turn polled the pending body once, with a waker that wakes the outer one"
        );
        assert_eq!(
            store.internal().scheduler().host_task_count(),
            1,
            "the host task that stayed pending stayed in the store"
        );
    }

    #[wcmp_macros::test]
    fn it_does_not_raise_the_recursive_driver_cause_from_inside_a_drivers_turn() {
        let mut owner = store();
        let mut store = owner.internal().context();
        let (reached, outer) = outer_waker();
        let (slot, polls) = host_task(&mut store, &reached, 1);

        let seen: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let recorded = seen.clone();
        let watched = slot.clone();
        store
            .internal()
            .scheduler_mut()
            .push_high_priority(Item::new(
                ItemKind::TaskStart,
                move |store: &mut StoreContext<'_, ()>| {
                    let outcome = SuspendSeam::suspend(store, move |_| {
                        watched.lock().expect("slot").is_some()
                    });
                    *recorded.lock().expect("record") = Some(cause(outcome));
                    Ok(())
                },
            ));

        let watched = seen.clone();
        let mut driver = Box::pin(Driver::new(
            store.internal().reborrow(),
            None,
            move |_store, _waker| watched.lock().expect("record").is_some().then(|| Ok(())),
        ));
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
            "the nested turn polled with a waker that wakes the driver's own"
        );
    }

    #[wcmp_macros::test]
    fn it_consults_the_provider_slot_before_it_falls_back_to_a_nested_turn() {
        let mut owner = store();
        let mut store = owner.internal().context();
        let calls = Arc::new(Mutex::new(0usize));
        store
            .internal()
            .scheduler_mut()
            .suspend_seam_mut()
            .set_provider(Recorded(calls.clone()));

        // Ready guest work a nested turn would have run.
        let log = log();
        store
            .internal()
            .scheduler_mut()
            .push_high_priority(marker(&log, "the nested turn"));

        let outcome = store
            .internal()
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
            store.internal().scheduler().queued_items(),
            1,
            "the ready item is still queued for a turn of the scheduler"
        );
        assert!(
            store.internal().scheduler().suspend_seam().has_provider(),
            "the provider went back into its slot"
        );
    }

    #[wcmp_macros::test]
    fn it_keeps_a_task_that_must_not_block_away_from_the_provider() {
        let mut owner = store();
        let mut store = owner.internal().context();
        let calls = Arc::new(Mutex::new(0usize));
        store
            .internal()
            .scheduler_mut()
            .suspend_seam_mut()
            .set_provider(Recorded(calls.clone()));
        current_task(&store, true);

        let outcome = store
            .internal()
            .run_in_turn(Waker::noop(), |store| {
                SuspendSeam::suspend(store, |_| false)
            })
            .expect("the outer turn runs");
        let yielded = store
            .internal()
            .run_in_turn(Waker::noop(), SuspendSeam::give_way)
            .expect("the outer turn runs");

        assert_eq!(
            cause(outcome),
            Error::Scheduler(SchedulerCause::CannotBlock).to_string(),
            "a provider would have let the task block, which the reference \
             forbids it, so the block took the nested turn and failed there"
        );
        assert!(
            yielded.is_ok(),
            "the yield gave way to nothing and returned"
        );
        assert_eq!(
            *calls.lock().expect("provider calls"),
            0,
            "the seam consulted the provider for neither the block nor the \
             yield"
        );
        assert!(
            store.internal().scheduler().suspend_seam().has_provider(),
            "the provider is still in its slot"
        );
    }

    #[wcmp_macros::test]
    fn it_runs_every_deferred_item_exactly_once_inside_a_nested_turn() {
        let mut owner = store();
        let mut store = owner.internal().context();
        let log = log();
        store
            .internal()
            .scheduler_mut()
            .push_low_priority(marker(&log, "A"));
        store
            .internal()
            .scheduler_mut()
            .push_low_priority(marker(&log, "B"));

        // The one item that is ready reaches the seam with a
        // condition nothing will meet, so its nested turn finds
        // nothing but the low-priority queue.
        let seen: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let recorded = seen.clone();
        store
            .internal()
            .scheduler_mut()
            .push_high_priority(Item::new(
                ItemKind::TaskStart,
                move |store: &mut StoreContext<'_, ()>| {
                    let outcome = SuspendSeam::suspend(store, |_| false);
                    *recorded.lock().expect("record") = Some(cause(outcome));
                    Ok(())
                },
            ));

        drain(&mut store);

        assert_eq!(
            seen.lock().expect("record").clone(),
            Some(Error::Scheduler(SchedulerCause::Deadlock).to_string()),
            "the nested turn ran the deferred work and the store then went \
             idle, so nothing left in it could ever meet the condition"
        );
        assert_eq!(
            entries(&log),
            vec!["A", "B"],
            "every deferred item ran, each exactly once: the nested turn took \
             them one at a time, in the order they were queued, and left \
             nothing for the outer turn to run twice"
        );
        assert_eq!(
            store.internal().scheduler().queued_items(),
            0,
            "nothing was left queued"
        );
    }

    #[wcmp_macros::test]
    fn it_runs_a_yielded_item_inside_a_nested_turn_once_nothing_else_is_ready() {
        let mut owner = store();
        let mut store = owner.internal().context();
        let log = log();
        store
            .internal()
            .scheduler_mut()
            .push_low_priority(marker(&log, "yielded"));

        // Both ready items block. The second one reaches the seam
        // from inside the first one's nested turn, and the
        // resumption after the yield is what is left once it has.
        store.internal().scheduler_mut().push_high_priority(blocker(
            &log,
            "outer enter",
            "outer leave",
        ));
        store.internal().scheduler_mut().push_high_priority(blocker(
            &log,
            "inner enter",
            "inner leave",
        ));

        drain(&mut store);

        assert_eq!(
            entries(&log),
            vec![
                "outer enter",
                "inner enter",
                "yielded",
                "inner leave",
                "outer leave"
            ],
            "the resumption after the yield gave way to every other ready \
             item and then ran inside the innermost nested turn, rather than \
             waiting for a turn of the driver that no blocked call can reach"
        );
    }

    #[wcmp_macros::test]
    fn it_leaves_a_yielded_item_to_the_executor_in_a_drivers_turn() {
        let mut owner = store();
        let mut store = owner.internal().context();
        let log = log();
        store
            .internal()
            .scheduler_mut()
            .push_low_priority(marker(&log, "yielded"));

        let mut driver = Box::pin(Driver::new(
            store.internal().reborrow(),
            None,
            |_store: &mut StoreContext<'_, ()>, _waker: &Waker| -> Option<Result<()>> { None },
        ));
        let outcome = poll_once(&mut driver, Waker::noop());

        assert!(
            outcome.is_pending(),
            "the turn deferred the resumption and ended, so the driver \
             returns pending"
        );
        assert!(
            entries(&log).is_empty(),
            "a driver's turn returns control to the host executor before a \
             yielded item runs, which is the half of the yield rule a nested \
             turn cannot keep"
        );
    }

    #[wcmp_macros::test]
    fn it_opens_another_nested_turn_for_an_item_a_nested_turn_ran() {
        let mut owner = store();
        let mut store = owner.internal().context();
        let log = log();
        let outer_flag = Arc::new(Mutex::new(false));
        let inner_flag = Arc::new(Mutex::new(false));
        let outer_seen: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let inner_seen: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

        // The item that blocks first. Its nested turn runs the item
        // behind it, which blocks in its turn.
        store.internal().scheduler_mut().push_high_priority(waiter(
            &log,
            "outer enter",
            "outer leave",
            &outer_flag,
            &outer_seen,
        ));
        store.internal().scheduler_mut().push_high_priority(waiter(
            &log,
            "inner enter",
            "inner leave",
            &inner_flag,
            &inner_seen,
        ));

        // What each of the two conditions waits for. The second
        // level reaches the first of these, and the first level
        // reaches the second once the level under it has returned.
        store
            .internal()
            .scheduler_mut()
            .push_high_priority(releases(&log, "frees the inner", &inner_flag));
        store
            .internal()
            .scheduler_mut()
            .push_high_priority(releases(&log, "frees the outer", &outer_flag));

        drain(&mut store);

        assert_eq!(
            inner_seen.lock().expect("record").clone(),
            Some("the seam returned with the condition held".to_owned()),
            "the item the first nested turn ran blocked and was given a \
             nested turn of its own, which ran the work that met its \
             condition"
        );
        assert_eq!(
            outer_seen.lock().expect("record").clone(),
            Some("the seam returned with the condition held".to_owned()),
            "the first suspension returned once the level under it had \
             returned and its own condition was met"
        );
        assert_eq!(
            entries(&log),
            vec![
                "outer enter",
                "inner enter",
                "frees the inner",
                "frees the outer",
                "inner leave",
                "outer leave"
            ],
            "the second level ran the work the first had not reached — a turn \
             runs every ready item before its outcome is read — and the two \
             suspensions returned innermost first, as the real frames they \
             sit on unwind"
        );
    }

    #[wcmp_macros::test]
    fn it_fails_a_suspension_inside_a_nested_turn_of_a_sync_task_with_the_cannot_block_cause() {
        let mut owner = store();
        let mut store = owner.internal().context();
        let mine = current_task(&store, true);

        // Two items of the instance whose call must return. The
        // first blocks, its nested turn runs the second, and the
        // second blocks from inside that turn.
        let seen: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        queue(
            &mut store,
            mine,
            Item::new(ItemKind::TaskStart, |store: &mut StoreContext<'_, ()>| {
                let _ = SuspendSeam::suspend(store, |_| false);
                Ok(())
            }),
        );
        let recorded = seen.clone();
        queue(
            &mut store,
            mine,
            Item::new(
                ItemKind::TaskStart,
                move |store: &mut StoreContext<'_, ()>| {
                    let outcome = SuspendSeam::suspend(store, |_| false);
                    *recorded.lock().expect("record") = Some(cause(outcome));
                    Ok(())
                },
            ),
        );

        drain(&mut store);

        assert_eq!(
            seen.lock().expect("record").clone(),
            Some(Error::Scheduler(SchedulerCause::CannotBlock).to_string()),
            "a call of the instance has not returned, so a block that cannot \
             progress names the caller's rule however deep the nesting is"
        );
    }

    #[wcmp_macros::test]
    fn it_traps_with_the_cannot_block_cause_while_another_instance_is_inside_a_synchronous_call() {
        let mut owner = store();
        let mut store = owner.internal().context();
        // The task that blocks is one the reference allows to
        // block, and the store is idle. Another instance is inside
        // a synchronous call all the same.
        current_task(&store, false);
        let mut guard = store.internal().tables().lock().expect("tables");
        let other = guard.tasks.insert_instance();
        guard
            .tasks
            .instance_mut(other)
            .expect("instance record")
            .may_not_suspend = true;
        drop(guard);

        let outcome = store
            .internal()
            .run_in_turn(Waker::noop(), |store| {
                SuspendSeam::suspend(store, |_| false)
            })
            .expect("the outer turn runs");

        assert_eq!(
            cause(outcome),
            Error::Scheduler(SchedulerCause::CannotBlock).to_string(),
            "the flag of every instance is read, not the blocked task's \
             alone: the callee blocking for ever is the caller failing to \
             return, so the cause names the caller's rule"
        );
    }

    #[wcmp_macros::test]
    fn it_runs_the_ready_work_of_every_instance_when_the_task_may_block() {
        let mut owner = store();
        let mut store = owner.internal().context();
        let log = log();

        let mine = current_task(&store, false);
        let other = store
            .internal()
            .tables()
            .lock()
            .expect("tables")
            .tasks
            .insert_instance();
        queue(&mut store, mine, marker(&log, "mine"));
        queue(&mut store, other, marker(&log, "other"));

        let outcome = store
            .internal()
            .run_in_turn(Waker::noop(), |store| {
                SuspendSeam::suspend(store, |_| false)
            })
            .expect("the outer turn runs");

        assert_eq!(
            cause(outcome),
            Error::Scheduler(SchedulerCause::Deadlock).to_string(),
            "this task may block, and nothing left in the store can ever meet \
             the condition it blocked on"
        );
        assert_eq!(
            entries(&log),
            vec!["mine", "other"],
            "a task the reference allows to block gives way to every ready \
             item, whichever instance queued it"
        );
        assert_eq!(
            store.internal().scheduler().queued_items(),
            0,
            "nothing was left queued"
        );
    }

    #[wcmp_macros::test]
    fn it_traps_with_the_stack_switch_cause_while_a_call_blocks_on_a_future_of_its_own() {
        let mut owner = store();
        let mut store = owner.internal().context();
        current_task(&store, false);

        // The store holds no host task: the future of a call that
        // blocked on one of its own stays in the frame that started
        // it, and the mark is what says so.
        let outcome = SuspendSeam::while_blocked_on_a_call_future(&mut store, |store| {
            store
                .internal()
                .run_in_turn(Waker::noop(), |store| {
                    SuspendSeam::suspend(store, |_| false)
                })
                .expect("the outer turn runs")
        });

        assert_eq!(
            cause(outcome),
            Error::Scheduler(SchedulerCause::StackSwitchNeeded).to_string(),
            "a future that can still resolve is pending, so the reference \
             permits this block and only the target has no provider to serve \
             it"
        );
        assert!(
            !store
                .internal()
                .scheduler()
                .suspend_seam()
                .blocked_on_a_call_future(),
            "the mark came back off when the blocked call's frame ended"
        );
    }

    #[wcmp_macros::test]
    fn it_runs_a_nested_turn_again_once_an_earlier_one_has_returned() {
        let mut owner = store();
        let mut store = owner.internal().context();
        let (reached, outer) = outer_waker();

        let first = store
            .internal()
            .run_in_turn(&outer, |store| SuspendSeam::suspend(store, |_| false))
            .expect("the outer turn runs");

        assert_eq!(
            cause(first),
            Error::Scheduler(SchedulerCause::Deadlock).to_string(),
            "the store was idle, so the first suspension gave up"
        );

        // A host task whose future completes on its first poll, so a
        // second nested turn has something to make the condition
        // hold.
        let (slot, polls) = host_task(&mut store, &reached, 1);
        let watched = slot.clone();
        let second = store
            .internal()
            .run_in_turn(&outer, move |store| {
                SuspendSeam::suspend(store, move |_| watched.lock().expect("slot").is_some())
            })
            .expect("the outer turn runs");

        assert_eq!(
            cause(second),
            "the seam returned with the condition held",
            "a suspension that gave up leaves the seam as it found it, so the \
             next one runs its nested turns too"
        );
        assert_eq!(polls.lock().expect("polls").clone(), vec![true]);
    }

    // The two tests below are native only: the browser aborts on a
    // panic instead of unwinding, so there is nothing to catch there
    // and nothing the seam could be left holding.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn it_puts_the_provider_back_when_the_suspension_panicked() {
        let mut owner = store();
        let mut store = owner.internal().context();
        store
            .internal()
            .scheduler_mut()
            .suspend_seam_mut()
            .set_provider(Panics);

        let unwound = unwind(|| SuspendSeam::suspend(&mut store, |_| false));

        assert!(
            unwound.is_err(),
            "the provider's panic unwound the suspension"
        );
        assert!(
            store.internal().scheduler().suspend_seam().has_provider(),
            "the provider went back into its slot, so the target still has the \
             capability it filled"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn it_clears_the_blocked_call_marker_when_a_blocked_call_panicked() {
        let mut owner = store();
        let mut store = owner.internal().context();

        let unwound = unwind(|| {
            SuspendSeam::while_blocked_on_a_call_future(
                &mut store,
                |_store: &mut StoreContext<'_, ()>| -> bool { panic!("the blocked call panicked") },
            )
        });

        assert!(unwound.is_err(), "the blocked call's panic unwound");
        assert!(
            !store
                .internal()
                .scheduler()
                .suspend_seam()
                .blocked_on_a_call_future(),
            "the seam no longer holds the mark of a call blocked on a future \
             of its own"
        );

        let again = store
            .internal()
            .run_in_turn(Waker::noop(), |store| {
                SuspendSeam::suspend(store, |_| false)
            })
            .expect("the outer turn runs");

        assert_eq!(
            cause(again),
            Error::Scheduler(SchedulerCause::Deadlock).to_string(),
            "the store is idle and no future of any frame is pending, so a \
             mark the panic left raised would have named the stack-switch \
             cause here"
        );
    }

    /// A component whose export calls a host function and adds one
    /// to what it returns. The host function's frame is the one a
    /// blocking built-in runs in: a trampoline, with the export
    /// call's driver the only driver on the stack.
    const CALLS_THE_HOST: &[u8] = component!(
        r#"
        (component
          (import "probe" (func $probe (param "x" u32) (result u32)))
          (core func $probe' (canon lower (func $probe)))
          (core module $m
            (import "" "probe" (func $probe (param i32) (result i32)))
            (func (export "run") (param i32) (result i32)
              local.get 0 call $probe i32.const 1 i32.add))
          (core instance $i (instantiate $m
            (with "" (instance (export "probe" (func $probe'))))))
          (func (export "run") (param "x" u32) (result u32)
            (canon lift (core func $i "run"))))
        "#
    );

    /// The same shape lifted `canon lift async` with a callback. The
    /// export calls the host function, adds one to what it returns,
    /// hands that to `task.return`, and exits. A call into such an
    /// export is a task the reference allows to block, so the host
    /// function's trampoline is a frame a suspension is served in
    /// rather than refused in.
    const CALLBACK_CALLS_THE_HOST: &[u8] = component!(
        r#"
        (component
          (import "probe" (func $probe (param "x" u32) (result u32)))
          (core func $probe' (canon lower (func $probe)))
          (core func $task-return (canon task.return (result u32)))
          (core module $m
            (import "" "probe" (func $probe (param i32) (result i32)))
            (import "" "task.return" (func $task-return (param i32)))
            (func (export "run") (param i32) (result i32)
              (call $task-return
                (i32.add (call $probe (local.get 0)) (i32.const 1)))
              (i32.const 0))
            (func (export "run-callback") (param i32 i32 i32) (result i32) unreachable))
          (core instance $i (instantiate $m
            (with "" (instance
              (export "probe" (func $probe'))
              (export "task.return" (func $task-return))))))
          (func (export "run") async (param "x" u32) (result u32)
            (canon lift (core func $i "run") async
              (callback (core func $i "run-callback")))))
        "#
    );

    #[wcmp_macros::test]
    async fn it_refuses_a_block_in_a_host_function_a_synchronous_export_called() {
        let engine = Engine::new().expect("engine");
        let component = Component::new(&engine, CALLS_THE_HOST)
            .await
            .expect("component parses");
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");

        // What the suspension reported, whether a turn of the store
        // was running when it did — the export call's, since the
        // host function enters no driver of its own — and how many
        // host tasks the store still held.
        let suspension: Arc<Mutex<Option<(String, bool, usize)>>> = Arc::new(Mutex::new(None));
        let recorded = suspension.clone();

        let mut linker: Linker<()> = Linker::new(&engine);
        linker
            .root()
            .func_wrap(
                "probe",
                move |mut call: HostCall<'_, ()>, (x,): (u32,)| -> Result<u32> {
                    // Everything below runs against the store as the
                    // trampoline reaches it: the core store's context
                    // the runtime layer handed it, and nothing besides.
                    // No driver, no `&mut Store`.
                    let store = call.store();

                    // A host task of this call, whose lowering would
                    // meet the condition below — in a turn this task is
                    // not allowed to take, because the export the guest
                    // is inside is synchronous.
                    let slot: Slot = Arc::new(Mutex::new(None));
                    let filled = slot.clone();
                    let subtask = store.internal().lock_tables()?.tasks.insert_subtask();
                    store.internal().push_host_task(HostTask::from_future(
                        subtask,
                        move |_store: &mut StoreContext<'_, ()>, outcome: Result<Vec<Val>>| {
                            *filled.lock().expect("slot") = Some(outcome);
                            Ok(())
                        },
                        core::future::ready(Ok(vec![Val::U32(x * 2)])),
                    ));

                    let watched = slot.clone();
                    let outcome = SuspendSeam::suspend(store, move |_store| {
                        watched.lock().expect("slot").is_some()
                    });
                    *recorded.lock().expect("record") = Some((
                        cause(outcome),
                        store.internal().turn_in_flight(),
                        store.internal().scheduler().host_task_count(),
                    ));
                    Ok(x)
                },
            )
            .expect("the registration");

        let instance = linker
            .instantiate(&mut store, &component)
            .await
            .expect("instantiate");
        let run = instance.get_func("run").expect("run export");
        let result = run
            .call(&mut store, &[Val::U32(20)])
            .await
            .expect("call run");

        assert_eq!(
            *suspension.lock().expect("record"),
            Some((
                Error::Scheduler(SchedulerCause::CannotBlock).to_string(),
                true,
                1
            )),
            "the host function reached the seam through what the trampoline \
             holds, from inside the turn of the export call; the call into a \
             synchronous export is one that must not block, and the refused \
             block left the store's host task for a later turn rather than \
             polling it"
        );
        assert_eq!(
            result.first(),
            Some(&Val::U32(21)),
            "the host function returned its argument and the guest added one"
        );
        assert_eq!(
            store.internal().scheduler().host_task_count(),
            0,
            "the turns the call's own driver ran after the call returned did \
             resolve it"
        );
    }

    #[wcmp_macros::test]
    async fn it_suspends_a_host_function_a_callback_export_called_until_a_host_task_completes() {
        let engine = Engine::new().expect("engine");
        let component = Component::new(&engine, CALLBACK_CALLS_THE_HOST)
            .await
            .expect("component parses");
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");

        // What the suspension reported, whether a turn of the store
        // was running when it did — the export call's, since the host
        // function enters no driver of its own — and how many host
        // tasks the store still held.
        let suspension: Arc<Mutex<Option<(String, bool, usize)>>> = Arc::new(Mutex::new(None));
        let recorded = suspension.clone();

        let mut linker: Linker<()> = Linker::new(&engine);
        linker
            .root()
            .func_wrap(
                "probe",
                move |mut call: HostCall<'_, ()>, (x,): (u32,)| -> Result<u32> {
                    // Everything below runs against the store as the
                    // trampoline reaches it: the core store's context the
                    // runtime layer handed it, and nothing besides. No
                    // driver, no `&mut Store`.
                    let store = call.store();

                    // A host task of this call. Its body is ready at
                    // once, and its lowering fills the slot below — but a
                    // lowering runs as a queued item in a later turn than
                    // the poll that saw the body complete, so the slot is
                    // what the suspension has to wait for.
                    let slot: Slot = Arc::new(Mutex::new(None));
                    let filled = slot.clone();
                    let subtask = store.internal().lock_tables()?.tasks.insert_subtask();
                    store.internal().push_host_task(HostTask::from_future(
                        subtask,
                        move |_store: &mut StoreContext<'_, ()>, outcome: Result<Vec<Val>>| {
                            *filled.lock().expect("slot") = Some(outcome);
                            Ok(())
                        },
                        core::future::ready(Ok(vec![Val::U32(x * 2)])),
                    ));

                    let watched = slot.clone();
                    let outcome = SuspendSeam::suspend(store, move |_store| {
                        watched.lock().expect("slot").is_some()
                    });
                    *recorded.lock().expect("record") = Some((
                        cause(outcome),
                        store.internal().turn_in_flight(),
                        store.internal().scheduler().host_task_count(),
                    ));

                    // What the host task produced, which only the turns
                    // the suspension ran could have put there.
                    let produced = slot.lock().expect("slot").take();
                    let Some(produced) = produced else {
                        return Err(Error::internal(
                            "the suspension returned with the host task unlowered",
                        ));
                    };
                    match produced?.first() {
                        Some(Val::U32(value)) => Ok(*value),
                        _ => Err(Error::internal("the host task produced no u32")),
                    }
                },
            )
            .expect("the registration");

        let instance = linker
            .instantiate(&mut store, &component)
            .await
            .expect("instantiate");
        let run = instance.get_func("run").expect("run export");
        let result = run
            .call(&mut store, &[Val::U32(20)])
            .await
            .expect("call run");

        assert_eq!(
            *suspension.lock().expect("record"),
            Some((
                "the seam returned with the condition held".to_owned(),
                true,
                0
            )),
            "the host function reached the seam through what the trampoline \
             holds, from inside the turn of the export call; the call is into \
             a callback export, which the reference allows to block, so the \
             nested turn polled the host task and ran its lowering, and the \
             task that completed left the store"
        );
        assert_eq!(
            result.first(),
            Some(&Val::U32(41)),
            "the guest's `task.return` carried what the host task produced \
             plus one, so the nested turns of the suspension polled the body \
             and ran the lowering it queued from inside the trampoline"
        );
        assert_eq!(
            store.internal().scheduler().host_task_count(),
            0,
            "the host task the call started is resolved and gone"
        );
        assert_eq!(
            store.internal().scheduler().queued_items(),
            0,
            "the task exited with its status word, so nothing of it is left \
             for a later turn"
        );
    }

    /// An item that gives way `gives_way` times and then sets
    /// `flag`, re-queueing itself as a resumption after a yield each
    /// time it gives way. That is what a callback task which returns
    /// the yield status word does: it runs, asks to be resumed, and
    /// runs again. `instance` tags the item as that instance's work,
    /// which is what a turn held to one instance looks for.
    ///
    /// `runs` counts the times it ran, so a test can say how far a
    /// block let it get. `usize::MAX` gives way for ever, which is
    /// the callee that spin-waits until a caller further down the
    /// stack unblocks it.
    fn yielder(
        runs: &Arc<Mutex<usize>>,
        gives_way: usize,
        flag: &Arc<Mutex<bool>>,
        instance: Option<InstanceId>,
    ) -> Item<()> {
        let counted = runs.clone();
        let flag = flag.clone();
        let item = Item::new(
            ItemKind::Callback,
            move |store: &mut StoreContext<'_, ()>| {
                let ran = {
                    let mut runs = counted.lock().expect("runs");
                    *runs += 1;
                    *runs
                };
                if ran > gives_way {
                    *flag.lock().expect("flag") = true;
                } else {
                    let next = yielder(&counted, gives_way, &flag, instance);
                    store.internal().scheduler_mut().push_low_priority(next);
                }
                Ok(())
            },
        );
        match instance {
            Some(instance) => item.in_instance(instance),
            None => item,
        }
    }

    /// An item that gives way for ever and queues one piece of fresh
    /// ready work each time it runs. The work counts `left` down and
    /// sets `flag` when it reaches zero, so the block that serves
    /// this pair makes real progress in every turn even though a
    /// resumption also runs in every turn.
    fn yielder_that_queues_work(
        runs: &Arc<Mutex<usize>>,
        left: &Arc<Mutex<usize>>,
        flag: &Arc<Mutex<bool>>,
    ) -> Item<()> {
        let counted = runs.clone();
        let left = left.clone();
        let flag = flag.clone();
        Item::new(
            ItemKind::Callback,
            move |store: &mut StoreContext<'_, ()>| {
                *counted.lock().expect("runs") += 1;
                let counting = left.clone();
                let set = flag.clone();
                store
                    .internal()
                    .scheduler_mut()
                    .push_high_priority(Item::new(
                        ItemKind::TaskStart,
                        move |_store: &mut StoreContext<'_, ()>| {
                            let mut left = counting.lock().expect("left");
                            *left = left.saturating_sub(1);
                            if *left == 0 {
                                *set.lock().expect("flag") = true;
                            }
                            Ok(())
                        },
                    ));
                let next = yielder_that_queues_work(&counted, &left, &flag);
                store.internal().scheduler_mut().push_low_priority(next);
                Ok(())
            },
        )
    }

    #[wcmp_macros::test]
    fn it_lets_a_yielder_that_gives_way_three_times_finish_inside_a_block() {
        let mut owner = store();
        let mut store = owner.internal().context();
        let released = Arc::new(Mutex::new(false));
        let runs = Arc::new(Mutex::new(0usize));
        store
            .internal()
            .scheduler_mut()
            .push_low_priority(yielder(&runs, 3, &released, None));

        let watched = released.clone();
        let outcome = store
            .internal()
            .run_in_turn(Waker::noop(), move |store| {
                SuspendSeam::suspend(store, move |_| *watched.lock().expect("flag"))
            })
            .expect("the outer turn runs");

        assert_eq!(
            cause(outcome),
            "the seam returned with the condition held",
            "the block served the yielder until it converged: a yielder that \
             gives way and then returns is not the shape the bound is for"
        );
        assert_eq!(
            *runs.lock().expect("runs"),
            4,
            "it ran once for each of the three times it gave way and once more \
             to return"
        );
    }

    #[wcmp_macros::test]
    fn it_lets_a_yielder_that_gives_way_the_whole_budget_finish_inside_a_block() {
        let mut owner = store();
        let mut store = owner.internal().context();
        let released = Arc::new(Mutex::new(false));
        let runs = Arc::new(Mutex::new(0usize));
        let gives_way = SPIN_BUDGET as usize;
        store
            .internal()
            .scheduler_mut()
            .push_low_priority(yielder(&runs, gives_way, &released, None));

        let watched = released.clone();
        let outcome = store
            .internal()
            .run_in_turn(Waker::noop(), move |store| {
                SuspendSeam::suspend(store, move |_| *watched.lock().expect("flag"))
            })
            .expect("the outer turn runs");

        assert_eq!(
            cause(outcome),
            "the seam returned with the condition held",
            "the budget is the run of turns the block spends before it gives \
             up, and a yielder that converges inside it is served to the end"
        );
        assert_eq!(*runs.lock().expect("runs"), gives_way + 1);
    }

    #[wcmp_macros::test]
    fn it_ends_a_block_whose_turns_ran_nothing_but_resumptions() {
        let mut owner = store();
        let mut store = owner.internal().context();
        let released = Arc::new(Mutex::new(false));
        let runs = Arc::new(Mutex::new(0usize));
        store.internal().scheduler_mut().push_low_priority(yielder(
            &runs,
            usize::MAX,
            &released,
            None,
        ));

        let watched = released.clone();
        let outcome = store
            .internal()
            .run_in_turn(Waker::noop(), move |store| {
                SuspendSeam::suspend(store, move |_| *watched.lock().expect("flag"))
            })
            .expect("the outer turn runs");

        assert_eq!(
            cause(outcome),
            Error::Scheduler(SchedulerCause::StackSwitchNeeded).to_string(),
            "the yielder re-queues itself every time it runs and this block \
             is what would release it, so the one thread that could release \
             it is the frame this block sits in and only a stack switch \
             would reach it"
        );
        assert_eq!(
            *runs.lock().expect("runs"),
            SPIN_BUDGET as usize + 1,
            "the block spent the budget and stopped: one turn to open the run \
             of turns the store did not serve and the budget's worth after it"
        );
        assert!(
            store.internal().scheduler().has_deferred_item(),
            "the resumption the yielder queued last is still in the store, \
             which is what the block gave up on rather than ran"
        );
    }

    #[wcmp_macros::test]
    fn it_keeps_serving_a_block_whose_turns_run_work_of_their_own() {
        let mut owner = store();
        let mut store = owner.internal().context();
        let released = Arc::new(Mutex::new(false));
        let runs = Arc::new(Mutex::new(0usize));
        // Well past the budget, so a bound that counted every turn
        // in which a resumption ran would give up long before the
        // work is done.
        let work = SPIN_BUDGET as usize * 3;
        let left = Arc::new(Mutex::new(work));
        store
            .internal()
            .scheduler_mut()
            .push_low_priority(yielder_that_queues_work(&runs, &left, &released));

        let watched = released.clone();
        let outcome = store
            .internal()
            .run_in_turn(Waker::noop(), move |store| {
                SuspendSeam::suspend(store, move |_| *watched.lock().expect("flag"))
            })
            .expect("the outer turn runs");

        assert_eq!(
            cause(outcome),
            "the seam returned with the condition held",
            "every turn ran an item that was not a resumption, so the run of \
             resumption-only turns never started and the block served the \
             store until its condition held"
        );
        assert_eq!(
            *left.lock().expect("left"),
            0,
            "the work the turns ran is what met the condition"
        );
        assert!(
            *runs.lock().expect("runs") > SPIN_BUDGET as usize,
            "the yielder gave way in more turns than the budget and the block \
             went on serving it"
        );
    }

    #[wcmp_macros::test]
    fn it_lets_a_yielder_of_its_own_instance_finish_under_a_task_that_must_not_block() {
        let mut owner = store();
        let mut store = owner.internal().context();
        let mine = current_task(&store, true);
        let released = Arc::new(Mutex::new(false));
        let runs = Arc::new(Mutex::new(0usize));
        store.internal().scheduler_mut().push_low_priority(yielder(
            &runs,
            3,
            &released,
            Some(mine),
        ));

        let watched = released.clone();
        let outcome = store
            .internal()
            .run_in_turn(Waker::noop(), move |store| {
                SuspendSeam::suspend(store, move |_| *watched.lock().expect("flag"))
            })
            .expect("the outer turn runs");

        assert_eq!(
            cause(outcome),
            "the seam returned with the condition held",
            "a task that must not block gives way to the resumptions of its \
             own instance, and the bound leaves a converging one alone there \
             too"
        );
        assert_eq!(*runs.lock().expect("runs"), 4);
    }

    #[wcmp_macros::test]
    fn it_ends_a_block_of_a_task_that_must_not_block_whose_turns_ran_nothing_but_resumptions() {
        let mut owner = store();
        let mut store = owner.internal().context();
        let mine = current_task(&store, true);
        let released = Arc::new(Mutex::new(false));
        let runs = Arc::new(Mutex::new(0usize));
        store.internal().scheduler_mut().push_low_priority(yielder(
            &runs,
            usize::MAX,
            &released,
            Some(mine),
        ));

        let watched = released.clone();
        let outcome = store
            .internal()
            .run_in_turn(Waker::noop(), move |store| {
                SuspendSeam::suspend(store, move |_| *watched.lock().expect("flag"))
            })
            .expect("the outer turn runs");

        assert_eq!(
            cause(outcome),
            Error::Scheduler(SchedulerCause::StackSwitchNeeded).to_string(),
            "the run of turns the store did not serve is counted whichever \
             queue the resumptions came off, and the budget names the stack \
             switch whatever the store's other rules would have named on idle"
        );
        assert_eq!(*runs.lock().expect("runs"), SPIN_BUDGET as usize + 1);
    }

    /// Give way `count` times over, each from its own frame as a
    /// guest loop calling `thread.yield` does, and report what the
    /// last of them answered.
    fn gives_way(store: &mut StoreContext<'_, ()>, count: u32) -> Result<()> {
        let mut outcome = Ok(());
        for _ in 0..count {
            outcome = SuspendSeam::give_way(store);
        }
        outcome
    }

    #[wcmp_macros::test]
    fn it_returns_from_every_give_way_of_a_run_inside_the_budget() {
        let mut owner = store();
        let mut store = owner.internal().context();

        assert_eq!(
            cause(gives_way(&mut store, SPIN_BUDGET)),
            "the seam returned with the condition held",
            "a thread that has given way no more times than the budget is \
             still being given its chances, so the built-in returns zero"
        );
    }

    #[wcmp_macros::test]
    fn it_ends_a_run_of_give_ways_past_the_budget_with_the_stack_switch_cause() {
        let mut owner = store();
        let mut store = owner.internal().context();

        assert_eq!(
            cause(gives_way(&mut store, SPIN_BUDGET + 1)),
            Error::Scheduler(SchedulerCause::StackSwitchNeeded).to_string(),
            "the store held nothing at every one of them and ran nothing \
             between them, so the thread is waiting on a guest frame on the \
             real stack and only a stack switch would reach it"
        );
    }

    #[wcmp_macros::test]
    fn it_starts_the_run_of_give_ways_over_when_the_store_ran_an_item() {
        let mut owner = store();
        let mut store = owner.internal().context();
        let log = log();

        // Twice the budget's worth of yields, with an item of the
        // store's run between each pair of them.
        let mut outcome = Ok(());
        for _ in 0..(SPIN_BUDGET * 2 + 2) {
            store
                .internal()
                .scheduler_mut()
                .push_high_priority(marker(&log, "ready"));
            outcome = SuspendSeam::give_way(&mut store);
        }

        assert_eq!(
            cause(outcome),
            "the seam returned with the condition held",
            "each yield gave way to work of the store's, so none of them is \
             asking for what the one before was refused, however many there \
             are"
        );
        assert_eq!(
            entries(&log).len(),
            (SPIN_BUDGET * 2 + 2) as usize,
            "every one of those items ran in the turn its yield gave way to"
        );
    }

    #[wcmp_macros::test]
    fn it_starts_the_run_of_give_ways_over_while_a_host_future_is_pending() {
        let mut owner = store();
        let mut store = owner.internal().context();
        let reached = Arc::new(Outer::default());
        // A body that never completes: the store holds a future
        // that can still resolve, so nothing here can say a yield
        // gave way to nothing.
        let (_slot, _polls) = host_task(&mut store, &reached, usize::MAX);

        assert_eq!(
            cause(gives_way(&mut store, SPIN_BUDGET * 2 + 2)),
            "the seam returned with the condition held",
            "a store holding a host future moves on its own when its \
             executor polls it again, so no run of yields taken against one \
             is ever counted"
        );
    }

    #[wcmp_macros::test]
    fn it_runs_the_ready_work_of_the_store_from_one_give_way() {
        let mut owner = store();
        let mut store = owner.internal().context();
        let log = log();
        store
            .internal()
            .scheduler_mut()
            .push_high_priority(marker(&log, "ready"));
        store
            .internal()
            .scheduler_mut()
            .push_low_priority(marker(&log, "deferred"));

        SuspendSeam::give_way(&mut store).expect("the yield gives way");

        assert_eq!(
            entries(&log),
            vec!["ready", "deferred"],
            "one nested turn runs what is ready and then the resumption the \
             yield rule was holding back, which is the whole of what a yield \
             gives way to"
        );
    }
}
