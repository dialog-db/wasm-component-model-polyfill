//! The callback loop of an export lifted `canon lift async`.
//!
//! Such an export does not return its result by returning. Its core
//! function returns a status word, and the polyfill re-enters the
//! export's callback once per event until a word says the task is
//! over. [`CallbackTask`] is the loop: everything the polyfill needs
//! to resume one task, held in a value an item of the store's ready
//! queues can carry, because a queued item outlives the call that
//! started the task.
//!
//! The word's low four bits are the code and its high bits a waitable
//! set index, which is the reference's `unpack_callback_result`. The
//! codes are exit, yield, and wait, and a code above two traps with
//! the unsupported-callback-code cause.
//!
//! - **Exit** ends the task's implicit thread, and the instance it
//!   held exclusively goes back. A task that holds an explicit thread
//!   goes on until its last thread ends. Otherwise the task ends here:
//!   a task that has not returned a result fails with the no-result
//!   cause, and the task's record leaves the store as a synchronous
//!   task's does.
//! - **Yield** gives way. The instance goes back and a callback item
//!   carrying the none event joins the low-priority queue, so it runs
//!   after every other ready item and only once a driver has returned
//!   control to the host executor.
//! - **Wait** names a waitable set in the instance's handle table.
//!   The instance goes back, and a set that already holds an event
//!   queues the callback item at once. A set that holds none parks
//!   the task's implicit thread on the set, and the item waits with
//!   it until a later turn finds the set filled. When no turn ever
//!   does, the call's driver goes idle and fails with the deadlock
//!   cause. Either way the item takes the set's event only when it
//!   runs, which is where the reference takes it, and a set that lost
//!   its event by then parks the task again.
//!
//! A callback item checks the instance before it runs: the exclusive
//! thread is one task's at a time, so an item that finds it taken
//! waits for the holder to release it. Otherwise the item takes the
//! instance, pushes the task as the current scope, calls the callback
//! with the event's three numbers, pops the scope, and hands the word
//! it returned back to this loop.
//!
//! A cancellation request reaches the task through this loop, once,
//! and only while the instance is free for the callback to run. It
//! comes before any other event: an item that runs while a request
//! waits delivers the task-cancelled event in place of the none event
//! a yield left, and in place of the event of the set it waited on,
//! which the set keeps for the next wait. A task waiting on a set
//! takes the request at once, because `subtask.cancel` queues its
//! waiting item, and a task that returns the wait code with a request
//! pending waits for nothing.
//!
//! An error the item raises fails the driver whose turn ran it, not
//! the call that started the task. That is Wasmtime's rule for a task
//! that keeps running after it has returned its result.
//!
//! An item outlives the call that queued it but not the task it
//! resumes. A yield and a wait each leave an item naming the task,
//! and the store's rule is that a task's pending work goes with its
//! record: whichever way the task ends, the item goes with it. The
//! item therefore never has to ask whether the task is still there.

use crate::concurrency::{Event, EventSlot, InstanceId, Item, ItemKind, TaskId};
use crate::error::{Error, Result, TaskCause};
use crate::internal::ErrorInternal;
use crate::resource::TableId;
use crate::runtime_layer::{Func as RuntimeFunc, Val as RuntimeVal};
use crate::store::StoreContext;
use crate::store::StoreContextInternalExt;

use super::async_lift::exit_implicit_thread;
use super::start_call::release_subtask;

/// How many low bits of a status word are the code.
const CODE_BITS: u32 = 4;

/// The mask that takes the code out of a status word.
const CODE_MASK: u32 = (1 << CODE_BITS) - 1;

/// The status word code that ends the task's implicit thread.
const EXIT: u32 = 0;

/// The status word code that gives way.
const YIELD: u32 = 1;

/// The status word code that waits on a waitable set.
const WAIT: u32 = 2;

/// The loop that resumes one call into a callback export.
///
/// The value is `'static` and carries no borrow of the store, so the
/// item that resumes the task can hold a copy of it: dropping the
/// call's future cancels nothing, and the task runs on in the next
/// turn of any driver.
#[derive(Clone)]
pub struct CallbackTask {
    /// The task the call is.
    task: TaskId,
    /// The component instance the export belongs to.
    instance: InstanceId,
    /// The instance's handle table, where the wait status word
    /// resolves its waitable set index.
    table: TableId,
    /// The export's callback, called once per event.
    callback: RuntimeFunc,
}

impl CallbackTask {
    /// Build the loop of one call: `task` is the call, `instance` the
    /// component instance the export belongs to, `table` that
    /// instance's handle table, and `callback` the function the
    /// export's lift named.
    pub fn new(task: TaskId, instance: InstanceId, table: TableId, callback: RuntimeFunc) -> Self {
        Self {
            task,
            instance,
            table,
            callback,
        }
    }

