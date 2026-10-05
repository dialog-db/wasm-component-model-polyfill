// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One wait of the suspend seam's fallback, which a plan can carry.

use std::marker::PhantomData;
use std::sync::{Arc, Mutex, PoisonError};

use crate::error::{Error, Result, SchedulerCause};
use crate::internal::ErrorInternal;
use crate::resource::HandleTables;
use crate::store::StoreContext;
use crate::store::StoreContextInternalExt;

use super::instance_id::InstanceId;
use super::outcome::Outcome;
use super::readiness::Readiness;
use super::scheduler::SPIN_BUDGET;
use super::subtask_id::SubtaskId;
use super::suspend_seam::SuspendSeam;
use super::thread_id::ThreadId;

/// One wait of the suspend seam's fallback: the nested turns a
/// blocking built-in runs from inside the guest call that blocked,
/// until its condition holds or nothing can progress.
///
/// The wait is data, so that it can stop between two of its steps and
/// go on later. That is what a provider that resumes a thread only
/// where the store runs no guest code needs: an item a nested turn
/// runs can have to resume a thread, which such a provider cannot do
/// from inside the guest call. The item then leaves the resumption to
/// the store, the nested turn ends there, and [`run`](Self::run)
/// answers `None`. The seam hands the wait to the scheduler as a plan,
/// the thread the built-in runs in suspends, and the scheduler runs
/// the resumption and then the rest of the wait, which continues the
/// nested turn it stopped in. Under every other provider, and with
/// none, nothing is ever left to the store, and a wait runs to its
/// end inside the call.
///
/// The wait records the thread's part in it on the thread's record
/// when it begins, and ends it when the wait is dropped, whether the
/// wait returned, unwound, or went to the scheduler and ended there.
/// A thread that waits on a condition records it, which the
/// scheduler evaluates between two items. A thread that suspended
/// itself is marked suspended, and runs again once a resume names it.
pub struct SeamWait<T: 'static> {
    /// The thread's part in the wait, which ends when this drops.
    _record: Record,
    /// What the wait waits for.
    condition: Condition,
    /// How many nested turns the wait runs.
    turns: Turns,
    /// The synchronous lower whose parked host task the wait polls
    /// before each nested turn, when the wait is one.
    call: Option<SubtaskId>,
    /// The instance whose work alone the nested turns may run, for a
    /// task that must not block.
    only: Option<InstanceId>,
    /// Where the last nested turn stands, when it stopped for the
    /// store to resume a thread.
    open: Open,
    /// How many of the wait's own nested turns in a row the store did
    /// not serve: the part of the seam's run of unserved turns this
    /// wait ran itself, which the budget holds a wait for a condition
    /// to. `None` for a suspension, which the budget holds to the
    /// store's whole run, as it holds a yield: a thread that suspends
    /// again and again in one frame after another is asking again for
    /// what it was refused, and only the run across those frames shows
    /// it.
    own_turns: Option<u32>,
    /// The wait serves the stores of one host data type.
    store: PhantomData<fn(T)>,
}

/// Where the last nested turn of a wait stands.
#[derive(Clone, Copy)]
enum Open {
    /// It ran to its end, or none ran yet.
    Closed,
    /// It stopped for work it left to the store, and goes on with its
    /// next item once that work is done.
    MidTurn,
    /// It stopped for such work in the item that ended it, with this
    /// outcome, which the wait takes once the work is done.
    Ended(Outcome),
}

/// What a wait waits for.
#[derive(Clone, Copy)]
enum Condition {
    /// A readiness condition to hold.
    Holds(Readiness),
    /// A resume to name the thread that suspended itself.
    Resumed(ThreadId),
}

/// How many nested turns a wait runs.
#[derive(Clone, Copy)]
enum Turns {
    /// As many as it takes, until the condition holds or nothing can
    /// progress.
    UntilHeld,
    /// Exactly one, which is a yield's one chance to give way.
    One,
    /// None. The wait was a switch, which is over once the thread it
    /// ran has stopped.
    None,
}

