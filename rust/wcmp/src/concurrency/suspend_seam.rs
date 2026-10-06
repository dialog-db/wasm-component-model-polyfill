// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The scheduler's one suspend capability, and the nested turn it
//! falls back to.

use std::marker::PhantomData;

use crate::error::{Error, Result, SchedulerCause};
use crate::internal::ErrorInternal;
use crate::runtime_layer::Val as RuntimeVal;
use crate::store::StoreContext;
use crate::store::StoreContextInternalExt;

use super::block_step::BlockStep;
use super::pending_block::PendingBlock;
use super::plan::Plan;
use super::readiness::Readiness;
use super::seam_wait::SeamWait;
use super::thread_id::ThreadId;

/// The first part of a blocking built-in, which answers what it
/// found.
type Begin<T> = dyn Fn(&mut StoreContext<'_, T>, &[RuntimeVal]) -> anyhow::Result<BlockStep<T>>;

/// The whole of a blocking built-in that brings its own wait for a
/// thread that cannot suspend its stack. It answers `None` when it
/// left a plan for the scheduler and parked its step.
type Whole<T> =
    dyn Fn(&mut StoreContext<'_, T>, &[RuntimeVal]) -> anyhow::Result<Option<Vec<RuntimeVal>>>;

/// Whether one call of a blocking built-in names a thread to switch
/// to.
type Switches<T> = dyn Fn(&mut StoreContext<'_, T>, &[RuntimeVal]) -> bool;

/// The scheduler's one suspend capability.
///
/// A blocking built-in splits into a try part and a finish part, as
/// the reference's `Thread.wait_until` does. The try part runs in the
/// host trampoline and asks the seam to wait, through
/// [`wait_until`](Self::wait_until): the seam records the thread's
/// [`Readiness`] condition on the thread's record and checks it. A
/// condition that already holds makes the built-in ready at once.
/// Otherwise the seam suspends the thread until the condition holds.
/// Either way the thread's wait ends before the seam returns, and
/// the built-in's finish part then computes its result and writes
/// what it writes to guest memory, such as the event of
/// `waitable-set.wait`.
///
/// A readiness condition only reads the store. It changes nothing,
/// polls no host future, and runs no guest code, as the reference's
/// `ready_func`. The scheduler evaluates the conditions of every
/// waiting thread between two items, and notes the threads that
/// became ready in the order they became ready, which is the order
/// threads that became ready together resume in. `thread.yield`
/// waits on a condition that always holds, through
/// [`give_way`](Self::give_way), because a yield waits for nothing
/// but its turn.
///
/// A host frame cannot suspend a guest stack: a provider suspends a
/// thread only in WebAssembly, with nothing but WebAssembly frames
/// between the start of the thread's stack and the suspension. Under
/// a provider a blocking built-in therefore reaches the guest as the
/// switch module's shim for it, and the shim calls
/// [`try_block`](Self::try_block) and
/// [`finish_block`](Self::finish_block). A thread that runs on a
/// stack of its own suspends in the shim, and the scheduler resumes
/// it once its condition holds. No nested turn runs for it: a
/// provider never runs turns inside a suspension, and a readiness
/// condition runs no guest code, so a suspension and a nested turn
/// never meet.
///
/// Every other block takes the fallback below, through
/// [`block`](Self::block): each block of a store with no provider,
/// and under a provider a block of a thread that runs on another
/// thread's stack — the callee of a synchronous call between two
/// components — or of a task that must not block.
///
/// One thread of an instance that must not suspend still suspends
/// in the shim: a thread that a block of its own instance's call
/// started or last resumed, from inside that block. Its suspension
/// hands control back to the block's frame, which runs the ready
/// threads of the instance and nothing else, as the reference's
/// `canon_lift` runs them once a thread of a sync-typed call's
/// instance blocks. A switch it makes can name the thread of that
/// block, which the frame then lets go on.
///
/// The fallback is a nested turn, run from inside the guest call
/// that blocked. It runs the guest work of other tasks that is
/// ready and polls the host tasks the executor woke, with the waker
/// of the outer turn, until the condition holds or nothing can
/// progress. A host task that stays pending inside a nested turn
/// stays in the store for the outer turn, so no wake is lost.
///
/// A synchronous lower of a host `async` function parks its future
/// among the store's host tasks, and the poll that completes it
/// resolves the call's subtask, which is what the waiting thread's
/// condition watches. The fallback polls that parked task once
/// before each nested turn, with the task's own waker, as well as
/// letting the turns poll it when it is woken. That is the block's
/// own call, which the thread is inside, so a nested turn held to
/// one instance still reaches it, and a body that answered pending
/// without asking for a wake is still polled again.
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
///   built-in traps with the first of five causes that holds. Rules
///   2 and 3 are read together, in one walk of the frames below the
///   blocked thread from the innermost out, and the frame nearer the
///   block decides between them:
///
///   1. The cannot-block cause, when the blocked thread's own
///      instance must not suspend, which is the may-not-suspend flag
///      of the instance record, and the turns found no thread of that
///      instance ready.
///   2. The stack-switch cause, when a nested start lies between
///      the blocked thread and the base of the real stack. A start
///      intrinsic ran an `async`-typed callee from inside its own
///      frame, and the store marks that on its stack of current
///      scopes for as long as the callee runs. A provider would
///      return control below the mark, where the caller that
///      started the callee could still meet the condition. That
///      caller is guest code on the real stack rather than work the
///      store holds, so an idle store does not say it cannot move.
///      Only the target's capability is missing. The rule reads the
///      marks whose caller would go on: an asynchronous lower's, and
///      a synchronous lower's once the callee has resolved. Before
///      that, a synchronous lower's caller would get control back
///      only to wait for the callee, which runs the ready work this
///      block already ran, so that caller can release nothing.
///   3. The cannot-block cause, when a caller below waits for the
///      blocked callee, through a
///      synchronous lower or a fused adapter's direct call, in an
///      instance that must not suspend, with no other thread of that
///      instance ready. That caller's wait is a block of its own
///      instance, which the reference's `canon_lift` traps. A
///      synchronous call into an instance no caller below waits in
///      does not count.
///   4. The stack-switch cause, when the store holds a host task
///      that has not resolved — the parked future of a synchronous
///      lower included — because the reference permits that block
///      and only the target has no provider to serve it.
///   5. The deadlock cause in every other case. Then no frame below
///      can move, and nothing left in the store can ever meet the
///      condition.
/// - **A task that must not block gives way to its own instance
///   alone.** The rule is lazy, as the reference and Wasmtime state
///   it. A task whose instance may not suspend runs the ready work
///   of that instance and nothing else — no item of another
///   instance, and no host task but the parked one of its own call
///   — and the built-in then fails with the cannot-block cause when
///   the condition still does not hold. The ready work of the
///   instance includes its threads suspended in the provider whose
///   condition holds: the turn queues their resumptions and resumes
///   them through the provider, from inside the block. Under the
///   host-suspension provider that resumption is left to the store, as the
///   paragraph on that provider below states, so the blocked thread
///   suspends as well, and the scheduler runs the resumption and then
///   the rest of the wait, held to the instance. That is the case of
///   a start function, of a host call into a synchronous export, of a
///   synchronous call between two components, and of a resource
///   destructor. A task that is allowed to block runs every ready
///   item and polls every woken host task.
/// - **The seam keeps one budget, and past it the call fails with
///   the stack-switch cause.** This is the polyfill's one departure
///   from the reference, which bounds neither the yielded item the
///   first rule runs nor the number of times a thread gives way.
///   The seam counts the nested turns in a row in which the store
///   did nothing of its own: a turn that ran nothing, or nothing
///   but a resumption after a yield, against a store with no host
///   future that can still resolve. A turn that ran any other item,
///   or that ran against a store whose host future wants another
///   poll, starts the count over. A yield and a suspension fail once
///   the store's run passes the budget, since a thread that asks
///   again in one frame after another shows its spin only across
///   those frames. A wait for a condition fails once its own part of
///   the run does, the turns it ran itself in a row, so the yields a
///   thread took before it waited are not the wait's to count: a
///   guest that yields up to the budget and then blocks on an idle
///   store gets the cause the store's other rules name. Once the run passes
///   [`SPIN_BUDGET`] the seam gives up and the call the suspended
///   thread is inside fails with
///   [`SchedulerCause::StackSwitchNeeded`], because the one thread
///   that could release it is a guest frame on the real stack that
///   the store cannot reach and only a stack switch could resume.
///   The failure starts the count over, since it ends the run.
///   Three shapes reach the budget and they are one shape. A callee
///   that spin-waits in its event loop until its caller unblocks it
///   gives way, is re-queued, runs again and gives way again, and
///   the caller's block runs it every turn. A callee whose core
///   function calls `thread.yield` in a loop against a store that
///   holds nothing asks the seam over and over for what it was
///   refused the time before. A thread that a stackful caller's yield
///   started, and that yields while that caller is ready below it, is
///   the same: each of its yields finds only the caller, which no
///   nested turn can reach, so with no provider it fails once its
///   yields pass the budget, however few the caller's own yields
///   are. Two corpus directives depend on the
///   budget, and without it both run for ever rather than failing.
///   The bound is a budget and not a proof: a yielder that
///   converges after more than [`SPIN_BUDGET`] turns of its own
///   would be cut short by it, which is why the number is drawn
///   generously.
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
/// The fallback runs the same way on both targets. A nested turn
/// runs from inside the lowered import the guest blocked in, so that
/// import's host function is on the stack for as long as the block
/// lasts, and every level of nesting leaves its own import's host
/// function there too. An item a turn runs which calls one of those
/// imports again is a second call of a host function already on the
/// stack, and both backends enter a host function at any depth.
///
/// Under the host-suspension provider a nested turn cannot resume a thread
/// suspended in the provider: the browser resumes a suspended stack
/// on a microtask, never inside the call that asks for it. The
/// fallback therefore runs as a [`SeamWait`], which stops where an
/// item leaves a resumption to the store. The seam then leaves the
/// rest of the wait to the scheduler as a [`Plan`], and the built-in's
/// shim suspends the thread the built-in runs in. The scheduler runs
/// the resumption, whatever it leads to, and the rest of the wait,
/// from a turn of a driver, and then resumes the thread with what the
/// wait ended with, which the try part reads on its retry. A first
/// part that leaves a resumption to the store, as a start intrinsic
/// whose callee switched to a suspended thread does, leaves a plan
/// the same way. No other item runs and no other host task is polled
/// in between, so a guest cannot tell.
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
/// [`SPIN_BUDGET`]: super::scheduler::SPIN_BUDGET
pub struct SuspendSeam<T: 'static> {
    unserved_turns: u32,
    served_mark: (u64, u64),
    /// The seam serves the stores of one host data type, whose
    /// context each of its entries takes.
    store: PhantomData<fn(T)>,
}

impl<T: 'static> SuspendSeam<T> {
    /// Construct the seam, with no nested turn noted yet.
    pub fn new() -> Self {
        Self {
            unserved_turns: 0,
            served_mark: (0, 0),
            store: PhantomData,
        }
    }

    /// Wait until `readiness` holds, which is the whole of what the
    /// try part of a blocking built-in asks of the seam.
    ///
    /// The seam records the condition on the current thread's record,
    /// which is where the scheduler evaluates it between two items,
    /// and checks it. A condition that already holds makes the
    /// built-in ready, and the seam returns at once: a wait whose set
    /// already holds an event, a copy whose end already holds its
    /// event, a call that already resolved. Otherwise the thread
    /// suspends until the condition holds, through the fallback, under
    /// the rules the type states. The wait ends before the seam
    /// returns, whichever way it went, so the thread's record never
    /// keeps a condition it is no longer suspended on.
    ///
    /// It returns `Ok(())` with the condition true, and otherwise the
    /// scheduler error the built-in traps with. The finish part of
    /// the built-in runs after it.
    ///
    /// A thread is recorded only when the store has a current
    /// thread. A block with none, such as the host task of a store
    /// that runs no task, waits on the condition all the same.
    ///
    /// The wait ends on an unwind too, for the reason [`SeamWait`]
    /// gives. This wait leaves no plan: a wait that stops for the
    /// store fails with the stack-switch cause.
    pub fn wait_until(store: &mut StoreContext<'_, T>, readiness: Readiness) -> Result<()> {
        let mut wait = SeamWait::until(store, readiness)?;
        wait.run(store)
            .unwrap_or(Err(Error::Scheduler(SchedulerCause::StackSwitchNeeded)))
    }

    /// Run a blocking built-in whole, where it stands: its first
    /// part, then, when it waits, the wait through
    /// [`wait_until`](Self::wait_until) — or [`give_way`](Self::give_way)
    /// for a yield — and its finish part.
    ///
    /// This is the built-in's host trampoline when the engine has no
    /// provider. The finish part runs whichever way the wait went, so
    /// a wait that failed gives back what the first part took.
    pub fn block(
        store: &mut StoreContext<'_, T>,
        begin: &Begin<T>,
        args: &[RuntimeVal],
    ) -> anyhow::Result<Vec<RuntimeVal>> {
        Self::block_or_plan(store, begin, args)?.ok_or_else(|| {
            anyhow::Error::from(Error::internal(
                "a blocking built-in left a plan with no provider to run it",
            ))
        })
    }

    /// Run a blocking built-in whole, as [`block`](Self::block) does,
    /// or leave the rest of it to the scheduler as a plan. This is
    /// what the try part of the built-in's shim runs for a thread
    /// that cannot suspend its stack, under a provider.
    ///
    /// A first part or a wait that leaves work to the store, which is
    /// what an item or a start that has to resume a thread does under
    /// the host-suspension provider, cannot go on inside the guest call. The rest
    /// of the built-in — its wait, if it has one, and its finish part
    /// — goes to the scheduler as a plan, the built-in's step waits
    /// for the plan's outcome, and this answers `None`: the try part
    /// answers that the built-in is not ready, and the shim suspends
    /// the thread. A wait that unwinds hands the finish part a failure
    /// before the panic carries on: an item a nested turn runs and a
    /// host task it polls can each panic, and a copy, a call, or a
    /// host future the first part left in the store would otherwise
    /// outlive the frame that waited on it.
    pub fn block_or_plan(
        store: &mut StoreContext<'_, T>,
        begin: &Begin<T>,
        args: &[RuntimeVal],
    ) -> anyhow::Result<Option<Vec<RuntimeVal>>> {
        let step = begin(store, args)?;
        let readiness = step.readiness();
        if store.internal().defers_work() {
            // The wait begins once the work the first part left is
            // done, as it would begin once the first part returned.
            let mut plan = Self::plan(store)?;
            plan.then_wait = readiness;
            Self::park(store, step)?;
            if let Err(error) = store.internal().leave_plan(plan) {
                Self::unpark(store);
                return Err(error.into());
            }
            return Ok(None);
        }
        let Some(readiness) = readiness else {
            return Ok(Some(step.finish(store, Ok(()))?));
        };
        let waited = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<_> {
            let wait = if readiness == Readiness::Yielded {
                SeamWait::give_way(store)?
            } else {
                SeamWait::until(store, readiness)?
            };
            Ok(Self::wait_or_plan(store, wait))
        }));
        match waited {
            Ok(Ok(Some(waited))) => Ok(Some(step.finish(store, waited)?)),
            Ok(Ok(None)) => {
                Self::park(store, step)?;
                Ok(None)
            }
            Ok(Err(error)) => Ok(Some(step.finish(store, Err(error))?)),
            Err(panic) => {
                let _ = step.finish(
                    store,
                    Err(Error::internal("a blocked built-in's wait unwound")),
                );
                std::panic::resume_unwind(panic)
            }
        }
    }

    /// The try part of a blocking built-in's shim: answer whether
    /// the built-in is ready, which is when the shim calls
    /// [`finish_block`](Self::finish_block), and otherwise leave the
    /// thread waiting for the shim to suspend it.
    ///
    /// The first try of a built-in runs its first part. A built-in
    /// that is done is ready, and its results wait for the finish.
    /// One that waits records its readiness condition on the current
    /// thread, which is where the scheduler evaluates it between two
    /// items, and it is ready when the condition already holds. A
    /// yield is never ready on its first try, because a yield always
    /// gives way.
    ///
    /// The shim suspends a thread that is not ready. The scheduler
    /// resumes it once its condition holds, in the order the
    /// threads became ready, and the shim tries again. That try runs
    /// nothing of the built-in: it only asks whether the condition
    /// still holds, since another thread may have taken what it
    /// waited for between the two, and a thread whose condition no
    /// longer holds suspends again. A thread whose built-in left a
    /// plan is ready once the scheduler resumes it, with the plan's
    /// outcome for the finish part.
    ///
    /// Only a thread whose entry started through the provider runs on
    /// a stack of its own, so only such a thread suspends. A thread
    /// that runs on the stack of another, which is the callee of a
    /// synchronous call between two components, and a thread of a
    /// task that must not block, wait where they stand, through
    /// [`block_or_plan`](Self::block_or_plan). A task that must not
    /// block must return before it waits on anything but the ready
    /// work of its own instance, and the nested turn serves exactly
    /// that rule. A built-in that brings a `fallback` of its own runs
    /// that instead. A thread on a stack of its own whose instance may
    /// not suspend suspends all the same when a block of its own
    /// instance's call started or last resumed it: the suspension
    /// returns control to that block, which serves the same rule.
    ///
    /// A call that `switches` says names a thread to switch to is the
    /// other exception for a task that must not block, where no block
    /// of its own instance's call lies below: the thread of a host call
    /// into a sync-typed export. Its thread suspends in the shim when
    /// it runs on a stack of its own, and the frame that started or
    /// resumed it runs the named thread. A switch waits on
    /// nothing, so it blocks nothing. Wasmtime makes the same
    /// exception: its suspension intrinsic reads the may-not-suspend
    /// flag only when the call names no thread to switch to. The
    /// scheduler records such a thread as a switcher, and the frame
    /// that started or resumed it resumes it before anything else once
    /// the threads the switch ran have stopped, if it is ready then.
    /// That is the reference's `canon_lift`, which runs the ready
    /// threads of a sync-typed task's own instance until the task
    /// resolves.
    ///
    /// A first part that leaves work to the store, which a start
    /// intrinsic whose callee switched to a suspended thread does
    /// under the host-suspension provider, leaves a plan for the thread: the
    /// scheduler does that work first, then records the built-in's
    /// condition on the thread and resumes it at once when the
    /// condition holds, as the try would have answered.
    ///
    /// A thread that suspended itself waits on
    /// [`Readiness::Resumed`], which no thread's record holds: the
    /// thread waits on nothing until a resume names it, so it joins
    /// no list of waiting threads, and only its shim asks the
    /// condition.
    pub fn try_block(
        store: &mut StoreContext<'_, T>,
        begin: &Begin<T>,
        fallback: Option<&Whole<T>>,
        switches: Option<&Switches<T>>,
        args: &[RuntimeVal],
    ) -> anyhow::Result<bool> {
        let (thread, own_stack, returns_to) = {
            let guard = store.internal().lock_tables()?;
            let thread = guard.tasks.current_thread();
            let own_stack = thread.is_some_and(|thread| guard.tasks.on_own_stack(thread));
            let returns_to = thread.and_then(|thread| guard.tasks.returns_to(thread));
            (thread, own_stack, returns_to)
        };
        if let Some(thread) = thread
            && let Some(block) = store.internal().scheduler_mut().block_mut(thread)
        {
            if block.waited.is_some() {
                return Ok(true);
            }
            let readiness = block.readiness;
            return Ok(Self::holds(store, readiness));
        }
        let must_not_block = store.internal().must_not_block_instance().is_some();
        // A thread whose instance must not suspend suspends only when a
        // block of its own instance's call started or last resumed it,
        // or, with no such block below, when the call names a thread to
        // switch to. The second is a switcher its frame takes back.
        let switcher = own_stack
            && must_not_block
            && returns_to.is_none()
            && switches.is_some_and(|switches| switches(store, args));
        let suspends = own_stack && (!must_not_block || returns_to.is_some() || switcher);
        let (Some(thread), true) = (thread, suspends) else {
            let done = match fallback {
                Some(whole) => whole(store, args)?,
                None => Self::block_or_plan(store, begin, args)?,
            };
            let Some(values) = done else {
                return Ok(false);
            };
            store.internal().scheduler_mut().keep_ready_block(values);
            return Ok(true);
        };
        // A thread the first part starts may start as the store's flight,
        // under a provider that runs a thread once the driver awaits it,
        // since this thread suspends for it here. Its end then reaches
        // the scheduler whole, a trap's reason included.
        store
            .internal()
            .scheduler_mut()
            .deferred_mut()
            .may_defer_start = true;
        let step = begin(store, args);
        store
            .internal()
            .scheduler_mut()
            .deferred_mut()
            .may_defer_start = false;
        let step = step?;
        if store.internal().defers_work() {
            // The condition is recorded once the work is done, as it
            // would be once the first part returned. Until then the
            // thread is inside the built-in, and waits on nothing.
            let readiness = step.readiness().unwrap_or(Readiness::Planned);
            let mut plan = Self::plan(store)?;
            plan.suspends = true;
            let previous = Self::current_readiness(store, thread)?;
            store
                .internal()
                .scheduler_mut()
                .begin_block(thread, PendingBlock::new(readiness, previous, step));
            // A switcher that leaves the rest of its block as a plan is
            // still a switcher its frame takes back, recorded at the
            // frame's level of deferred work as the other branch records
            // it.
            let mark = store.internal().scheduler().switcher_mark();
            if switcher {
                store.internal().scheduler_mut().push_switcher(thread);
            }
            if let Err(error) = store.internal().leave_plan(plan) {
                store.internal().scheduler_mut().end_block(thread);
                store.internal().scheduler_mut().pop_switcher_above(mark);
                return Err(error.into());
            }
            return Ok(false);
        }
        let Some(readiness) = step.readiness() else {
            let values = step.finish(store, Ok(()))?;
            store.internal().scheduler_mut().keep_ready_block(values);
            return Ok(true);
        };
        let previous = {
            let mut guard = store.internal().lock_tables()?;
            match readiness {
                Readiness::Resumed { .. } | Readiness::Planned => guard
                    .tasks
                    .thread(thread)
                    .and_then(|record| record.readiness),
                _ => guard.tasks.start_waiting(thread, readiness)?,
            }
        };
        store
            .internal()
            .scheduler_mut()
            .begin_block(thread, PendingBlock::new(readiness, previous, step));
        if switcher {
            store.internal().scheduler_mut().push_switcher(thread);
        }
        Ok(readiness != Readiness::Yielded && Self::holds(store, readiness))
    }

    /// The finish part of a blocking built-in's shim: the built-in's
    /// results, once its try part answered that it is ready.
    ///
    /// The results of a built-in that was done at its first try are
    /// the ones the try kept. A built-in whose thread waited ends the
    /// wait, which takes the thread off the list of waiting threads,
    /// and runs its finish part, with what the wait of its plan ended
    /// with when it left one.
    pub fn finish_block(store: &mut StoreContext<'_, T>) -> anyhow::Result<Vec<RuntimeVal>> {
        let thread = store.internal().lock_tables()?.tasks.current_thread();
        let block = thread.and_then(|thread| store.internal().scheduler_mut().end_block(thread));
        if let (Some(thread), Some(block)) = (thread, block) {
            // A built-in that left a plan recorded nothing on the
            // thread for it: the plan's wait kept its own record, and
            // ended it before the thread resumed.
            if block.readiness != Readiness::Planned {
                store
                    .internal()
                    .lock_tables()?
                    .tasks
                    .stop_waiting(thread, block.previous);
            }
            let waited = block.waited.unwrap_or(Ok(()));
            return block.step.finish(store, waited);
        }
        store
            .internal()
            .scheduler_mut()
            .take_ready_block()
            .ok_or_else(|| anyhow::anyhow!("a shim finished a built-in its try did not begin"))
    }

    /// Whether `readiness` holds in the store.
    fn holds(store: &StoreContext<'_, T>, readiness: Readiness) -> bool {
        store
            .internal_ref()
            .lock_tables()
            .is_ok_and(|guard| guard.tasks.readiness_holds(readiness))
    }

    /// Suspend the current guest thread until `condition` holds.
    ///
    /// This is the seam's primitive. [`wait_until`](Self::wait_until)
    /// goes through it with the condition of a [`Readiness`], which
    /// is what a blocking built-in records. The condition must only
    /// read the store, as a readiness condition does: the seam hands
    /// it a shared borrow, so it cannot run guest code or poll a
    /// store's host task, and it must hold nothing it would change. It
    /// returns `Ok(())` with the condition true, and otherwise the
    /// scheduler error the built-in traps with.
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
        condition: impl Fn(&StoreContext<'_, T>) -> bool,
    ) -> Result<()> {
        SeamWait::run_until(store, &condition)
    }

    /// Give way once, which is the whole of what `thread.yield`
    /// asks of the seam.
    ///
    /// A yield waits for one chance to be given back control and
    /// for nothing else, so it records a condition that always
    /// holds, and it gives way whether or not that condition holds:
    /// a yield always gives way. The seam runs exactly one nested
    /// turn: the ready work of the store, or of the calling task's own
    /// instance alone when that task must not block. A task that must not
    /// block with no ready work of its own instance gives way to
    /// nothing, as Wasmtime runs it.
    ///
    /// The seam's budget is the one thing that can fail this. A
    /// thread whose turns the store has not served past
    /// [`SPIN_BUDGET`] times over is spin-waiting for a guest frame
    /// on the real stack, and the failure is the stack-switch cause
    /// that ends the call the thread is inside. Everything else
    /// answers `Ok(Some(()))`, which is what makes the built-in return
    /// zero whenever it returns at all. It answers `Ok(None)` when it
    /// left the rest of the yield to the scheduler as a plan.
    ///
    /// [`SPIN_BUDGET`]: super::scheduler::SPIN_BUDGET
    pub fn give_way(store: &mut StoreContext<'_, T>) -> Result<Option<()>> {
        let wait = SeamWait::give_way(store)?;
        Self::waited(store, wait)
    }

    /// Suspend the current thread, run `switch`, and then wait until
    /// a resume names the thread, which is what `thread.suspend` and
    /// the two built-ins that suspend and then switch ask of the seam.
    ///
    /// The try part suspends the thread: it neither runs nor waits on
    /// any condition. `switch` runs next, with the thread suspended.
    /// It is the switch of the reference's `Thread.resume` loop,
    /// which runs the named thread before anything else, and it does
    /// nothing for a plain suspension. The thread then waits until a
    /// resume makes it ready. A resume that runs while `switch` runs
    /// makes it ready at once. Otherwise the wait goes through the
    /// fallback, under the rules the type states, and a thread that
    /// nothing resumes fails with the cause those rules select. The
    /// suspension ends before the seam returns, however it went, and
    /// on an unwind too, for the reason [`SeamWait`] gives. A failure
    /// of `switch` is the seam's failure. A switch or a wait that
    /// leaves work to the store leaves the rest to the scheduler as a
    /// plan, and the seam answers `Ok(None)`.
    pub fn suspend_current(
        store: &mut StoreContext<'_, T>,
        switch: impl FnOnce(&mut StoreContext<'_, T>) -> Result<()>,
    ) -> Result<Option<()>> {
        let wait = SeamWait::suspended(store)?;
        switch(store)?;
        if store.internal().defers_work() {
            return Self::leave(store, wait);
        }
        Self::waited(store, wait)
    }

    /// Make the current thread ready and run `switch`, which is what
    /// the two built-ins that yield and then switch ask of the seam.
    ///
    /// The thread waits on a condition that always holds for as long
    /// as `switch` runs, as the reference's `yield_then_resume`
    /// records it: a thread that the started thread names sees it
    /// ready and not suspended. Once `switch` has returned, the
    /// thread's yield has given way to the thread it named, and its
    /// condition holds, so it goes on at once. A failure of `switch`
    /// is the seam's failure. A switch that leaves work to the store
    /// leaves the rest to the scheduler as a plan, and the seam
    /// answers `Ok(None)`.
    pub fn yield_to(
        store: &mut StoreContext<'_, T>,
        switch: impl FnOnce(&mut StoreContext<'_, T>) -> Result<()>,
    ) -> Result<Option<()>> {
        let wait = SeamWait::yield_to(store)?;
        switch(store)?;
        if store.internal().defers_work() {
            return Self::leave(store, wait);
        }
        Ok(Some(()))
    }

    /// Park `step`, the step of the built-in the current thread runs,
    /// until the plan the built-in leaves is done: the thread's retry
    /// reads the plan's outcome from it.
    pub fn park(store: &mut StoreContext<'_, T>, step: BlockStep<T>) -> Result<()> {
        let thread = Self::current(store)?;
        store
            .internal()
            .scheduler_mut()
            .begin_block(thread, PendingBlock::new(Readiness::Planned, None, step));
        Ok(())
    }

    /// Run `wait` to its end, or leave the rest of it to the
    /// scheduler as a plan, which answers `Ok(None)`. A failed wait is
    /// the seam's failure.
    fn waited(store: &mut StoreContext<'_, T>, wait: SeamWait<T>) -> Result<Option<()>> {
        match Self::wait_or_plan(store, wait) {
            Some(waited) => waited.map(Some),
            None => Ok(None),
        }
    }

    /// Run `wait` until it ends, answering what it ended with, or
    /// until it stops for work it left to the store, in which case the
    /// rest of it goes to the scheduler as a plan and this answers
    /// `None`. A wait that stops where the thread cannot suspend its
    /// stack ends with the stack-switch cause.
    fn wait_or_plan(store: &mut StoreContext<'_, T>, mut wait: SeamWait<T>) -> Option<Result<()>> {
        if let Some(waited) = wait.run(store) {
            return Some(waited);
        }
        match Self::leave(store, wait) {
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        }
    }

    /// Leave the rest of `wait` to the scheduler as a plan for the
    /// current thread, answering `Ok(None)`.
    fn leave(store: &mut StoreContext<'_, T>, wait: SeamWait<T>) -> Result<Option<()>> {
        let mut plan = Self::plan(store)?;
        plan.wait = Some(wait);
        store.internal().leave_plan(plan)?;
        Ok(None)
    }

    /// An empty plan for the built-in the current thread runs.
    fn plan(store: &mut StoreContext<'_, T>) -> Result<Plan<T>> {
        Ok(Plan::new(Self::current(store)?))
    }

    /// Take back the step [`park`](Self::park) parked, for a plan the
    /// store refused.
    fn unpark(store: &mut StoreContext<'_, T>) {
        if let Ok(thread) = Self::current(store) {
            store.internal().scheduler_mut().end_block(thread);
        }
    }

    /// The current thread of `store`. Every guest call runs a thread.
    fn current(store: &mut StoreContext<'_, T>) -> Result<ThreadId> {
        store
            .internal()
            .lock_tables()?
            .tasks
            .current_thread()
            .ok_or_else(|| Error::internal("a blocking built-in ran with no thread on the stack"))
    }

    /// The condition `thread`'s record holds now.
    fn current_readiness(
        store: &mut StoreContext<'_, T>,
        thread: ThreadId,
    ) -> Result<Option<Readiness>> {
        Ok(store
            .internal()
            .lock_tables()?
            .tasks
            .thread(thread)
            .and_then(|record| record.readiness))
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
    /// therefore one run across every yield of the store: a thread
    /// that gives way in one frame after another builds a single run,
    /// and whatever the store ran in between — a driver's turn
    /// included — ends it. The answer is the length of the run, zero
    /// when the store served this turn. A block holds its own turns to
    /// the budget as well, counted from its start: see
    /// [`SeamWait`](super::SeamWait).
    ///
    /// [`SPIN_BUDGET`]: super::scheduler::SPIN_BUDGET
    pub fn note_turn(store: &mut StoreContext<'_, T>) -> u32 {
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
        seam.unserved_turns
    }

    /// The failure a suspension ends with once the run of turns the
    /// store did not serve has passed [`SPIN_BUDGET`].
    ///
    /// The run starts over here. The failure ends the call the
    /// spinning thread is inside, so the run it built ends with it. A
    /// later block of the same store, in the next call a host makes,
    /// say, is a run of its own, and would otherwise read the
    /// finished run as its own and fail with the stack-switch cause
    /// at its first turn, whatever the store's other rules would name.
    ///
    /// [`SPIN_BUDGET`]: super::scheduler::SPIN_BUDGET
    pub fn past_budget(store: &mut StoreContext<'_, T>) -> Error {
        store
            .internal()
            .scheduler_mut()
            .suspend_seam_mut()
            .unserved_turns = 0;
        Error::Scheduler(SchedulerCause::StackSwitchNeeded)
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
    use super::super::lower_kind::LowerKind;
    use super::super::outcome::Outcome;
    use super::super::scheduler::SPIN_BUDGET;
    use super::super::subtask_id::SubtaskId;

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

    /// What a test read of a waiting thread while it waited: the
    /// condition its record held, and one more reading beside it.
    type Seen<V> = Arc<Mutex<Option<(Option<Readiness>, V)>>>;

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
            .insert_subtask()
            .expect("room under the record cap");
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
        let task = guard
            .tasks
            .create_task(None, None, instance)
            .expect("room under the record cap");
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
            .create_task(None, None, instance)
            .expect("room under the record cap");
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
        let mut driver = Box::pin(Driver::run(
            store.internal().reborrow(),
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
    fn it_reads_the_causes_past_the_cannot_block_rule_when_a_thread_of_its_instance_is_ready_below()
    {
        let mut owner = store();
        let mut store = owner.internal().context();
        {
            // A thread of the instance switched to the blocked thread
            // from a built-in of its own on the real stack, and has
            // been made ready since: it waits below the blocked thread,
            // where no nested turn reaches it.
            let mut guard = store.internal_ref().tables().lock().expect("tables");
            let instance = guard.tasks.insert_instance();
            guard
                .tasks
                .instance_mut(instance)
                .expect("instance record")
                .may_not_suspend = true;
            let below = guard
                .tasks
                .create_task(None, None, instance)
                .expect("room under the record cap");
            let below = guard.tasks.task(below).expect("task").implicit_thread;
            guard
                .tasks
                .start_waiting(below, Readiness::Yielded)
                .expect("the thread below waits");
            guard.tasks.begin_thread_switch(below);
            let task = guard
                .tasks
                .create_task(None, None, instance)
                .expect("room under the record cap");
            guard.tasks.push_task_scope(task);
        }

        let outcome = store
            .internal()
            .run_in_turn(Waker::noop(), |store| {
                SuspendSeam::suspend(store, |_| false)
            })
            .expect("the outer turn runs");

        assert_eq!(
            cause(outcome),
            Error::Scheduler(SchedulerCause::StackSwitchNeeded).to_string(),
            "another thread of the instance is ready, so the cannot-block \
             rule does not hold, and that thread would go on under a stack \
             switch"
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

    /// Mark the stack of current scopes where a start intrinsic runs
    /// an `async`-typed callee from inside its own frame, for a
    /// caller that lowered the call through `lower`, and make a task
    /// of a fresh instance the callee's scope above the mark. The
    /// subtask it answers is the caller's record of the call.
    fn nested_start(
        store: &StoreContext<'_, ()>,
        lower: LowerKind,
        may_not_suspend: bool,
    ) -> SubtaskId {
        let subtask = {
            let mut guard = store.internal_ref().tables().lock().expect("tables");
            let subtask = guard
                .tasks
                .insert_subtask()
                .expect("room under the record cap");
            guard.tasks.begin_nested_start(subtask, lower);
            subtask
        };
        current_task(store, may_not_suspend);
        subtask
    }

    #[wcmp_macros::test]
    fn it_traps_with_the_stack_switch_cause_when_an_asynchronous_start_lies_below_the_block() {
        let mut owner = store();
        let mut store = owner.internal().context();
        // The caller that lowered the call is below the mark, and a
        // task of its own is the scope under it.
        current_task(&store, false);
        let _ = nested_start(&store, LowerKind::Async, false);

        let outcome = store
            .internal()
            .run_in_turn(Waker::noop(), |store| {
                SuspendSeam::suspend(store, |_| false)
            })
            .expect("the outer turn runs");

        assert_eq!(
            cause(outcome),
            Error::Scheduler(SchedulerCause::StackSwitchNeeded).to_string(),
            "the store is idle, but the caller below the nested start could \
             still meet the condition once a provider returned control to it"
        );
    }

    #[wcmp_macros::test]
    fn it_traps_with_the_stack_switch_cause_above_an_asynchronous_start_while_the_caller_is_inside_a_synchronous_call()
     {
        let mut owner = store();
        let mut store = owner.internal().context();
        // The caller's instance is inside a synchronous call, and the
        // callee's instance, where the block is, is not.
        current_task(&store, true);
        let _ = nested_start(&store, LowerKind::Async, false);

        let outcome = store
            .internal()
            .run_in_turn(Waker::noop(), |store| {
                SuspendSeam::suspend(store, |_| false)
            })
            .expect("the outer turn runs");

        assert_eq!(
            cause(outcome),
            Error::Scheduler(SchedulerCause::StackSwitchNeeded).to_string(),
            "the caller below the asynchronous start would go on once a \
             provider returned control to it, so its synchronous call does \
             not make this block fail with the cannot-block cause"
        );
    }

    #[wcmp_macros::test]
    fn it_traps_with_the_deadlock_cause_above_a_synchronous_start_whose_callee_has_not_resolved() {
        let mut owner = store();
        let mut store = owner.internal().context();
        current_task(&store, false);
        let _ = nested_start(&store, LowerKind::Sync, false);

        let outcome = store
            .internal()
            .run_in_turn(Waker::noop(), |store| {
                SuspendSeam::suspend(store, |_| false)
            })
            .expect("the outer turn runs");

        assert_eq!(
            cause(outcome),
            Error::Scheduler(SchedulerCause::Deadlock).to_string(),
            "a synchronous lower's caller would only wait for its callee, \
             which runs the work this block already ran, so no frame below \
             can move"
        );
    }

    #[wcmp_macros::test]
    fn it_traps_with_the_cannot_block_cause_above_a_synchronous_start_whose_caller_must_not_block()
    {
        let mut owner = store();
        let mut store = owner.internal().context();
        // The caller's instance is inside a synchronous call, and the
        // callee's instance, where the block is, is not.
        current_task(&store, true);
        let _ = nested_start(&store, LowerKind::Sync, false);

        let outcome = store
            .internal()
            .run_in_turn(Waker::noop(), |store| {
                SuspendSeam::suspend(store, |_| false)
            })
            .expect("the outer turn runs");

        assert_eq!(
            cause(outcome),
            Error::Scheduler(SchedulerCause::CannotBlock).to_string(),
            "the synchronous lower's caller would wait for the callee in an \
             instance that must not suspend, and that wait traps"
        );
    }

    #[wcmp_macros::test]
    fn it_traps_with_the_deadlock_cause_while_only_another_instance_must_not_suspend() {
        let mut owner = store();
        let mut store = owner.internal().context();
        // Instance A has a synchronous call in progress, and the block
        // is in instance B, which may suspend. The cannot-block rule
        // reads the blocked thread's own instance alone.
        {
            let mut guard = store.internal().tables().lock().expect("tables");
            let other = guard.tasks.insert_instance();
            guard
                .tasks
                .instance_mut(other)
                .expect("instance record")
                .may_not_suspend = true;
        }
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
            "a synchronous call in another instance of the store does not \
             make this block fail with the cannot-block cause"
        );
    }

    #[wcmp_macros::test]
    fn it_reads_the_frames_below_a_block_innermost_first() {
        let mut owner = store();
        let mut store = owner.internal().context();
        // A lowers C asynchronously, C calls the sync-typed C2
        // directly, C2 lowers D synchronously, and D blocks. C2 waits
        // for D right below the block, in an instance that must not
        // suspend, and traps before control would ever return to A's
        // asynchronous lower further down.
        current_task(&store, false);
        let _ = nested_start(&store, LowerKind::Async, false);
        current_task(&store, true);
        let _ = nested_start(&store, LowerKind::Sync, false);

        let outcome = store
            .internal()
            .run_in_turn(Waker::noop(), |store| {
                SuspendSeam::suspend(store, |_| false)
            })
            .expect("the outer turn runs");

        assert_eq!(
            cause(outcome),
            Error::Scheduler(SchedulerCause::CannotBlock).to_string(),
            "the sync-typed caller nearer the block decides, as the \
             reference and a provider give"
        );
    }

    #[wcmp_macros::test]
    fn it_traps_with_the_cannot_block_cause_above_a_direct_call_whose_caller_must_not_block() {
        let mut owner = store();
        let mut store = owner.internal().context();
        // A fused adapter called the blocked callee directly: its task
        // scope lies right above its caller's, with no mark between.
        // The caller's instance is inside a synchronous call, and the
        // callee's, an `async`-typed callee lifted synchronously, is not.
        current_task(&store, true);
        current_task(&store, false);

        let outcome = store
            .internal()
            .run_in_turn(Waker::noop(), |store| {
                SuspendSeam::suspend(store, |_| false)
            })
            .expect("the outer turn runs");

        assert_eq!(
            cause(outcome),
            Error::Scheduler(SchedulerCause::CannotBlock).to_string(),
            "the caller would wait for the callee it called directly in an \
             instance that must not suspend, and that wait traps"
        );
    }

    #[wcmp_macros::test]
    fn it_traps_with_the_stack_switch_cause_above_a_synchronous_start_whose_callee_resolved() {
        let mut owner = store();
        let mut store = owner.internal().context();
        current_task(&store, false);
        let subtask = nested_start(&store, LowerKind::Sync, false);
        store
            .internal()
            .tables()
            .lock()
            .expect("tables")
            .tasks
            .subtask_returned(subtask)
            .expect("the callee returns");

        let outcome = store
            .internal()
            .run_in_turn(Waker::noop(), |store| {
                SuspendSeam::suspend(store, |_| false)
            })
            .expect("the outer turn runs");

        assert_eq!(
            cause(outcome),
            Error::Scheduler(SchedulerCause::StackSwitchNeeded).to_string(),
            "the callee blocks after its `task.return`, so the synchronous \
             lower below it would return the result and the caller's own \
             code would go on"
        );
    }

    #[wcmp_macros::test]
    fn it_traps_with_the_cannot_block_cause_above_a_nested_start_while_a_synchronous_call_is_in_progress()
     {
        let mut owner = store();
        let mut store = owner.internal().context();
        current_task(&store, false);
        let _ = nested_start(&store, LowerKind::Async, true);

        let outcome = store
            .internal()
            .run_in_turn(Waker::noop(), |store| {
                SuspendSeam::suspend(store, |_| false)
            })
            .expect("the outer turn runs");

        assert_eq!(
            cause(outcome),
            Error::Scheduler(SchedulerCause::CannotBlock).to_string(),
            "the cannot-block rule comes before the nested start: a \
             synchronous call into the blocked task's own instance has not \
             returned, and the reference forbids that on every target"
        );
    }

    #[wcmp_macros::test]
    fn it_traps_with_the_deadlock_cause_once_the_nested_start_has_returned() {
        let mut owner = store();
        let mut store = owner.internal().context();
        current_task(&store, false);
        let subtask = nested_start(&store, LowerKind::Async, false);
        {
            let mut guard = store.internal().tables().lock().expect("tables");
            let callee = guard.tasks.current_task().expect("the callee's task");
            guard.leave_task_scope(callee);
            guard.tasks.end_nested_start(subtask);
        }

        let outcome = store
            .internal()
            .run_in_turn(Waker::noop(), |store| {
                SuspendSeam::suspend(store, |_| false)
            })
            .expect("the outer turn runs");

        assert_eq!(
            cause(outcome),
            Error::Scheduler(SchedulerCause::Deadlock).to_string(),
            "the caller blocks after its callee returned to it, so no frame \
             below the block can move and nothing in the store can meet the \
             condition"
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
        let mut driver = Box::pin(Driver::run(
            store.internal().reborrow(),
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

        let mut driver = Box::pin(Driver::run(
            store.internal().reborrow(),
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
    fn it_traps_with_the_deadlock_cause_while_only_another_instance_is_inside_a_synchronous_call() {
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
            Error::Scheduler(SchedulerCause::Deadlock).to_string(),
            "only the blocked thread's own instance and the instances of \
             the callers below it that wait are read: a synchronous call \
             into another instance does not make this block fail"
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

    /// Park the host task of a synchronous lower in `store`, as the
    /// lower's try part does, with a body that completes on its
    /// `ready_on`th poll. Answers the subtask of the call and the
    /// record of the body's polls.
    fn parked_call(
        store: &mut StoreContext<'_, ()>,
        outer: &Arc<Outer>,
        ready_on: usize,
    ) -> (SubtaskId, Polls) {
        let subtask = store
            .internal()
            .tables()
            .lock()
            .expect("tables")
            .tasks
            .insert_subtask()
            .expect("room under the record cap");
        let polls: Polls = Arc::new(Mutex::new(Vec::new()));
        store
            .internal()
            .scheduler_mut()
            .park_call(HostTask::from_future(
                subtask,
                |_store: &mut StoreContext<'_, ()>, _outcome: Result<Vec<Val>>| Ok(()),
                Probe {
                    outer: outer.clone(),
                    ready_on,
                    polls: polls.clone(),
                },
            ));
        (subtask, polls)
    }

    /// The current thread, which a blocking built-in records its
    /// condition on.
    fn current_thread(store: &StoreContext<'_, ()>) -> ThreadId {
        store
            .internal_ref()
            .tables()
            .lock()
            .expect("tables")
            .tasks
            .current_thread()
            .expect("a current thread")
    }

    /// A thread of a fresh task of `instance`, waiting on the call
    /// a fresh subtask records. Answers the thread and the subtask.
    fn waiting_on_a_call(
        store: &StoreContext<'_, ()>,
        instance: InstanceId,
    ) -> (ThreadId, SubtaskId) {
        let mut guard = store.internal_ref().tables().lock().expect("tables");
        let task = guard
            .tasks
            .create_task(None, None, instance)
            .expect("room under the record cap");
        let thread = guard
            .tasks
            .task(task)
            .expect("the task record")
            .implicit_thread;
        let subtask = guard
            .tasks
            .insert_subtask()
            .expect("room under the record cap");
        guard
            .tasks
            .start_waiting(thread, Readiness::Subtask { subtask })
            .expect("the thread starts waiting");
        (thread, subtask)
    }

    /// An item that resolves the calls `subtasks` record, as the poll
    /// that completes a host task does.
    fn resolves(subtasks: Vec<SubtaskId>) -> Item<()> {
        Item::new(
            ItemKind::TaskStart,
            move |store: &mut StoreContext<'_, ()>| {
                let mut guard = store.internal().lock_tables()?;
                for subtask in &subtasks {
                    guard.tasks.subtask_returned(*subtask)?;
                }
                Ok(())
            },
        )
    }

    /// An item that records the ready threads as the scheduler last
    /// noted them.
    fn reads_ready_threads(seen: &Arc<Mutex<Option<Vec<ThreadId>>>>) -> Item<()> {
        let seen = seen.clone();
        Item::new(
            ItemKind::TaskStart,
            move |store: &mut StoreContext<'_, ()>| {
                let ready = store.internal().lock_tables()?.tasks.ready_threads();
                *seen.lock().expect("record") = Some(ready);
                Ok(())
            },
        )
    }

    #[wcmp_macros::test]
    fn it_traps_with_the_stack_switch_cause_while_the_store_holds_a_parked_call() {
        let mut owner = store();
        let mut store = owner.internal().context();
        let (reached, outer) = outer_waker();
        current_task(&store, false);
        // A body that never completes and asks for a poll every time
        // it is polled.
        let (subtask, polls) = parked_call(&mut store, &reached, usize::MAX);

        let outcome = store
            .internal()
            .run_in_turn(&outer, move |store| {
                SuspendSeam::wait_until(store, Readiness::Subtask { subtask })
            })
            .expect("the outer turn runs");

        assert_eq!(
            cause(outcome),
            Error::Scheduler(SchedulerCause::StackSwitchNeeded).to_string(),
            "the store holds the parked host task of the call, so the \
             reference permits this block and only the target has no \
             provider to serve it"
        );
        assert_eq!(
            polls.lock().expect("polls").clone(),
            vec![true, true, true],
            "the fallback polled the call before its nested turn and after \
             it, the nested turn polled it once as it was woken, and every \
             poll's wake reached the outer turn's waker"
        );
        assert!(
            store.internal().scheduler().holds_host_task(subtask),
            "the call's host task is still among the store's host tasks: \
             taking it out is the finish part's to do"
        );
    }

    #[wcmp_macros::test]
    fn it_evaluates_a_readiness_condition_without_polling_a_host_future_or_running_guest_code() {
        let mut owner = store();
        let mut store = owner.internal().context();
        let (reached, _outer) = outer_waker();
        current_task(&store, false);
        let thread = current_thread(&store);
        let (subtask, polls) = parked_call(&mut store, &reached, 1);
        let log = log();
        store
            .internal()
            .scheduler_mut()
            .push_high_priority(marker(&log, "guest code"));
        let readiness = Readiness::Subtask { subtask };
        store
            .internal()
            .lock_tables()
            .expect("tables")
            .tasks
            .start_waiting(thread, readiness)
            .expect("the thread starts waiting");
        let items_run = store.internal().scheduler().items_run();

        // Every evaluation there is: the condition on its own, the
        // thread's readiness, and the scheduler's evaluation of every
        // waiting thread. A host future ready on its first poll would
        // make the condition hold if any of them polled it.
        let unmet = {
            let mut guard = store.internal().lock_tables().expect("tables");
            let unmet = (
                guard.tasks.readiness_holds(readiness),
                guard.tasks.thread_ready(thread),
            );
            guard.tasks.note_ready_threads();
            (unmet.0, unmet.1, guard.tasks.ready_threads())
        };

        assert_eq!(
            unmet,
            (false, false, Vec::new()),
            "the call is unresolved, and nothing the evaluation did resolved it"
        );
        assert!(
            polls.lock().expect("polls").is_empty(),
            "no evaluation polled the host future the condition waits on"
        );
        assert!(
            entries(&log).is_empty(),
            "no evaluation ran the ready guest code"
        );
        assert_eq!(store.internal().scheduler().items_run(), items_run);
        assert_eq!(store.internal().scheduler().queued_items(), 1);
        assert!(store.internal().scheduler().holds_host_task(subtask));

        // The call resolves the way the poll that completes its body
        // resolves it, and the same evaluations now find it ready.
        let met = {
            let mut guard = store.internal().lock_tables().expect("tables");
            guard
                .tasks
                .subtask_returned(subtask)
                .expect("the call returns");
            let met = (
                guard.tasks.readiness_holds(readiness),
                guard.tasks.thread_ready(thread),
            );
            guard.tasks.note_ready_threads();
            (met.0, met.1, guard.tasks.ready_threads())
        };

        assert_eq!(met, (true, true, vec![thread]));
        assert!(
            polls.lock().expect("polls").is_empty(),
            "a condition that holds is read, not polled"
        );
        assert!(entries(&log).is_empty());
        assert_eq!(store.internal().scheduler().items_run(), items_run);
    }

    #[wcmp_macros::test]
    fn it_evaluates_the_conditions_of_waiting_threads_between_two_items() {
        let mut owner = store();
        let mut store = owner.internal().context();
        let instance = current_task(&store, false);
        let (thread, subtask) = waiting_on_a_call(&store, instance);
        let seen = Arc::new(Mutex::new(None));
        store
            .internal()
            .scheduler_mut()
            .push_high_priority(resolves(vec![subtask]));
        store
            .internal()
            .scheduler_mut()
            .push_high_priority(reads_ready_threads(&seen));

        store.internal().turn(Waker::noop()).expect("a turn");

        assert_eq!(
            seen.lock().expect("record").clone(),
            Some(vec![thread]),
            "the turn evaluated the waiting thread's condition after the item \
             that met it and before the next item ran"
        );
    }

    #[wcmp_macros::test]
    fn it_notes_threads_that_became_ready_in_the_order_they_became_ready() {
        let mut owner = store();
        let mut store = owner.internal().context();
        let instance = current_task(&store, false);
        let (first, first_call) = waiting_on_a_call(&store, instance);
        let (second, second_call) = waiting_on_a_call(&store, instance);
        let (third, third_call) = waiting_on_a_call(&store, instance);
        let seen = Arc::new(Mutex::new(None));
        // The second thread becomes ready alone, and the first and
        // third together, one item later.
        for item in [
            resolves(vec![second_call]),
            resolves(vec![third_call, first_call]),
            reads_ready_threads(&seen),
        ] {
            store.internal().scheduler_mut().push_high_priority(item);
        }

        store.internal().turn(Waker::noop()).expect("a turn");

        assert_eq!(
            seen.lock().expect("record").clone(),
            Some(vec![second, first, third]),
            "the second thread became ready first, and the two that became \
             ready together follow in the order they began to wait"
        );
    }

    #[wcmp_macros::test]
    fn it_records_the_condition_of_a_waiting_thread_until_the_wait_ends() {
        let mut owner = store();
        let mut store = owner.internal().context();
        let instance = current_task(&store, false);
        let thread = current_thread(&store);
        let subtask = store
            .internal()
            .lock_tables()
            .expect("tables")
            .tasks
            .insert_subtask()
            .expect("room under the record cap");
        // What the thread's record held while the thread waited, read
        // by the item that then resolves the call.
        let seen: Seen<Vec<ThreadId>> = Arc::new(Mutex::new(None));
        let recorded = seen.clone();
        queue(
            &mut store,
            instance,
            Item::new(
                ItemKind::TaskStart,
                move |store: &mut StoreContext<'_, ()>| {
                    let mut guard = store.internal().lock_tables()?;
                    let readiness = guard
                        .tasks
                        .thread(thread)
                        .and_then(|record| record.readiness);
                    let waiting = guard.tasks.waiting_threads().to_vec();
                    *recorded.lock().expect("record") = Some((readiness, waiting));
                    guard.tasks.subtask_returned(subtask)
                },
            ),
        );

        let outcome = store
            .internal()
            .run_in_turn(Waker::noop(), move |store| {
                SuspendSeam::wait_until(store, Readiness::Subtask { subtask })
            })
            .expect("the outer turn runs");

        assert_eq!(cause(outcome), "the seam returned with the condition held");
        assert_eq!(
            seen.lock().expect("record").clone(),
            Some((Some(Readiness::Subtask { subtask }), vec![thread])),
            "the try part recorded the condition on the thread's record, and \
             the thread was among the waiting threads while it waited"
        );
        let guard = store.internal().lock_tables().expect("tables");
        assert_eq!(
            guard
                .tasks
                .thread(thread)
                .and_then(|record| record.readiness),
            None,
            "the wait ended when the thread resumed"
        );
        assert!(guard.tasks.waiting_threads().is_empty());
    }

    #[wcmp_macros::test]
    fn it_is_ready_at_once_when_the_condition_already_holds() {
        let mut owner = store();
        let mut store = owner.internal().context();
        current_task(&store, false);
        let thread = current_thread(&store);
        let subtask = {
            let mut guard = store.internal().lock_tables().expect("tables");
            let subtask = guard
                .tasks
                .insert_subtask()
                .expect("room under the record cap");
            guard
                .tasks
                .subtask_returned(subtask)
                .expect("the call returns");
            subtask
        };
        let log = log();
        store
            .internal()
            .scheduler_mut()
            .push_high_priority(marker(&log, "the nested turn"));

        let outcome = store
            .internal()
            .run_in_turn(Waker::noop(), move |store| {
                SuspendSeam::wait_until(store, Readiness::Subtask { subtask })
            })
            .expect("the outer turn runs");

        assert_eq!(cause(outcome), "the seam returned with the condition held");
        assert!(
            entries(&log).is_empty(),
            "the built-in was ready, so no nested turn ran"
        );
        assert!(
            store
                .internal()
                .lock_tables()
                .expect("tables")
                .tasks
                .thread(thread)
                .is_some_and(|record| record.readiness.is_none()),
            "and the thread waits on nothing"
        );
    }

    #[wcmp_macros::test]
    fn it_records_a_yield_as_a_wait_that_always_holds() {
        let mut owner = store();
        let mut store = owner.internal().context();
        let instance = current_task(&store, false);
        let thread = current_thread(&store);
        let seen: Seen<bool> = Arc::new(Mutex::new(None));
        let recorded = seen.clone();
        queue(
            &mut store,
            instance,
            Item::new(
                ItemKind::TaskStart,
                move |store: &mut StoreContext<'_, ()>| {
                    let guard = store.internal().lock_tables()?;
                    let readiness = guard
                        .tasks
                        .thread(thread)
                        .and_then(|record| record.readiness);
                    *recorded.lock().expect("record") =
                        Some((readiness, guard.tasks.thread_ready(thread)));
                    Ok(())
                },
            ),
        );

        store
            .internal()
            .run_in_turn(Waker::noop(), SuspendSeam::give_way)
            .expect("the outer turn runs")
            .expect("the yield returns");

        assert_eq!(
            seen.lock().expect("record").clone(),
            Some((Some(Readiness::Yielded), true)),
            "the yielding thread waited, on a condition that holds, while the \
             ready work it gave way to ran"
        );
        assert!(
            store
                .internal()
                .lock_tables()
                .expect("tables")
                .tasks
                .thread(thread)
                .is_some_and(|record| record.readiness.is_none()),
            "the wait ended when the yield returned"
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
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
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
                    let subtask = store
                        .internal()
                        .lock_tables()?
                        .tasks
                        .insert_subtask()
                        .expect("room under the record cap");
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
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
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
                    let subtask = store
                        .internal()
                        .lock_tables()?
                        .tasks
                        .insert_subtask()
                        .expect("room under the record cap");
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
            outcome = SuspendSeam::give_way(store).map(|_| ());
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
    fn it_names_a_block_after_a_run_of_give_ways_by_the_stores_own_rules() {
        let mut owner = store();
        let mut store = owner.internal().context();
        current_task(&store, false);

        assert_eq!(
            cause(gives_way(&mut store, SPIN_BUDGET)),
            "the seam returned with the condition held",
            "the yields stay inside the budget"
        );
        assert_eq!(
            cause(SuspendSeam::suspend(&mut store, |_| false)),
            Error::Scheduler(SchedulerCause::Deadlock).to_string(),
            "a block starts a run of its own, so the yields before it do not \
             decide its cause: the store is idle, and nothing can meet the \
             condition"
        );
    }

    #[wcmp_macros::test]
    fn it_counts_the_give_ways_of_two_tasks_as_one_run() {
        let mut owner = store();
        let mut store = owner.internal().context();
        let half = SPIN_BUDGET / 2;

        current_task(&store, false);
        assert_eq!(
            cause(gives_way(&mut store, half)),
            "the seam returned with the condition held"
        );
        current_task(&store, false);
        assert_eq!(
            cause(gives_way(&mut store, SPIN_BUDGET + 1 - half)),
            Error::Scheduler(SchedulerCause::StackSwitchNeeded).to_string(),
            "the run is the store's, not a thread's: two tasks that take turns \
             to give way against a store that holds nothing build one run"
        );
    }

    #[wcmp_macros::test]
    fn it_starts_the_run_of_give_ways_over_once_the_budget_has_failed_one() {
        let mut owner = store();
        let mut store = owner.internal().context();

        assert_eq!(
            cause(gives_way(&mut store, SPIN_BUDGET + 1)),
            Error::Scheduler(SchedulerCause::StackSwitchNeeded).to_string(),
            "the run passed the budget"
        );
        assert_eq!(
            cause(SuspendSeam::give_way(&mut store).map(|_| ())),
            "the seam returned with the condition held",
            "the failure ended the run it closed, so the next give way, in \
             whatever call makes it, starts a run of its own"
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
            outcome = SuspendSeam::give_way(&mut store).map(|_| ());
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