    /// Act on the status word `word` the export's core function or
    /// its callback returned. The module documentation states what
    /// each code does.
    pub fn handle_status_word<T: 'static>(
        &self,
        store: &mut StoreContext<'_, T>,
        word: i32,
    ) -> Result<()> {
        let word = word as u32;
        let code = word & CODE_MASK;
        let set_index = word >> CODE_BITS;
        match code {
            EXIT => self.exit(store),
            YIELD => self.give_way(store),
            WAIT => self.wait(store, set_index),
            _ => {
                self.end(store)?;
                Err(Error::Task(TaskCause::UnsupportedCallbackCode))
            }
        }
    }

    /// End the task's implicit thread, which is the exit code. A task
    /// that has not returned a result fails with the no-result cause,
    /// and its record leaves the store either way.
    fn exit<T: 'static>(&self, store: &mut StoreContext<'_, T>) -> Result<()> {
        exit_implicit_thread(store, self.task)
    }

    /// Give way, which is the yield code: the instance goes back and
    /// the callback item joins the low-priority queue with the none
    /// event.
    fn give_way<T: 'static>(&self, store: &mut StoreContext<'_, T>) -> Result<()> {
        store.internal().release_exclusive_thread(self.task)?;
        let slot = EventSlot::holding(Event::none());
        let item = self.item(slot);
        store.internal().scheduler_mut().push_low_priority(item);
        Ok(())
    }

    /// Wait on the waitable set `set_index` names, which is the wait
    /// code. A set that already holds an event, or a task a
    /// cancellation request waits for, queues the callback item at
    /// once, and the item takes its event as it runs; a set that
    /// holds none parks the task's implicit thread and the item waits
    /// with it.
    fn wait<T: 'static>(&self, store: &mut StoreContext<'_, T>, set_index: u32) -> Result<()> {
        let slot = EventSlot::new();
        let item = self.item(slot.clone());
        let waited = store.internal().wait_callback_on_set(
            self.task,
            self.instance,
            self.table,
            set_index,
            slot,
            item,
        );
        match waited {
            Ok(()) => Ok(()),
            Err(error) => {
                // The word named something that is not a waitable
                // set, so the task can never be resumed: its record
                // leaves the store with the failure.
                self.end(store)?;
                Err(error)
            }
        }
    }

    /// The item that resumes this task with the event `slot` holds.
    /// It is the instance's own work, so a turn held to that instance
    /// runs it, and it is this task's own work, so it goes with the
    /// task's record when that record leaves the store.
    fn item<T: 'static>(&self, slot: EventSlot) -> Item<T> {
        let resumed = self.clone();
        Item::new(
            ItemKind::Callback,
            move |store: &mut StoreContext<'_, T>| resumed.run(store, slot),
        )
        .in_instance(self.instance)
        .for_task(self.task)
    }

    /// Run one callback invocation, which is what the item does.
    ///
    /// The instance is one task's at a time: an item that finds it
    /// taken waits for the holder to release it, carrying the event
    /// it was queued with. Otherwise the item takes the instance,
    /// enters the task, calls the callback, leaves the task, and acts
    /// on the word the callback returned.
    fn run<T: 'static>(&self, store: &mut StoreContext<'_, T>, slot: EventSlot) -> Result<()> {
        if store.internal().instance_is_held(self.instance)? {
            let item = self.item(slot.clone());
            store
                .internal()
                .scheduler_mut()
                .hold_for_exclusive(self.instance, slot, item);
            return Ok(());
        }
        let Some(event) = self.take_event(store, &slot)? else {
            return Ok(());
        };
        store
            .internal()
            .take_exclusive_thread(self.task, self.instance)?;
        let base = store.internal().scope_depth()?;
        store.internal().enter_export_task(self.task)?;
        // The callback is an entry of the task's implicit thread, so
        // it starts through the store's provider when there is one,
        // and the word it returns is acted on when the entry finishes.
        let thread = store.internal().implicit_thread(self.task)?;
        let (code, first, second) = event.triple();
        let arguments = [
            RuntimeVal::I32(code as i32),
            RuntimeVal::I32(first as i32),
            RuntimeVal::I32(second as i32),
        ];
        let resumed = self.clone();
        let finish =
            move |store: &mut StoreContext<'_, T>, called: Result<Vec<RuntimeVal>>| match called
                .and_then(|results| status_word(&results))
            {
                Ok(word) => {
                    store.internal().leave_export_task(resumed.task)?;
                    resumed.handle_status_word(store, word)
                }
                Err(error) => {
                    resumed.abandon(store)?;
                    Err(error)
                }
            };
        store.internal().run_thread_entry(
            thread,
            base,
            &self.callback,
            &arguments,
            vec![RuntimeVal::I32(0)],
            finish,
        )
    }

    /// The event the callback receives on this run, taken as the item
    /// runs, which is where the reference's callback loop takes it.
    ///
    /// A cancellation request the task has not been told of comes
    /// first: the callback receives the task-cancelled event, and a
    /// set the item was queued to take from keeps its event for the
    /// next wait. Otherwise the item takes the next event of that set,
    /// or the event its slot holds. A set that lost its event before
    /// the item ran, because a waitable left it or was dropped, parks
    /// the task on the set again, as the reference's wait goes on
    /// waiting, and `None` says the callback does not run this time.
    /// The set itself cannot go: it counts the task's implicit thread
    /// as a waiter from the moment the item is queued until here, so
    /// `waitable-set.drop` traps in the meantime, as the reference's
    /// does on a set its callback loop still waits on.
    fn take_event<T: 'static>(
        &self,
        store: &mut StoreContext<'_, T>,
        slot: &EventSlot,
    ) -> Result<Option<Event>> {
        let set = slot.take_set();
        let emptied = {
            let mut tables = store.internal().lock_tables()?;
            // The item runs, so the set stops counting the waiter it
            // counted while the item was queued. A set that holds no
            // event below counts it again as the task parks.
            let thread = tables
                .tasks
                .task(self.task)
                .map(|record| record.implicit_thread)
                .ok_or_else(|| Error::internal("a callback task is not in the store"))?;
            tables.tasks.end_queued_wait(thread);
            if tables.tasks.deliver_pending_cancel(self.task) {
                return Ok(Some(Event::task_cancelled()));
            }
            match set {
                None => return Ok(Some(slot.take())),
                Some(set) if tables.tasks.set_has_pending_event(set)? => {
                    return Ok(Some(tables.poll_waitable_set(set)?));
                }
                Some(set) => set,
            }
        };
        let item = self.item(slot.clone());
        store.internal().park_callback_on_set(
            self.task,
            self.instance,
            emptied,
            slot.clone(),
            item,
        )?;
        Ok(None)
    }

    /// Give back what a callee whose callback failed still holds.
    ///
    /// A trap the callback raised, or an exception it did not catch,
    /// ends the callee's side of the call. The callee's task is
    /// abandoned, which ends its implicit thread — so the instance
    /// that thread held exclusively goes back — pops its scope with
    /// no borrow check, and drops whatever the task still had
    /// queued. The caller's record of the call, for a call that came
    /// from another component, goes the way a trap in the callee's
    /// first phase sends it: its resolution is a cancellation, which
    /// gives back every handle the caller lent, and the record and
    /// the caller's entry for it leave the store. The lends are the
    /// subtask's rather than the abandoned task's, under the rule
    /// `HandleTables::lend_to` states, so it is the cancellation
    /// that gives them back and not the task's exit above. A call
    /// that came from the host has no such record, and the driver
    /// whose turn ran the callback takes the failure instead.
    ///
    /// The caller's own task is not ended here. A caller that gave
    /// way to park on the subtask is still parked when the failure
    /// travels out, and its record, its implicit thread and the
    /// waitable set it made stay in the store: what the failure ends
    /// is the driver's turn, not the task the driver was waiting on.
    fn abandon<T: 'static>(&self, store: &mut StoreContext<'_, T>) -> Result<()> {
        let subtask = store
            .internal()
            .lock_tables()?
            .tasks
            .task(self.task)
            .and_then(|record| record.subtask);
        store.internal().abandon_export_task(self.task)?;
        if let Some(subtask) = subtask {
            release_subtask(store, subtask, None);
        }
        Ok(())
    }

    /// Take the task's record out of the store on a failure that
    /// leaves it unresumable, so a driver that already failed does
    /// not leave a task behind that nothing can run.
    fn end<T: 'static>(&self, store: &mut StoreContext<'_, T>) -> Result<()> {
        // The task is ending on a failure that is already the call's,
        // so a borrow the guest did not drop has nothing to be
        // reported to and the count goes with the record.
        let _borrows = store.internal().end_export_task(self.task)?;
        Ok(())
    }
}

/// The status word in the one core result slot an asynchronous
/// export's core function and its callback each return.
pub fn status_word(results: &[RuntimeVal]) -> Result<i32> {
    match results.first() {
        Some(RuntimeVal::I32(word)) => Ok(*word),
        _ => Err(Error::internal(
            "an asynchronous export returned no status word",
        )),
    }
}