/// A thread's part in a wait, which ends when this drops.
///
/// The record lives behind the store's handle tables, so this holds
/// a handle to them, as a turn's guard does. The lock is read past a
/// poison without clearing it. The panic the wait unwound with can
/// have poisoned it, and the wait ends all the same. Clearing the
/// poison is left to the turn, where the store is handed over.
struct Record {
    tables: Arc<Mutex<HandleTables>>,
    part: Part,
}

/// What a thread records for a wait.
enum Part {
    /// Nothing: the store had no current thread.
    Nothing,
    /// The thread waits on a condition, and the condition it waited
    /// on before goes back when the wait ends.
    Waiting(ThreadId, Option<Readiness>),
    /// The thread suspended itself. When the wait ends it is no
    /// longer suspended, and waits on the condition it waited on
    /// before.
    Suspended(ThreadId, Option<Readiness>),
}

impl Drop for Record {
    fn drop(&mut self) {
        let mut guard = self.tables.lock().unwrap_or_else(PoisonError::into_inner);
        match self.part {
            Part::Nothing => {}
            Part::Waiting(thread, previous) => guard.tasks.stop_waiting(thread, previous),
            Part::Suspended(thread, previous) => {
                if let Some(record) = guard.tasks.thread_mut(thread) {
                    record.suspended = false;
                }
                guard.tasks.stop_waiting(thread, previous);
            }
        }
    }
}

impl<T: 'static> SeamWait<T> {
    /// A wait until `readiness` holds, which is the whole of what the
    /// try part of a blocking built-in asks of the seam. The current
    /// thread of `store` records the condition, and a store with no
    /// current thread records nothing.
    pub fn until(store: &mut StoreContext<'_, T>, readiness: Readiness) -> Result<Self> {
        let record = Self::waiting(store, readiness)?;
        let call = match readiness {
            Readiness::Subtask { subtask } => Some(subtask),
            _ => None,
        };
        Ok(Self::new(
            store,
            record,
            Condition::Holds(readiness),
            Turns::UntilHeld,
            call,
        ))
    }

    /// A yield's one chance to give way: one nested turn. The current
    /// thread records a condition that always holds for as long as
    /// the turn runs.
    pub fn give_way(store: &mut StoreContext<'_, T>) -> Result<Self> {
        let record = Self::waiting(store, Readiness::Yielded)?;
        Ok(Self::new(
            store,
            record,
            Condition::Holds(Readiness::Yielded),
            Turns::One,
            None,
        ))
    }

    /// A yield to a thread a switch runs: the current thread records
    /// a condition that always holds while the switch runs, and the
    /// wait is over once the switch is.
    pub fn yield_to(store: &mut StoreContext<'_, T>) -> Result<Self> {
        let record = Self::waiting(store, Readiness::Yielded)?;
        Ok(Self::new(
            store,
            record,
            Condition::Holds(Readiness::Yielded),
            Turns::None,
            None,
        ))
    }

    /// A suspension of the current thread until a resume names it.
    /// The thread is marked suspended at once, so a switch it makes
    /// next sees it suspended. A store with no current thread has
    /// nothing to suspend, and that is an internal error: every guest
    /// call runs a thread.
    pub fn suspended(store: &mut StoreContext<'_, T>) -> Result<Self> {
        let tables = store.internal().tables_handle();
        let (thread, previous) = {
            let mut guard = store.internal().lock_tables()?;
            let thread = guard
                .tasks
                .current_thread()
                .ok_or_else(|| Error::internal("a thread suspended with no thread running"))?;
            let previous = guard
                .tasks
                .thread(thread)
                .and_then(|record| record.readiness);
            guard.tasks.suspend_thread(thread)?;
            (thread, previous)
        };
        let record = Record {
            tables,
            part: Part::Suspended(thread, previous),
        };
        Ok(Self::new(
            store,
            record,
            Condition::Resumed(thread),
            Turns::UntilHeld,
            None,
        ))
    }

    /// Run the wait, from where it stopped last, until it ends or
    /// stops for the store to resume a thread. It answers what the
    /// wait ended with — `Ok(())` with the condition true, and
    /// otherwise the error the built-in traps with — or `None` when it
    /// stopped. The rules the suspend seam states hold for every turn
    /// it runs.
    pub fn run(&mut self, store: &mut StoreContext<'_, T>) -> Option<Result<()>> {
        match self.turns {
            Turns::None => Some(Ok(())),
            Turns::One => self.run_one(store),
            Turns::UntilHeld => {
                let condition = self.condition;
                let holds = move |store: &StoreContext<'_, T>| condition.holds(store);
                Self::run_open(
                    store,
                    &holds,
                    self.call,
                    self.only,
                    &mut self.open,
                    &mut self.own_turns,
                )
            }
        }
    }

    /// Run nested turns until `condition` holds or nothing can
    /// progress, from inside a guest call that cannot stop for the
    /// store: the loop of the seam's fallback, for a condition that is
    /// not a readiness condition. A turn that stops for the store fails
    /// the wait with the stack-switch cause.
    pub fn run_until(
        store: &mut StoreContext<'_, T>,
        condition: &dyn Fn(&StoreContext<'_, T>) -> bool,
    ) -> Result<()> {
        let mut open = Open::Closed;
        let mut own_turns = Some(0);
        let only = store.internal().must_not_block_instance();
        Self::run_open(store, condition, None, only, &mut open, &mut own_turns)
            .unwrap_or(Err(Error::Scheduler(SchedulerCause::StackSwitchNeeded)))
    }

    /// The loop of a wait that runs nested turns until its condition
    /// holds, from where `open` says the last turn stands. It leaves
    /// `open` saying where the turn stopped when it stops for the
    /// store, answering `None`, and `own_turns` holding the wait's own
    /// part of the seam's run when the budget holds the wait to it.
    fn run_open(
        store: &mut StoreContext<'_, T>,
        condition: &dyn Fn(&StoreContext<'_, T>) -> bool,
        call: Option<SubtaskId>,
        only: Option<InstanceId>,
        open: &mut Open,
        own_turns: &mut Option<u32>,
    ) -> Option<Result<()>> {
        // The waker of the outer turn, so that a wake of a host task
        // polled here reaches the waker the executor already holds.
        // There is none when no turn is running, and a waker that does
        // nothing serves instead, as it does for a trampoline that
        // starts a host task outside a turn.
        let waker = store.internal().active_waker();
        // Whether the seam's budget is what ended the loop. The seam
        // keeps the store's run of turns it did not serve, across
        // frames, and the block is held to its own part of that run,
        // which `own_turns` keeps across the times the wait stops for
        // the store.
        let past_budget;
        loop {
            let outcome = match core::mem::replace(open, Open::Closed) {
                Open::Ended(outcome) => outcome,
                resumed => {
                    let resume = matches!(resumed, Open::MidTurn);
                    if !resume {
                        if condition(store) {
                            return Some(Ok(()));
                        }
                        if let Some(call) = call {
                            if let Err(error) = store.internal().poll_parked_call(call) {
                                return Some(Err(error));
                            }
                            if condition(store) {
                                return Some(Ok(()));
                            }
                        }
                    }
                    let outcome = match store.internal().continue_nested_turn(&waker, only, resume)
                    {
                        Ok(outcome) => outcome,
                        Err(error) => return Some(Err(error)),
                    };
                    if let Some(stopped) = Self::stopped(store, outcome) {
                        *open = stopped;
                        return None;
                    }
                    outcome
                }
            };
            // A wait for a condition is held to its own part of the
            // run: its turns in a row that the store did not serve,
            // counted from its start. The run of the store reaches
            // further back, over the yields before the wait, which are
            // no evidence about it.
            let run = SuspendSeam::note_turn(store);
            let noted = match own_turns {
                Some(own) => {
                    *own = if run == 0 { 0 } else { own.saturating_add(1) };
                    *own > SPIN_BUDGET
                }
                None => run > SPIN_BUDGET,
            };
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
                _ => {
                    past_budget = noted;
                    break;
                }
            }
        }
        // A last turn that met the condition is the wait ending,
        // whatever the budget stands at: the block got what it was
        // waiting for and the call goes on. A synchronous lower's own
        // call gets its last poll first.
        if let Some(call) = call
            && let Err(error) = store.internal().poll_parked_call(call)
        {
            return Some(Err(error));
        }
        if condition(store) {
            return Some(Ok(()));
        }
        if past_budget {
            return Some(Err(SuspendSeam::past_budget(store)));
        }
        // A wait held to one instance is the block of a thread whose
        // own instance must not suspend. Its cause is read from that
        // instance and not from the other instances of the store.
        let cause = match only {
            Some(instance) => store.internal().suspend_cause_in(instance),
            None => store.internal().suspend_cause(),
        };
        Some(Err(Error::Scheduler(cause)))
    }

    /// The one nested turn a yield runs.
    fn run_one(&mut self, store: &mut StoreContext<'_, T>) -> Option<Result<()>> {
        if !matches!(self.open, Open::Ended(_)) {
            let waker = store.internal().active_waker();
            let resume = matches!(self.open, Open::MidTurn);
            let outcome = match store
                .internal()
                .continue_nested_turn(&waker, self.only, resume)
            {
                Ok(outcome) => outcome,
                Err(error) => return Some(Err(error)),
            };
            if let Some(stopped) = Self::stopped(store, outcome) {
                self.open = stopped;
                return None;
            }
        }
        self.open = Open::Closed;
        if SuspendSeam::note_turn(store) > SPIN_BUDGET {
            return Some(Err(SuspendSeam::past_budget(store)));
        }
        Some(Ok(()))
    }

    /// Where the nested turn that just answered `outcome` stands, when
    /// it stopped for work it left to the store: in the middle, or at
    /// its end.
    fn stopped(store: &mut StoreContext<'_, T>, outcome: Outcome) -> Option<Open> {
        if !store.internal().defers_work() {
            return None;
        }
        let at_end = core::mem::take(
            &mut store
                .internal()
                .scheduler_mut()
                .deferred_mut()
                .stopped_at_end,
        );
        Some(if at_end {
            Open::Ended(outcome)
        } else {
            Open::MidTurn
        })
    }

    /// Record that the current thread of `store` waits until
    /// `readiness` holds. A store with no current thread records
    /// nothing.
    fn waiting(store: &mut StoreContext<'_, T>, readiness: Readiness) -> Result<Record> {
        let tables = store.internal().tables_handle();
        let part = {
            let mut guard = store.internal().lock_tables()?;
            match guard.tasks.current_thread() {
                Some(thread) => {
                    Part::Waiting(thread, guard.tasks.start_waiting(thread, readiness)?)
                }
                None => Part::Nothing,
            }
        };
        Ok(Record { tables, part })
    }

    /// A wait of `record`, `condition`, `turns`, and `call`, held to
    /// the current task's own instance when that task must not block.
    /// The instance is read once: what the turns are allowed to run
    /// cannot change under the wait, because the flag is set for the
    /// length of the call the thread is inside.
    fn new(
        store: &mut StoreContext<'_, T>,
        record: Record,
        condition: Condition,
        turns: Turns,
        call: Option<SubtaskId>,
    ) -> Self {
        let own_turns = match condition {
            Condition::Holds(_) => Some(0),
            Condition::Resumed(_) => None,
        };
        Self {
            _record: record,
            condition,
            turns,
            call,
            only: store.internal().must_not_block_instance(),
            open: Open::Closed,
            own_turns,
            store: PhantomData,
        }
    }
}

impl Condition {
    /// Whether the condition holds in `store`. It only reads the
    /// store.
    fn holds<T: 'static>(self, store: &StoreContext<'_, T>) -> bool {
        store
            .internal_ref()
            .lock_tables()
            .is_ok_and(|guard| match self {
                Self::Holds(readiness) => guard.tasks.readiness_holds(readiness),
                Self::Resumed(thread) => !guard.tasks.thread_suspended(thread),
            })
    }
}
