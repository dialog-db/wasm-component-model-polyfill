//! The store's ready queues, its host tasks, and its entry gate.

use core::task::Waker;
use std::collections::{BTreeMap, HashMap, VecDeque};

use super::SuspendSeam;
use super::end_id::EndId;
use super::event_slot::EventSlot;
use super::host_reader::HostReader;
use super::host_task::HostTask;
use super::host_task_set::HostTaskSet;
use super::host_writer::HostWriter;
use super::instance_id::InstanceId;
use super::item::Item;
use super::parked_thread::ParkedThread;
use super::pending_block::PendingBlock;
use super::readiness::Readiness;
use super::subtask_id::SubtaskId;
use super::task_id::TaskId;
use super::task_tables::TaskTables;
use super::thread_id::ThreadId;
use super::waitable_set_id::WaitableSetId;
use crate::error::Result;
use crate::resource::HandleTables;
use crate::value::Val;
use wasm_runtime_layer::Val as RuntimeVal;

/// One task held at an instance's entry gate.
///
/// A task that waits at the gate is a queued item, not a suspended
/// thread: its implicit thread has not run yet, so the gate needs no
/// stack switch.
struct GateEntry<T: 'static> {
    task: TaskId,
    instance: InstanceId,
    needs_exclusive: bool,
    item: Item<T>,
}

/// One callback item the scheduler holds until the store can run it.
///
/// A held item is not ready: a turn that finds nothing but held
/// items goes idle, and the driver that polled it fails with the
/// cause an idle store gives. That is the deadlock trap of a
/// callback task that waits on a set no turn ever fills.
struct HeldCallback<T: 'static> {
    /// The component instance the item's task belongs to.
    instance: InstanceId,
    /// What the item waits for.
    condition: HeldFor,
    /// Where the event the item receives is left for it.
    slot: EventSlot,
    /// The item itself.
    item: Item<T>,
}

/// What a held callback item waits for.
#[derive(Clone, Copy)]
enum HeldFor {
    /// An event on the waitable set the task's implicit thread is
    /// parked on. The item is queued when a waitable of the set
    /// holds an event, carrying that event.
    Event {
        /// The thread parked on the set, whose wait ends as the item
        /// is queued.
        thread: ThreadId,
        /// The set the thread waits on.
        set: WaitableSetId,
    },
    /// The instance's exclusive thread, which another task holds.
    /// The item is queued when the holder releases it.
    ExclusiveThread,
}

/// The held callback items, in the order they were held, with the
/// indexes that say which of them a turn has to look at.
///
/// A turn does not examine every held item. An item held for an
/// event is looked at only when its set has been signalled — a
/// waitable of the set took on an event, or one holding an event
/// joined it — which the task tables record as it happens. An item
/// held for the exclusive thread is looked at every time, because a
/// release of the instance leaves no such record; those are the
/// callbacks that met another task inside their instance, never the
/// ones parked waiting for an event. An item is also looked at once
/// on the first turn after it was held, whatever was signalled.
///
/// Each item carries the number it was held under, and the items a
/// turn looks at are looked at in that order, so what is released
/// together is queued in the order it was held.
struct HeldCallbacks<T: 'static> {
    entries: BTreeMap<u64, HeldCallback<T>>,
    next: u64,
    /// The items held for an event, by the set they wait on.
    by_set: HashMap<WaitableSetId, Vec<u64>>,
    /// The items held for the exclusive thread.
    exclusive: Vec<u64>,
    /// The items held since the last look, which the next one
    /// examines whatever was signalled.
    fresh: Vec<u64>,
}

impl<T: 'static> HeldCallbacks<T> {
    fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            next: 0,
            by_set: HashMap::new(),
            exclusive: Vec::new(),
            fresh: Vec::new(),
        }
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Hold `entry` behind every item already held.
    fn hold(&mut self, entry: HeldCallback<T>) {
        let number = self.next;
        self.next += 1;
        match entry.condition {
            HeldFor::Event { set, .. } => self.by_set.entry(set).or_default().push(number),
            HeldFor::ExclusiveThread => self.exclusive.push(number),
        }
        self.fresh.push(number);
        self.entries.insert(number, entry);
    }

    /// Take the item held under `number` out, and out of the index
    /// that named it.
    fn remove(&mut self, number: u64) -> Option<HeldCallback<T>> {
        let entry = self.entries.remove(&number)?;
        match entry.condition {
            HeldFor::Event { set, .. } => {
                if let Some(numbers) = self.by_set.get_mut(&set) {
                    numbers.retain(|held| *held != number);
                    if numbers.is_empty() {
                        self.by_set.remove(&set);
                    }
                }
            }
            HeldFor::ExclusiveThread => self.exclusive.retain(|held| *held != number),
        }
        Some(entry)
    }

    /// The numbers of the items a turn has to look at, given the sets
    /// signalled since the last look, in the order the items were
    /// held.
    fn take_to_examine(&mut self, signalled: &[WaitableSetId]) -> Vec<u64> {
        let mut numbers = core::mem::take(&mut self.fresh);
        numbers.extend_from_slice(&self.exclusive);
        for set in signalled {
            if let Some(held) = self.by_set.get(set) {
                numbers.extend_from_slice(held);
            }
        }
        numbers.sort_unstable();
        numbers.dedup();
        numbers
    }
}

/// The store's ready queues, its host tasks, and its entry gate.
///
/// The scheduler is a cooperative loop the store owns. It has no
/// thread of its own and no executor of its own: a driver polls it,
/// one poll is a turn, and a turn runs the items that are ready. The
/// queues resolve the order the specification leaves open, the way
/// Wasmtime resolves it:
///
/// - The switch slot holds the one thread the scheduler must switch
///   to next, which is the callee thread of a call between two
///   components. It runs before anything else.
/// - The high-priority queue holds fresh readiness, in the order it
///   became ready.
/// - The low-priority queue holds resumptions after a yield. A yield
///   always gives way: its item resumes after every other ready item,
///   and its resumption first returns control to the host executor.
/// - The resume-after-yield slot holds the item a previous turn took
///   out of the low-priority queue. It runs at the top of the next
///   turn.
///
/// The host tasks sit here too. A host task's future is `'static`
/// and does not borrow the store, but it is not `Send` in the
/// browser, and the store's handle tables are, so it cannot live
/// beside them.
///
/// An item queued for a task is that task's pending work, and a
/// task's pending work goes with its record. When a task's record
/// leaves the store, every item that names that task is dropped
/// wherever it waits — in a ready queue, in the switch or
/// resume-after-yield slot, at an entry gate, or among the held
/// callbacks — and whatever holding it reserved is given back:
/// [`discard_task_items`](Scheduler::discard_task_items) is that
/// sweep, and it is the second half of ending a task.
///
/// The rule is a drop rather than a check at the point an item runs
/// because an item does not only run. It also reserves: a start item
/// at the gate counts against the instance's tally of tasks waiting
/// to enter, and a held callback keeps the task's implicit thread
/// parked on a waitable set. An item that merely returned early when
/// it found no record would leave both reservations standing, and
/// the gate would stay shut against every later call of that
/// instance. Dropping the item is also what keeps guest code from
/// running for a task the store no longer has: a callback item runs
/// the export's callback before anything it does could notice the
/// record is gone.
///
/// Three ends sweep, and they are the three the store's context
/// offers for the task of an export's call: the success exit, the
/// end of a callback task parked between events, and the failure
/// abandon. Each sweeps only when the store's records really ended
/// the task it named. A task whose scope is not on the stack is not
/// ended by an end that names it, and what such a task has queued is
/// its own pending work still, so sweeping it would drop the work of
/// a task that is still to run. The ordinary end is the exit status
/// word, where the task has nothing queued and the sweep finds
/// nothing. The end that makes the rule necessary is a call that
/// fails while its callee is parked: the failure ends the callee's
/// task, and the item the callee left behind would otherwise outlive
/// it.
///
/// Three other ends take a task's record out of the store and sweep
/// nothing, and need not: the exit intrinsic of a synchronous call
/// between two components, through `exit_current_task`; the drop of
/// the guard a call the polyfill itself makes into a guest is held
/// by, which for the two of those calls that run as a task is a
/// `cabi_realloc` or a destructor; and the drop of the guard a core
/// module's `start` function runs under. No item ever names one of
/// those tasks. An item names a task only when it is the start or
/// the callback of an export's call, which is the one shape the
/// store queues by task; each of these three is pushed and popped
/// inside a single call on the caller's own stack, reaching the
/// scheduler for nothing, so there is never an item of theirs to
/// drop.
///
/// One last removal is not an end at all: the record a caller's
/// subtask entry keeps alive leaves when `subtask.drop` takes that
/// entry, and the task it names ended — and was swept — back when
/// its last thread ended.
///
/// Beside the host tasks sit the ends the host serves, keyed by the
/// end's identity: the writable ends it serves through producers and
/// the readable ends it serves through consumers. A guest's read or
/// write on such an end's stream or future is a host task while the
/// producer or consumer is pending, and the producer or consumer
/// itself stays here between copies.
///
/// The suspend capability is named here too. It is the seam a
/// blocking built-in asks to suspend the current guest thread, and
/// the slot a target fills to serve that block by switching stacks.
///
/// The turn itself lives on the store, because running an item needs
/// the store the item runs against. This type holds what the turn
/// chooses between.
pub struct Scheduler<T: 'static> {
    switch_slot: Option<Item<T>>,
    high_priority: VecDeque<Item<T>>,
    low_priority: VecDeque<Item<T>>,
    resume_after_yield: Option<Item<T>>,
    entry_gate: VecDeque<GateEntry<T>>,
    held_callbacks: HeldCallbacks<T>,
    host_tasks: HostTaskSet<T>,
    /// The key each host task a synchronous lower parked is held
    /// under, by the subtask of its call.
    parked_calls: HashMap<SubtaskId, u64>,
    /// What the host task of a synchronous lower produced, with the
    /// task itself, by the subtask of its call, from the poll that
    /// completed it to the finish part of the lower that takes it.
    settled_calls: HashMap<SubtaskId, (HostTask<T>, Result<Vec<Val>>)>,
    host_writers: HashMap<EndId, Box<dyn HostWriter<T>>>,
    host_readers: HashMap<EndId, Box<dyn HostReader<T>>>,
    /// The waker of the last poll of each host end that answered
    /// pending, which a cancel of the guest's copy wakes.
    host_end_wakers: HashMap<EndId, Waker>,
    suspend_seam: SuspendSeam<T>,
    resumptions: u64,
    items_run: u64,
    nested_turns: u64,
    /// How many times threads have suspended in the provider, ever,
    /// which numbers each suspension in that order.
    parked_count: u64,
    /// The threads suspended in the provider, until they resume.
    parked: HashMap<ThreadId, ParkedThread<T>>,
    /// The blocking built-in each thread suspended in the provider
    /// waits in.
    blocks: HashMap<ThreadId, PendingBlock<T>>,
    /// The results of a blocking built-in whose try part found it
    /// done, from the try to the finish the shim calls next.
    ready_block: Option<Vec<RuntimeVal>>,
    /// The thread a thread that just suspended in the provider named
    /// to run next, from the try part of its switch to the frame that
    /// resumed it.
    next_thread: Option<ThreadId>,
}

/// How many nested turns in a row the suspend seam runs against a
/// store that does nothing of its own before it decides the
/// suspension can never be served.
///
/// Two shapes reach it, and they are one shape: a thread that gives
/// way again and again against a store that holds nothing, and a
/// block whose turns keep re-running one yielded item. Both are a
/// callee only a caller further down the stack can release, which
/// on a target with no stack switch is a caller the store cannot
/// reach.
///
/// The number is a budget, not a proof. Nothing short of running
/// the guest to its end tells a loop that gives way this many times
/// and then returns from one that never returns at all, so the
/// budget is drawn generously: a converging loop of any ordinary
/// length finishes well inside it, and a spinning one reaches its
/// failure in a bounded number of turns rather than running for
/// ever. It is the polyfill's own number, and the reference states
/// no such bound.
pub const SPIN_BUDGET: u32 = 64;

impl<T: 'static> Scheduler<T> {
    /// Construct a scheduler with nothing queued.
    pub fn new() -> Self {
        Self {
            switch_slot: None,
            high_priority: VecDeque::new(),
            low_priority: VecDeque::new(),
            resume_after_yield: None,
            entry_gate: VecDeque::new(),
            held_callbacks: HeldCallbacks::new(),
            host_tasks: HostTaskSet::new(),
            parked_calls: HashMap::new(),
            settled_calls: HashMap::new(),
            host_writers: HashMap::new(),
            host_readers: HashMap::new(),
            host_end_wakers: HashMap::new(),
            suspend_seam: SuspendSeam::new(),
            resumptions: 0,
            items_run: 0,
            nested_turns: 0,
            parked_count: 0,
            parked: HashMap::new(),
            blocks: HashMap::new(),
            ready_block: None,
            next_thread: None,
        }
    }

    /// Count one item the store has run. A turn spells this before
    /// it runs an item, so the count is what says whether the store
    /// got anywhere between two moments.
    pub fn note_item_run(&mut self) {
        self.items_run = self.items_run.saturating_add(1);
    }

    /// How many items the store has run, ever. A block reads it
    /// beside [`Scheduler::resumptions`] to tell a turn that ran
    /// work of its own from one that ran nothing but resumptions
    /// after a yield.
    pub fn items_run(&self) -> u64 {
        self.items_run
    }

    /// Whether a host future that can still resolve is pending, which
    /// is whether the store holds a host task. The future of a
    /// synchronous lower counts through that alone, because the lower
    /// parks its future among the store's host tasks.
    ///
    /// A store holding one moves on its own when its executor polls
    /// it again, so a nested turn that ran nothing against such a
    /// store has not said that nothing can ever run. That is what
    /// keeps a thread waiting on a host future out of the seam's
    /// budget.
    pub fn host_future_pending(&self) -> bool {
        !self.host_tasks.is_empty()
    }

    /// Count one nested turn the suspend seam ran.
    pub fn note_nested_turn(&mut self) {
        self.nested_turns = self.nested_turns.saturating_add(1);
    }

    /// How many nested turns the suspend seam has run, ever. A
    /// suspension a provider serves runs none.
    pub fn nested_turns(&self) -> u64 {
        self.nested_turns
    }

    /// Keep `thread`, which suspended in the provider, until it
    /// resumes. Each suspension takes the next number of the store's
    /// order of suspension, a thread that suspends again included.
    pub fn park_thread(&mut self, thread: ThreadId, mut parked: ParkedThread<T>) {
        self.parked_count = self.parked_count.saturating_add(1);
        parked.number = self.parked_count;
        self.parked.insert(thread, parked);
    }

    /// The threads suspended in the provider, in the order they last
    /// suspended.
    pub fn parked_in_order(&self) -> Vec<ThreadId> {
        let mut parked: Vec<(u64, ThreadId)> = self
            .parked
            .iter()
            .map(|(thread, parked)| (parked.number, *thread))
            .collect();
        parked.sort_unstable_by_key(|(number, _)| *number);
        parked.into_iter().map(|(_, thread)| thread).collect()
    }

    /// Take `thread` out of the parked threads to resume it. `None`
    /// when it is not parked, which is the case for a thread whose
    /// task ended while it waited.
    pub fn take_parked_thread(&mut self, thread: ThreadId) -> Option<ParkedThread<T>> {
        self.parked.remove(&thread)
    }

    /// Whether `thread` is suspended in the provider.
    pub fn is_parked(&self, thread: ThreadId) -> bool {
        self.parked.contains_key(&thread)
    }

    /// How many threads are suspended in the provider.
    pub fn parked_threads(&self) -> usize {
        self.parked.len()
    }

    /// Mark the resumption of the parked `thread` queued, and answer
    /// its task and the number of the suspension it resumes. `None`
    /// when the thread is not parked or its resumption is queued
    /// already.
    pub fn queue_resumption(&mut self, thread: ThreadId) -> Option<(TaskId, u64)> {
        let parked = self.parked.get_mut(&thread)?;
        if parked.queued {
            return None;
        }
        parked.queued = true;
        Some((parked.task, parked.number))
    }

    /// The number of the suspension `thread` is parked in, when it is
    /// parked.
    pub fn parked_number(&self, thread: ThreadId) -> Option<u64> {
        self.parked.get(&thread).map(|parked| parked.number)
    }

    /// Name `thread` as the one to run next, once the thread that is
    /// suspending now has left the real stack. This is the switch of
    /// the reference's `Thread.resume` loop: the frame that resumed
    /// the switching thread runs the named thread before anything
    /// else.
    pub fn name_next_thread(&mut self, thread: ThreadId) {
        self.next_thread = Some(thread);
    }

    /// Take the thread a switch named to run next.
    pub fn take_next_thread(&mut self) -> Option<ThreadId> {
        self.next_thread.take()
    }

    /// Record the blocking built-in `thread` waits in, from the try
    /// part that began the wait to the finish part that ends it.
    pub fn begin_block(&mut self, thread: ThreadId, block: PendingBlock<T>) {
        self.blocks.insert(thread, block);
    }

    /// The condition of the blocking built-in `thread` waits in,
    /// when it waits in one.
    pub fn block_readiness(&self, thread: ThreadId) -> Option<Readiness> {
        self.blocks.get(&thread).map(|block| block.readiness)
    }

    /// Take the blocking built-in `thread` waits in, to finish it.
    pub fn end_block(&mut self, thread: ThreadId) -> Option<PendingBlock<T>> {
        self.blocks.remove(&thread)
    }

    /// Keep the results of a blocking built-in whose try part found
    /// it done, for the finish part the shim calls next.
    pub fn keep_ready_block(&mut self, values: Vec<RuntimeVal>) {
        self.ready_block = Some(values);
    }

    /// Take the results a try part kept.
    pub fn take_ready_block(&mut self) -> Option<Vec<RuntimeVal>> {
        self.ready_block.take()
    }

    /// The store's one suspend capability: the seam a blocking
    /// built-in asks to suspend the current guest thread until a
    /// readiness condition holds.
    pub fn suspend_seam(&self) -> &SuspendSeam<T> {
        &self.suspend_seam
    }

    /// The store's one suspend capability, mutably, which is where
    /// the seam keeps its count of the nested turns the store did not
    /// serve.
    pub fn suspend_seam_mut(&mut self) -> &mut SuspendSeam<T> {
        &mut self.suspend_seam
    }

    /// Give `task` to the store. A host task that joined since the
    /// last turn counts as woken, so the next turn polls it.
    pub fn push_host_task(&mut self, task: HostTask<T>) {
        self.host_tasks.push(task);
    }

    /// Park the host task of a synchronous lower among the store's
    /// host tasks, where turns poll it as they poll every other. It
    /// counts as woken, as every task that joins does. A task that
    /// resolves no subtask is not a call, and joins as any other
    /// host task does.
    pub fn park_call(&mut self, task: HostTask<T>) {
        let subtask = task.subtask();
        let key = self.host_tasks.push(task);
        if let Some(subtask) = subtask {
            self.parked_calls.insert(subtask, key);
        }
    }

    /// Whether the host task of the call `subtask` records is one a
    /// synchronous lower parked, and still pending.
    pub fn is_parked_call(&self, subtask: SubtaskId) -> bool {
        self.parked_calls.contains_key(&subtask)
    }

    /// Take out the parked host task of the call `subtask` records,
    /// with its key and the waker to poll it with. `None` when no
    /// such task is parked, or when a turn has it out. The task goes
    /// back through [`restore_host_task`](Self::restore_host_task),
    /// or leaves through [`complete_host_task`](Self::complete_host_task).
    pub fn take_parked_call(&mut self, subtask: SubtaskId) -> Option<(u64, Waker, HostTask<T>)> {
        let key = *self.parked_calls.get(&subtask)?;
        let (waker, task) = self.host_tasks.take(key)?;
        Some((key, waker, task))
    }

    /// Keep what the parked host task of the call `subtask` records
    /// produced, with the task, for the finish part of the lower that
    /// waits on it. The task is no longer parked.
    pub fn settle_call(
        &mut self,
        subtask: SubtaskId,
        task: HostTask<T>,
        outcome: Result<Vec<Val>>,
    ) {
        self.parked_calls.remove(&subtask);
        self.settled_calls.insert(subtask, (task, outcome));
    }

    /// Take what the parked host task of the call `subtask` records
    /// produced, with the task, once a poll has completed it.
    pub fn take_settled_call(
        &mut self,
        subtask: SubtaskId,
    ) -> Option<(HostTask<T>, Result<Vec<Val>>)> {
        self.settled_calls.remove(&subtask)
    }

    /// Take the parked host task of the call `subtask` records out of
    /// the store for good, pending or settled, without polling it:
    /// the lower that waited on it failed, and nothing will lower
    /// what it produces.
    pub fn withdraw_call(&mut self, subtask: SubtaskId) {
        if let Some(key) = self.parked_calls.remove(&subtask) {
            self.host_tasks.remove(key);
        }
        self.settled_calls.remove(&subtask);
    }

    /// Whether the store holds the host task of the call `subtask`
    /// records among its host tasks.
    pub fn holds_host_task(&self, subtask: SubtaskId) -> bool {
        self.host_tasks.holds(subtask)
    }

    /// Record `waker`, the waker of the driver polling the store, as
    /// the one a host task's wake is passed on to.
    pub fn watch_host_tasks(&self, waker: &Waker) {
        self.host_tasks.watch(waker);
    }

    /// Take out the host tasks woken since the last take, in the
    /// order they were woken, so a turn can poll them while it holds
    /// the store. Each comes with its key and the waker to poll it
    /// with; a task that is still pending goes back through
    /// [`restore_host_task`](Self::restore_host_task) and one that
    /// completed leaves through
    /// [`complete_host_task`](Self::complete_host_task).
    pub fn take_woken_host_tasks(&mut self) -> Vec<(u64, Waker, HostTask<T>)> {
        self.host_tasks.take_woken()
    }

    /// Put back a host task that is still pending.
    pub fn restore_host_task(&mut self, key: u64, task: HostTask<T>) {
        self.host_tasks.restore(key, task);
    }

    /// Let go of a host task that completed.
    pub fn complete_host_task(&mut self, key: u64) {
        self.host_tasks.complete(key);
    }

    /// Take every host task out, woken or not.
    pub fn take_host_tasks(&mut self) -> Vec<HostTask<T>> {
        self.host_tasks.take_all()
    }

    /// How many host tasks the store holds.
    pub fn host_task_count(&self) -> usize {
        self.host_tasks.len()
    }

    /// Hold `writer` as the writable end `end` the host serves. The
    /// end records cannot hold it, because it is polled with the
    /// store's context and they know nothing of the host data.
    pub fn insert_host_writer(&mut self, end: EndId, writer: Box<dyn HostWriter<T>>) {
        self.host_writers.insert(end, writer);
    }

    /// Take out the writable end `end` the host serves, so that it can
    /// be polled with the store lent to its producer. One that is
    /// still serving goes back through
    /// [`insert_host_writer`](Self::insert_host_writer).
    pub fn take_host_writer(&mut self, end: EndId) -> Option<Box<dyn HostWriter<T>>> {
        self.host_writers.remove(&end)
    }

    /// Whether the scheduler holds the producer of the writable end
    /// `end` the host serves. It does not once a pipe of the host's
    /// own took the producer over, or while a poll has it out.
    pub fn holds_host_writer(&self, end: EndId) -> bool {
        self.host_writers.contains_key(&end)
    }

    /// Let go of the writable end `end` the host serves, for good:
    /// take out its producer, if the scheduler still holds it, and
    /// forget the waker kept for it. The caller drops the producer
    /// once it holds no lock, because the drop runs host code.
    pub fn release_host_writer(&mut self, end: EndId) -> Option<Box<dyn HostWriter<T>>> {
        self.host_end_wakers.remove(&end);
        self.host_writers.remove(&end)
    }

    /// Hold `reader` as the readable end `end` the host serves. The
    /// end records cannot hold it, for the reason
    /// [`insert_host_writer`](Self::insert_host_writer) states.
    pub fn insert_host_reader(&mut self, end: EndId, reader: Box<dyn HostReader<T>>) {
        self.host_readers.insert(end, reader);
    }

    /// Take out the readable end `end` the host serves, so that it can
    /// be polled with the store lent to its consumer. One that is
    /// still serving goes back through
    /// [`insert_host_reader`](Self::insert_host_reader).
    pub fn take_host_reader(&mut self, end: EndId) -> Option<Box<dyn HostReader<T>>> {
        self.host_readers.remove(&end)
    }

    /// Let go of the readable end `end` the host serves, for good, as
    /// [`release_host_writer`](Self::release_host_writer) lets go of a
    /// writable one.
    pub fn release_host_reader(&mut self, end: EndId) -> Option<Box<dyn HostReader<T>>> {
        self.host_end_wakers.remove(&end);
        self.host_readers.remove(&end)
    }

    /// Keep `waker`, the waker of a poll of the host end `end` that
    /// answered pending, in place of any kept before. It is the one
    /// Wasmtime calls the cancel waker: a cancel of the guest's copy
    /// wakes it, so that the host task polls the producer or consumer
    /// again, asked to finish.
    pub fn set_host_end_waker(&mut self, end: EndId, waker: Waker) {
        self.host_end_wakers.insert(end, waker);
    }

    /// Take the waker kept for the host end `end`, if a poll of it
    /// answered pending since the last one was taken.
    pub fn take_host_end_waker(&mut self, end: EndId) -> Option<Waker> {
        self.host_end_wakers.remove(&end)
    }

    /// Put `item` in the switch slot: the scheduler runs it before
    /// anything else. The slot holds one item, so a second switch
    /// before the first has run sends the first to the front of the
    /// high-priority queue rather than dropping it.
    pub fn switch_to(&mut self, item: Item<T>) {
        if let Some(displaced) = self.switch_slot.replace(item) {
            self.high_priority.push_front(displaced);
        }
    }

    /// Queue `item` as fresh readiness.
    pub fn push_high_priority(&mut self, item: Item<T>) {
        self.high_priority.push_back(item);
    }

    /// Queue `item` as a resumption after a yield. It runs after
    /// every other ready item, and the driver returns control to the
    /// host executor before it runs.
    pub fn push_low_priority(&mut self, item: Item<T>) {
        self.low_priority.push_back(item);
    }

    /// Start the implicit thread of `task`, which belongs to
    /// `instance`, by queueing `item` as fresh readiness once the
    /// entry gate lets the task through. The gate rules are the ones
    /// [`past_entry_gate`](Self::past_entry_gate) states.
    ///
    /// This is the whole of the polyfill's reentrance rule, because
    /// the gate is the only thing a call into an instance that is
    /// already on the stack meets. No call traps for reentrance:
    /// nothing here raises `CannotEnterComponent`, and the fused
    /// adapters of Wasmtime 49 emit none either, for a call between
    /// a parent and a child or into the caller's own instance. The
    /// five rules the gate carries:
    ///
    /// - A sync-typed callee can be entered at any depth — from a
    ///   child, a parent, a sibling, a destructor, or the host. Its
    ///   task ignores the gate, and the synchronous baseline serves
    ///   the call as a nested call on the real stack.
    /// - An async-typed callee lifted synchronously or with a
    ///   callback needs the exclusive thread of its instance, so a
    ///   reentrant call into it waits at the gate while the holder
    ///   runs core code.
    /// - A callback holder releases the instance between events, so
    ///   the call waiting at the gate proceeds when the holder waits
    ///   or gives way. A synchronous holder releases only on return,
    ///   so a cycle back through it never opens the gate: the store
    ///   goes idle and the block fails with the deadlock cause.
    /// - A task the gate holds is a subtask in its starting state,
    ///   so an asynchronous lower answers `STARTING`, and a wait on
    ///   that subtask with nothing else ready fails with the
    ///   deadlock cause too. Backpressure holds a task the same way
    ///   and reads the same.
    /// - The host can always enter. From the host, reentrance while
    ///   an instance is on the stack is reachable only through
    ///   `call_concurrent`, and the gate treats it like any other
    ///   call. Wasmtime refuses a host entry only into a store a
    ///   trap poisoned, which is not a rule the gate carries.
    ///
    /// The may-leave flag is a separate thing and unchanged by any
    /// of this: a lowered import called from a `realloc` or a
    /// `post-return` fails with the cannot-leave cause wherever the
    /// gate would have let it through.
    pub fn enter_implicit_thread(
        &mut self,
        tables: &mut TaskTables,
        task: TaskId,
        instance: InstanceId,
        async_function: bool,
        needs_exclusive: bool,
        item: Item<T>,
    ) {
        if let Some(item) = self.past_entry_gate(
            tables,
            task,
            instance,
            async_function,
            needs_exclusive,
            item,
        ) {
            self.high_priority.push_back(item);
        }
    }

    /// Start the implicit thread of `task` with the item the switch
    /// slot holds, which is what a call between two components does:
    /// the callee runs next, from inside the trampoline the caller
    /// is in.
    ///
    /// The gate rules are the ones
    /// [`enter_implicit_thread`](Self::enter_implicit_thread)
    /// states. A task the gate lets through leaves its item in the
    /// switch slot, so the caller's trampoline runs it. A task the
    /// gate holds clears the slot: the item is not ready, and it
    /// waits at the gate in arrival order like any other. Does
    /// nothing when the slot is empty.
    pub fn enter_implicit_thread_in_switch_slot(
        &mut self,
        tables: &mut TaskTables,
        task: TaskId,
        instance: InstanceId,
        async_function: bool,
        needs_exclusive: bool,
    ) {
        let Some(item) = self.switch_slot.take() else {
            return;
        };
        self.switch_slot = self.past_entry_gate(
            tables,
            task,
            instance,
            async_function,
            needs_exclusive,
            item,
        );
    }

    /// Take `item` through the entry gate of `instance`: hand it
    /// back when the gate lets the task through, and queue it at the
    /// gate when the gate holds it.
    ///
    /// This is the reference's `enter_implicit_thread`. A task of a
    /// synchronous export of a synchronous function ignores the gate
    /// and becomes ready at once, as the reference states. A task of
    /// an `async`-typed function waits at the gate when the
    /// instance's backpressure is set, when it needs the exclusive
    /// thread and one is set, or when tasks are already waiting —
    /// the last so that a fresh arrival cannot overtake a task that
    /// is already queued. `needs_exclusive` is the reference's `not
    /// opts.async or opts.callback`: a task lifted synchronously and
    /// a callback task each need the instance to themselves.
    fn past_entry_gate(
        &mut self,
        tables: &mut TaskTables,
        task: TaskId,
        instance: InstanceId,
        async_function: bool,
        needs_exclusive: bool,
        item: Item<T>,
    ) -> Option<Item<T>> {
        // The gate is the one place that knows which instance a
        // task's start belongs to, so it is where the item learns
        // it. A turn run for a task that must not block asks each
        // item that question.
        let item = item.in_instance(instance);
        if !async_function {
            return Some(item);
        }
        let waiting = tables
            .instance(instance)
            .map(|record| record.waiting_to_enter)
            .unwrap_or(0);
        if Self::blocked(tables, instance, needs_exclusive) || waiting > 0 {
            if let Some(record) = tables.instance_mut(instance) {
                record.waiting_to_enter += 1;
            }
            self.entry_gate.push_back(GateEntry {
                task,
                instance,
                needs_exclusive,
                item,
            });
            return None;
        }
        Self::claim_exclusive(tables, task, instance, needs_exclusive);
        Some(item)
    }

    /// Take the item the switch slot holds, which is what the
    /// trampoline of a call between two components runs. `None` when
    /// the entry gate took the item instead.
    pub fn take_switch_slot(&mut self) -> Option<Item<T>> {
        self.switch_slot.take()
    }

    /// Release every task the entry gate can let through, in arrival
    /// order. A task whose instance is still blocked stays, and so
    /// does every task of that instance behind it, so the tasks of
    /// one instance start in the order they arrived.
    ///
    /// This is the other half of the reentrance rule
    /// [`enter_implicit_thread`](Self::enter_implicit_thread)
    /// states: the gate is the only serialization, and this is
    /// where it stops serializing. A callback holder reaches here
    /// between two of its events and a synchronous holder on its
    /// return, and each releases whichever reentrant call was
    /// waiting on the instance it gave back. A cycle that no holder
    /// ever releases leaves its task here for the life of the
    /// store, which is what the deadlock cause reports when the
    /// turns run out of anything else to do.
    ///
    /// A task released here is fresh readiness of the turn that
    /// released it, and a turn hands it to the turn that follows
    /// rather than running it: a call that already has its result
    /// must be answered before the store runs what it left behind.
    /// That rule lives in the store's turn, which is the only
    /// caller of this.
    pub fn open_entry_gate(&mut self, tables: &mut TaskTables) {
        if self.entry_gate.is_empty() {
            return;
        }
        let mut held: VecDeque<GateEntry<T>> = VecDeque::new();
        let mut closed: Vec<InstanceId> = Vec::new();
        while let Some(entry) = self.entry_gate.pop_front() {
            if closed.contains(&entry.instance)
                || Self::blocked(tables, entry.instance, entry.needs_exclusive)
            {
                closed.push(entry.instance);
                held.push_back(entry);
                continue;
            }
            if let Some(record) = tables.instance_mut(entry.instance) {
                record.waiting_to_enter = record.waiting_to_enter.saturating_sub(1);
            }
            Self::claim_exclusive(tables, entry.task, entry.instance, entry.needs_exclusive);
            self.high_priority.push_back(entry.item);
        }
        self.entry_gate = held;
    }

    /// Take the item a previous turn left in the resume-after-yield
    /// slot, which runs at the top of a turn.
    pub fn take_resume_after_yield(&mut self) -> Option<Item<T>> {
        self.resume_after_yield.take()
    }

    /// Take the next ready item: the switch slot first, then the
    /// front of the high-priority queue.
    pub fn take_ready(&mut self) -> Option<Item<T>> {
        match self.switch_slot.take() {
            Some(item) => Some(item),
            None => self.high_priority.pop_front(),
        }
    }

    /// Take the next ready item that belongs to `instance`, leaving
    /// every other item where it is.
    ///
    /// This is what a turn run for a task that must not block takes.
    /// Such a task gives way only to the ready work of its own
    /// instance, so the item another instance queued stays queued
    /// and the turn that follows the block runs it. The order within
    /// the instance is the order of the whole queue: the switch slot
    /// first, then the high-priority queue from its front.
    pub fn take_ready_in(&mut self, instance: InstanceId) -> Option<Item<T>> {
        if self
            .switch_slot
            .as_ref()
            .is_some_and(|item| item.instance() == Some(instance))
        {
            return self.switch_slot.take();
        }
        let position = self
            .high_priority
            .iter()
            .position(|item| item.instance() == Some(instance))?;
        self.high_priority.remove(position)
    }

    /// Take the next resumption after a yield: the one already in
    /// the resume-after-yield slot first, then the front of the
    /// low-priority queue.
    ///
    /// This is what a nested turn takes once nothing else is ready.
    /// A driver's turn defers instead, so that the item runs only
    /// after control has gone back to the host executor; a nested
    /// turn runs from inside a guest call and has no control to
    /// give back, so it runs the item where it stands.
    pub fn take_deferred(&mut self) -> Option<Item<T>> {
        let taken = match self.resume_after_yield.take() {
            Some(item) => Some(item),
            None => self.low_priority.pop_front(),
        };
        self.count_resumption(taken)
    }

    /// Take the next resumption after a yield that belongs to
    /// `instance`, leaving every other one where it is. This is what
    /// a nested turn run for a task that must not block takes, which
    /// gives way to the work of its own instance and to nothing
    /// else.
    pub fn take_deferred_in(&mut self, instance: InstanceId) -> Option<Item<T>> {
        if self
            .resume_after_yield
            .as_ref()
            .is_some_and(|item| item.instance() == Some(instance))
        {
            let taken = self.resume_after_yield.take();
            return self.count_resumption(taken);
        }
        let position = self
            .low_priority
            .iter()
            .position(|item| item.instance() == Some(instance))?;
        let taken = self.low_priority.remove(position);
        self.count_resumption(taken)
    }

    /// Count a resumption a nested turn is about to run, and hand
    /// the item back.
    fn count_resumption(&mut self, taken: Option<Item<T>>) -> Option<Item<T>> {
        if taken.is_some() {
            self.resumptions = self.resumptions.saturating_add(1);
        }
        taken
    }

    /// How many resumptions after a yield the scheduler has handed
    /// out to a nested turn, ever.
    ///
    /// A nested turn runs a yielded item where a driver's turn
    /// defers it, so a block served by nested turns would run for
    /// ever against a guest that gives way until its caller unblocks
    /// it — the one shape that needs a stack switch. A block reads
    /// this beside [`Scheduler::items_run`] after each of its turns:
    /// a turn in which the two moved by the same amount ran nothing
    /// but resumptions, so the store did no work of its own in it.
    pub fn resumptions(&self) -> u64 {
        self.resumptions
    }

    /// Move the front of the low-priority queue into the
    /// resume-after-yield slot, so the turn can end and the driver
    /// can return control to the host executor before the item runs.
    /// `false` when nothing is deferred.
    ///
    /// The slot holds one item, and an occupied slot already holds a
    /// resumption no driver has returned control for yet. The queue
    /// then keeps its front rather than losing it to an overwrite,
    /// and the answer is still `true`: deferred work is waiting and
    /// the turn should end.
    pub fn defer_low_priority(&mut self) -> bool {
        if self.resume_after_yield.is_some() {
            return true;
        }
        match self.low_priority.pop_front() {
            Some(item) => {
                self.resume_after_yield = Some(item);
                true
            }
            None => false,
        }
    }

    /// Whether anything is ready to run, deferred work included.
    pub fn has_ready_item(&self) -> bool {
        self.has_immediate_item() || self.has_deferred_item()
    }

    /// Whether an item can run in this turn: one in the switch slot
    /// or one in the high-priority queue.
    pub fn has_immediate_item(&self) -> bool {
        self.switch_slot.is_some() || !self.high_priority.is_empty()
    }

    /// Whether a resumption after a yield is waiting: one still in
    /// the low-priority queue, or one already in the
    /// resume-after-yield slot. Neither runs until a driver has
    /// returned control to the host executor.
    pub fn has_deferred_item(&self) -> bool {
        !self.low_priority.is_empty() || self.resume_after_yield.is_some()
    }

    /// How many items the store holds, ready, held at the gate, or
    /// held until the store can run them.
    pub fn queued_items(&self) -> usize {
        usize::from(self.switch_slot.is_some())
            + self.high_priority.len()
            + self.low_priority.len()
            + usize::from(self.resume_after_yield.is_some())
            + self.entry_gate.len()
            + self.held_callbacks.len()
    }

    /// How many tasks wait at an entry gate.
    pub fn waiting_at_gate(&self) -> usize {
        self.entry_gate.len()
    }

    /// How many callback items the store holds until it can run them.
    pub fn held_callbacks(&self) -> usize {
        self.held_callbacks.len()
    }

    /// Whether a callback item the store holds until an event is
    /// parked on behalf of `thread`: the implicit thread of a callback
    /// task that returned the wait code. Such a thread waits on a set
    /// as a thread inside `waitable-set.wait` does, but it is no frame
    /// on the stack. Its callback runs as an item once the set holds
    /// an event and the instance's exclusive thread is free.
    pub fn holds_callback_of(&self, thread: ThreadId) -> bool {
        self.held_callbacks.entries.values().any(|entry| {
            matches!(entry.condition, HeldFor::Event { thread: held, .. } if held == thread)
        })
    }

    /// Hold `item` until a waitable of `set` holds an event. `thread`
    /// is the task's implicit thread, which the caller parked on the
    /// set; the wait ends when the item is queued.
    ///
    /// This is the second half of the wait status word: a callback
    /// task that returned it with a set holding no event leaves the
    /// item here, and the turn that finds the set filled queues it
    /// with the event the set delivers.
    pub fn hold_for_event(
        &mut self,
        instance: InstanceId,
        thread: ThreadId,
        set: WaitableSetId,
        slot: EventSlot,
        item: Item<T>,
    ) {
        self.held_callbacks.hold(HeldCallback {
            instance,
            condition: HeldFor::Event { thread, set },
            slot,
            item,
        });
    }

    /// Hold `item` until the exclusive thread of `instance` is free.
    ///
    /// A callback item runs core code, so it needs the instance to
    /// itself. One that a turn reaches while another task holds the
    /// instance is deferred here rather than run, and the turn that
    /// finds the instance free queues it again with the event it
    /// already carries.
    pub fn hold_for_exclusive(&mut self, instance: InstanceId, slot: EventSlot, item: Item<T>) {
        self.held_callbacks.hold(HeldCallback {
            instance,
            condition: HeldFor::ExclusiveThread,
            slot,
            item,
        });
    }

    /// Queue every held callback item whose condition now holds, in
    /// the order the items were held.
    ///
    /// An item held for an event is queued with the event the set
    /// delivers, and the wait its thread began ends as it is queued.
    /// An item held for the exclusive thread is queued with the event
    /// it already carries. Both go on the high-priority queue: the
    /// readiness is fresh, and a yield that gave way has given way
    /// already.
    ///
    /// Only the items that could have become ready are looked at: an
    /// item held for an event is looked at when its set was signalled
    /// since the last look, so a turn costs nothing for a callback
    /// task whose set nothing has touched. The documentation of the
    /// held items' store states the whole rule.
    ///
    /// The list of signalled sets is taken on every look, whether or
    /// not anything is held, so a store that never holds a callback
    /// item does not keep a growing list of every set that ever took
    /// on an event. Nothing is lost by it: an item held after its
    /// set was signalled is examined on the first look after it was
    /// held, whatever was signalled.
    ///
    /// An item whose release fails is dropped and its error ends the
    /// turn. Every other item is still held when the next turn looks.
    pub fn release_held_callbacks(&mut self, tables: &mut HandleTables) -> Result<()> {
        let signalled = tables.tasks.take_signalled_sets();
        if self.held_callbacks.is_empty() {
            return Ok(());
        }
        let numbers = self.held_callbacks.take_to_examine(&signalled);
        for (position, number) in numbers.iter().enumerate() {
            if let Err(error) = self.release_one(*number, tables) {
                // Only the entry whose own release failed is given
                // up. The ones not looked at yet are no less held
                // because the store lost a record another item
                // named, and the signal that brought them here has
                // been taken, so the next look examines them anyway.
                self.held_callbacks
                    .fresh
                    .extend_from_slice(&numbers[position + 1..]);
                return Err(error);
            }
        }
        Ok(())
    }

    /// Queue the item held under `number` when its condition holds,
    /// and leave it held when it does not.
    fn release_one(&mut self, number: u64, tables: &mut HandleTables) -> Result<()> {
        let Some(entry) = self.held_callbacks.entries.get(&number) else {
            return Ok(());
        };
        let condition = entry.condition;
        let ready = match condition {
            HeldFor::ExclusiveThread => tables
                .tasks
                .instance(entry.instance)
                .is_none_or(|record| record.exclusive_thread.is_none()),
            HeldFor::Event { set, .. } => match tables.tasks.set_has_pending_event(set) {
                Ok(ready) => ready,
                Err(error) => {
                    self.held_callbacks.remove(number);
                    return Err(error);
                }
            },
        };
        if !ready {
            return Ok(());
        }
        let Some(entry) = self.held_callbacks.remove(number) else {
            return Ok(());
        };
        if let HeldFor::Event { thread, set } = condition {
            entry
                .slot
                .fill(tables.finish_wait_on_waitable_set(set, thread)?);
        }
        self.high_priority.push_back(entry.item);
        Ok(())
    }

    /// Drop every item that names `task`, wherever it is waiting,
    /// and undo what holding it reserved.
    ///
    /// This is the second half of ending a task, and the store's own
    /// rule for a dead task's pending work: the record and the work
    /// queued against it leave together. The module documentation
    /// states why the rule is a drop rather than a check at the
    /// point an item runs.
    ///
    /// Each place an item can be waiting is swept, and each place
    /// keeps the order of the items that stay:
    ///
    /// - The switch slot, the two ready queues, and the
    ///   resume-after-yield slot hold items that are ready or nearly
    ///   so. Dropping one reserves nothing to give back.
    /// - The entry gate holds the start of a task that has not run.
    ///   The count of the tasks waiting to enter the instance falls
    ///   with the entry, or the gate would stay shut against every
    ///   call that came after this one.
    /// - The held callbacks hold an item waiting for an event or for
    ///   the instance's exclusive thread. One waiting for an event
    ///   has parked the task's implicit thread on a waitable set, so
    ///   the wait ends with the item and the set is left with no
    ///   waiter that can never be answered. A set the store no
    ///   longer holds fails that step, and the item is dropped all
    ///   the same: it is the record the wait was for that has gone.
    ///
    /// The sweep runs after the task ended, so the thread the wait
    /// was parked on has exited with the task and only the set's
    /// tally of waiters falls. That tally is the half that matters:
    /// `waitable-set.drop` traps on a set a thread still waits on,
    /// so a tally left standing would make the set undroppable for
    /// the life of the instance.
    ///
    /// A thread of the task that is suspended in the provider is the
    /// task's pending work too, and it leaves with the task, never
    /// resumed. The blocking built-in it waits in has done its first
    /// part, which holds something back in the same way a held
    /// callback does: a raised tally of waiters, a synchronous
    /// waiter on an end, a host task parked for a synchronous lower,
    /// a callee's task. The sweep cannot give that back itself: the
    /// finish part that does runs with the store, and the tables are
    /// locked here. It hands each such thread back, with the built-in
    /// it waits in, and the caller finishes them once the lock is
    /// released, through `release_discarded_threads` on the store.
    pub fn discard_task_items(
        &mut self,
        tables: &mut TaskTables,
        task: TaskId,
    ) -> Vec<(ThreadId, ParkedThread<T>, Option<PendingBlock<T>>)> {
        let names_task = |item: &Item<T>| item.task() == Some(task);
        if self.switch_slot.as_ref().is_some_and(&names_task) {
            self.switch_slot = None;
        }
        if self.resume_after_yield.as_ref().is_some_and(&names_task) {
            self.resume_after_yield = None;
        }
        self.high_priority.retain(|item| !names_task(item));
        self.low_priority.retain(|item| !names_task(item));
        self.entry_gate.retain(|entry| {
            if entry.task != task {
                return true;
            }
            if let Some(record) = tables.instance_mut(entry.instance) {
                record.waiting_to_enter = record.waiting_to_enter.saturating_sub(1);
            }
            false
        });
        let named: Vec<u64> = self
            .held_callbacks
            .entries
            .iter()
            .filter(|(_, entry)| names_task(&entry.item))
            .map(|(number, _)| *number)
            .collect();
        for number in named {
            if let Some(entry) = self.held_callbacks.remove(number)
                && let HeldFor::Event { thread, set } = entry.condition
            {
                let _ = tables.end_wait(set, thread);
            }
        }
        // A thread of the task that is suspended in the provider is
        // never resumed. Its continuation stays in the switch
        // module's table unresumed until the store drops or a later
        // thread takes its slot. The threads go back in the order
        // they last suspended, as the idle driver fails them.
        let mut parked: Vec<(u64, ThreadId)> = self
            .parked
            .iter()
            .filter(|(_, parked)| parked.task == task)
            .map(|(thread, parked)| (parked.number, *thread))
            .collect();
        parked.sort_unstable_by_key(|(number, _)| *number);
        parked
            .into_iter()
            .filter_map(|(_, thread)| {
                let parked = self.parked.remove(&thread)?;
                Some((thread, parked, self.blocks.remove(&thread)))
            })
            .collect()
    }

    /// The reference's `has_backpressure`, for one waiting task.
    fn blocked(tables: &TaskTables, instance: InstanceId, needs_exclusive: bool) -> bool {
        match tables.instance(instance) {
            Some(record) => {
                record.backpressure > 0 || (needs_exclusive && record.exclusive_thread.is_some())
            }
            None => false,
        }
    }

    /// End the implicit thread of `task`, which is the reference's
    /// `exit_implicit_thread`: the thread the task's call ran on is
    /// over, so the instance it held exclusively goes back and the
    /// next task waiting at the gate can take it.
    ///
    /// The release is keyed on the thread rather than on the task's
    /// canon options, so a task whose thread never claimed the
    /// instance leaves whoever holds it alone. A task of a
    /// synchronous export is such a task: it ignores the gate, so it
    /// never claimed anything to give back.
    pub fn exit_implicit_thread(&self, tables: &mut TaskTables, task: TaskId) {
        self.release_exclusive_thread(tables, task);
    }

    /// Give the instance back that `task`'s implicit thread holds
    /// exclusively, if it holds one.
    ///
    /// The callback loop of an asynchronous export spells this on its
    /// own, between events: the exclusive thread is held while core
    /// code runs and released while the task waits or gives way, so a
    /// synchronous export of the same instance can run in between.
    ///
    /// The release is keyed on the thread rather than on the task's
    /// canon options, so a task whose thread never claimed the
    /// instance leaves whoever holds it alone.
    pub fn release_exclusive_thread(&self, tables: &mut TaskTables, task: TaskId) {
        let Some(record) = tables.task(task) else {
            return;
        };
        let (thread, instance) = (record.implicit_thread, record.instance);
        let Some(record) = instance.and_then(|instance| tables.instance_mut(instance)) else {
            return;
        };
        if record.exclusive_thread == Some(thread) {
            record.exclusive_thread = None;
        }
    }

    /// Give the instance to `task`'s implicit thread, which is what a
    /// callback item does before it runs the callback: the instance
    /// is held for the length of the core code and released again
    /// when the status word says the task waits or gives way.
    pub fn take_exclusive_thread(
        &self,
        tables: &mut TaskTables,
        task: TaskId,
        instance: InstanceId,
    ) {
        Self::claim_exclusive(tables, task, instance, true);
    }

    /// Give the instance to the task's implicit thread when the task
    /// needs it exclusively.
    fn claim_exclusive(
        tables: &mut TaskTables,
        task: TaskId,
        instance: InstanceId,
        needs_exclusive: bool,
    ) {
        if !needs_exclusive {
            return;
        }
        let Some(thread) = tables.task(task).map(|record| record.implicit_thread) else {
            return;
        };
        if let Some(record) = tables.instance_mut(instance) {
            record.exclusive_thread = Some(thread);
        }
    }
}

impl<T: 'static> Default for Scheduler<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use crate::store::{StoreContextInternalExt, StoreInternalExt};
    use core::future::Future;
    use core::pin::Pin;
    use core::task::{Context, Poll, Waker};
    use std::sync::{Arc, Mutex};

    use crate::engine::Engine;
    use crate::error::Result;
    use crate::store::{Store, StoreContext};
    use crate::value::Val;

    use super::super::end_id::EndId;
    use super::super::event::Event;
    use super::super::event_slot::EventSlot;
    use super::super::host_task::HostTask;
    use super::super::instance_id::InstanceId;
    use super::super::item::Item;
    use super::super::item_kind::ItemKind;
    use super::super::outcome::Outcome;
    use super::super::subtask_state::SubtaskState;
    use super::super::task_id::TaskId;
    use super::super::thread_id::ThreadId;
    use super::super::waitable_set_id::WaitableSetId;

    /// What the items of one test wrote as they ran, in order.
    type Log = Arc<Mutex<Vec<&'static str>>>;

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

    /// A component instance record with nothing set.
    fn instance(store: &Store<()>) -> InstanceId {
        store
            .internal_ref()
            .tables()
            .lock()
            .expect("tables")
            .tasks
            .insert_instance()
    }

    /// Raise or lower the backpressure of `instance`, which is what
    /// shuts and opens its entry gate.
    fn set_backpressure(store: &Store<()>, instance: InstanceId, value: u32) {
        store
            .internal_ref()
            .tables()
            .lock()
            .expect("tables")
            .tasks
            .instance_mut(instance)
            .expect("instance record")
            .backpressure = value;
    }

    /// Queue the start of a fresh task of `instance`, with an item
    /// that `build` makes from the task's own identity.
    fn start_with(
        store: &mut StoreContext<'_, ()>,
        instance: InstanceId,
        async_function: bool,
        needs_exclusive: bool,
        build: impl FnOnce(TaskId) -> Item<()>,
    ) -> TaskId {
        let task = store
            .internal()
            .tables()
            .lock()
            .expect("tables")
            .tasks
            .create_task(None, None, instance);
        store
            .internal()
            .start_export_thread(task, instance, async_function, needs_exclusive, build(task))
            .expect("queue the task's start");
        task
    }

    /// Queue the start of a fresh task of `instance`.
    fn start(
        store: &mut StoreContext<'_, ()>,
        instance: InstanceId,
        async_function: bool,
        needs_exclusive: bool,
        item: Item<()>,
    ) -> TaskId {
        start_with(store, instance, async_function, needs_exclusive, |_| item)
    }

    /// Queue the start of a fresh task of `instance` through the
    /// switch slot, which is what a call between two components
    /// does: the item goes in the slot, and the gate decides whether
    /// it stays there for the caller's trampoline to run.
    fn start_switched(
        store: &mut StoreContext<'_, ()>,
        instance: InstanceId,
        async_function: bool,
        needs_exclusive: bool,
        item: Item<()>,
    ) -> TaskId {
        let task = store
            .internal()
            .tables()
            .lock()
            .expect("tables")
            .tasks
            .create_task(None, None, instance);
        store.internal().scheduler_mut().switch_to(item);
        store
            .internal()
            .start_switched_export_thread(task, instance, async_function, needs_exclusive)
            .expect("queue the task's start");
        task
    }

    /// An item that runs one export call the way `Func::run_task`
    /// runs it, without a guest: the task becomes the current scope,
    /// it resolves with no result, and then it exits. Nothing else
    /// stands between a call returning and the instance going back,
    /// so a test that ends a call this way is the ordering the
    /// export path really takes.
    fn export_call(log: &Log, name: &'static str, task: TaskId) -> Item<()> {
        let log = log.clone();
        Item::new(
            ItemKind::TaskStart,
            move |store: &mut StoreContext<'_, ()>| {
                log.lock().expect("log").push(name);
                store.internal().enter_export_task(task)?;
                store.internal().resolve_export_task(task, None)?;
                store
                    .internal()
                    .exit_export_task(task)?
                    .expect("the call dropped every borrow it took");
                Ok(())
            },
        )
    }

    #[wcmp_macros::test]
    fn it_runs_the_switch_slot_before_the_high_priority_queue() {
        let mut store = store();
        let log = log();
        store
            .internal()
            .scheduler_mut()
            .push_high_priority(marker(&log, "high"));
        store
            .internal()
            .scheduler_mut()
            .switch_to(marker(&log, "switch"));

        let outcome = store.internal().turn(Waker::noop()).expect("turn");

        assert_eq!(outcome, Outcome::Idle, "both items ran and nothing is left");
        assert_eq!(
            entries(&log),
            vec!["switch", "high"],
            "the thread the scheduler must switch to next runs before fresh readiness"
        );
    }

    #[wcmp_macros::test]
    fn it_runs_the_high_priority_queue_in_the_order_items_became_ready() {
        let mut store = store();
        let log = log();
        store
            .internal()
            .scheduler_mut()
            .push_high_priority(marker(&log, "first"));
        store
            .internal()
            .scheduler_mut()
            .push_high_priority(marker(&log, "second"));

        store.internal().turn(Waker::noop()).expect("turn");

        assert_eq!(entries(&log), vec!["first", "second"]);
    }

    #[wcmp_macros::test]
    fn it_takes_only_the_ready_work_of_the_instance_it_is_asked_for() {
        let mut store = store();
        let log = log();
        let mine = instance(&store);
        let other = instance(&store);

        // Three items of one instance and one of another, plus one
        // that names no instance at all.
        start(
            &mut store.internal().context(),
            mine,
            false,
            true,
            marker(&log, "mine"),
        );
        start(
            &mut store.internal().context(),
            other,
            false,
            true,
            marker(&log, "other"),
        );
        start(
            &mut store.internal().context(),
            mine,
            false,
            true,
            marker(&log, "mine again"),
        );
        store
            .internal()
            .scheduler_mut()
            .push_high_priority(marker(&log, "no instance"));

        while let Some(item) = store.internal().scheduler_mut().take_ready_in(mine) {
            item.run(&mut store.internal().context())
                .expect("the item runs");
        }

        assert_eq!(
            entries(&log),
            vec!["mine", "mine again"],
            "the instance's own work ran, in queue order, and nothing else did"
        );
        assert_eq!(
            store.internal().scheduler().queued_items(),
            2,
            "the other instance's item and the item that names none are still \
             queued for a turn of the scheduler"
        );
    }

    #[wcmp_macros::test]
    fn it_takes_a_switch_slot_item_that_is_the_instances_own_work() {
        let mut store = store();
        let log = log();
        let mine = instance(&store);
        store
            .internal()
            .scheduler_mut()
            .push_high_priority(marker(&log, "queued").in_instance(mine));
        store
            .internal()
            .scheduler_mut()
            .switch_to(marker(&log, "switch").in_instance(mine));

        let item = store
            .internal()
            .scheduler_mut()
            .take_ready_in(mine)
            .expect("the slot holds this instance's work");
        item.run(&mut store.internal().context())
            .expect("the item runs");

        assert_eq!(
            entries(&log),
            vec!["switch"],
            "the slot comes before the high-priority queue here as it does in \
             a turn of the whole scheduler"
        );
        assert_eq!(
            store.internal().scheduler().queued_items(),
            1,
            "the slot is empty and the instance's queued item is what is left"
        );
    }

    #[wcmp_macros::test]
    fn it_leaves_a_switch_slot_item_of_another_instance_in_the_slot() {
        let mut store = store();
        let log = log();
        let mine = instance(&store);
        let other = instance(&store);
        store
            .internal()
            .scheduler_mut()
            .push_high_priority(marker(&log, "mine").in_instance(mine));
        store
            .internal()
            .scheduler_mut()
            .switch_to(marker(&log, "another instance").in_instance(other));

        let item = store
            .internal()
            .scheduler_mut()
            .take_ready_in(mine)
            .expect("the queue holds this instance's work");
        item.run(&mut store.internal().context())
            .expect("the item runs");

        assert_eq!(
            entries(&log),
            vec!["mine"],
            "the slot's item is not this instance's work, so the item came \
             from the queue behind it"
        );

        // The slot kept its item rather than losing it to the skip: a
        // turn of the whole scheduler still runs it, and runs it first.
        let left = store
            .internal()
            .scheduler_mut()
            .take_ready()
            .expect("the switch slot kept its item");
        left.run(&mut store.internal().context())
            .expect("the item runs");

        assert_eq!(entries(&log), vec!["mine", "another instance"]);
    }

    #[wcmp_macros::test]
    fn it_takes_the_resume_after_yield_slot_before_the_low_priority_queue() {
        let mut store = store();
        let log = log();
        store
            .internal()
            .scheduler_mut()
            .push_low_priority(marker(&log, "one"));
        store
            .internal()
            .scheduler_mut()
            .push_low_priority(marker(&log, "two"));
        assert!(
            store.internal().scheduler_mut().defer_low_priority(),
            "the front of the queue moves into the resume-after-yield slot"
        );

        while let Some(item) = store.internal().scheduler_mut().take_deferred() {
            item.run(&mut store.internal().context())
                .expect("the item runs");
        }

        assert_eq!(
            entries(&log),
            vec!["one", "two"],
            "the slot's item came first and the queue followed, which is the \
             order a driver's turns would have run them in"
        );
        assert_eq!(
            store.internal().scheduler().queued_items(),
            0,
            "nothing deferred was left behind"
        );
    }

    #[wcmp_macros::test]
    fn it_takes_only_the_deferred_work_of_the_instance_it_is_asked_for() {
        let mut store = store();
        let log = log();
        let mine = instance(&store);
        let other = instance(&store);
        store
            .internal()
            .scheduler_mut()
            .push_low_priority(marker(&log, "other").in_instance(other));
        store
            .internal()
            .scheduler_mut()
            .push_low_priority(marker(&log, "mine").in_instance(mine));
        store
            .internal()
            .scheduler_mut()
            .push_low_priority(marker(&log, "no instance"));

        while let Some(item) = store.internal().scheduler_mut().take_deferred_in(mine) {
            item.run(&mut store.internal().context())
                .expect("the item runs");
        }

        assert_eq!(
            entries(&log),
            vec!["mine"],
            "a task that must not block gives way to the deferred work of its \
             own instance and to nothing else"
        );
        assert_eq!(
            store.internal().scheduler().queued_items(),
            2,
            "the other instance's item and the item that names none are still \
             queued for a turn of the scheduler"
        );
    }

    #[wcmp_macros::test]
    fn it_yields_to_the_driver_before_it_runs_a_low_priority_item() {
        let mut store = store();
        let log = log();
        store
            .internal()
            .scheduler_mut()
            .push_low_priority(marker(&log, "low"));
        store
            .internal()
            .scheduler_mut()
            .push_high_priority(marker(&log, "high"));

        let first = store.internal().turn(Waker::noop()).expect("first turn");

        assert_eq!(
            first,
            Outcome::Progress,
            "the turn that ran an item ends before it defers the low-priority \
             item, so the driver consults its condition first"
        );
        assert_eq!(
            entries(&log),
            vec!["high"],
            "the high-priority queue drains first, and the low-priority item has not run"
        );

        let second = store.internal().turn(Waker::noop()).expect("second turn");

        assert_eq!(
            second,
            Outcome::Yield,
            "the low-priority item gives way and the turn ends"
        );
        assert_eq!(entries(&log), vec!["high"]);

        let third = store.internal().turn(Waker::noop()).expect("third turn");

        assert_eq!(third, Outcome::Idle);
        assert_eq!(
            entries(&log),
            vec!["high", "low"],
            "the resumption runs at the top of the next turn"
        );
    }

    #[wcmp_macros::test]
    fn it_lets_a_synchronous_task_past_a_shut_entry_gate() {
        let mut store = store();
        let log = log();
        let instance = instance(&store);
        set_backpressure(&store, instance, 1);

        start(
            &mut store.internal().context(),
            instance,
            false,
            true,
            marker(&log, "sync"),
        );
        store.internal().turn(Waker::noop()).expect("turn");

        assert_eq!(
            store.internal().scheduler().waiting_at_gate(),
            0,
            "a task of a synchronous export never waits at the gate"
        );
        assert_eq!(entries(&log), vec!["sync"]);
    }

    #[wcmp_macros::test]
    fn it_starts_tasks_held_at_the_entry_gate_in_arrival_order() {
        let mut store = store();
        let log = log();
        let instance = instance(&store);
        set_backpressure(&store, instance, 1);

        for name in ["first", "second", "third"] {
            start(
                &mut store.internal().context(),
                instance,
                true,
                false,
                marker(&log, name),
            );
        }

        let shut = store
            .internal()
            .turn(Waker::noop())
            .expect("turn with the gate shut");

        assert_eq!(
            shut,
            Outcome::Idle,
            "nothing is ready while the gate holds every task"
        );
        assert_eq!(store.internal().scheduler().waiting_at_gate(), 3);
        assert!(entries(&log).is_empty(), "no task's thread has run");

        set_backpressure(&store, instance, 0);
        let open = store
            .internal()
            .turn(Waker::noop())
            .expect("turn with the gate open");

        assert_eq!(open, Outcome::Idle);
        assert_eq!(store.internal().scheduler().waiting_at_gate(), 0);
        assert_eq!(
            entries(&log),
            vec!["first", "second", "third"],
            "the tasks start in the order they arrived at the gate"
        );
    }

    #[wcmp_macros::test]
    fn it_holds_a_task_behind_one_that_already_waits_at_the_gate() {
        let mut store = store();
        let log = log();
        let instance = instance(&store);
        set_backpressure(&store, instance, 1);
        start(
            &mut store.internal().context(),
            instance,
            true,
            false,
            marker(&log, "early"),
        );
        set_backpressure(&store, instance, 0);

        // Nothing blocks the second task, but one is already
        // waiting, so it queues behind it rather than overtaking it.
        start(
            &mut store.internal().context(),
            instance,
            true,
            false,
            marker(&log, "late"),
        );
        store.internal().turn(Waker::noop()).expect("turn");

        assert_eq!(entries(&log), vec!["early", "late"]);
    }

    #[wcmp_macros::test]
    fn it_clears_the_switch_slot_when_the_gate_holds_the_switched_task() {
        let mut store = store();
        let log = log();
        let instance = instance(&store);
        // One task of the instance holds it exclusively and a second
        // already waits at the gate behind it, so the callee of a
        // call between two components arrives third.
        start(
            &mut store.internal().context(),
            instance,
            true,
            true,
            marker(&log, "holder"),
        );
        start(
            &mut store.internal().context(),
            instance,
            true,
            true,
            marker(&log, "early"),
        );

        start_switched(
            &mut store.internal().context(),
            instance,
            true,
            true,
            marker(&log, "callee"),
        );

        // The caller's trampoline runs the slot next, and finds it
        // empty: the callee is not ready, so nothing runs inside the
        // caller's frame.
        store
            .internal()
            .context()
            .internal()
            .run_switch_slot()
            .expect("the caller's trampoline runs the slot");

        assert!(
            entries(&log).is_empty(),
            "the gate held the callee, so the slot the trampoline ran was empty"
        );
        assert!(
            store
                .internal()
                .scheduler_mut()
                .take_switch_slot()
                .is_none(),
            "the held task left nothing in the slot"
        );
        assert_eq!(
            store.internal().scheduler().waiting_at_gate(),
            2,
            "the callee waits at the gate behind the task that arrived before it"
        );
    }

    #[wcmp_macros::test]
    fn it_starts_a_switched_task_the_gate_held_after_the_earlier_arrivals() {
        let mut store = store();
        let log = log();
        let instance = instance(&store);
        // The two earlier tasks each end their call the way the
        // export path ends one, so the instance goes back and the
        // gate lets the next task through.
        start_with(
            &mut store.internal().context(),
            instance,
            true,
            true,
            |task| export_call(&log, "holder", task),
        );
        start_with(
            &mut store.internal().context(),
            instance,
            true,
            true,
            |task| export_call(&log, "early", task),
        );
        start_switched(
            &mut store.internal().context(),
            instance,
            true,
            true,
            marker(&log, "callee"),
        );

        store
            .internal()
            .context()
            .internal()
            .run_switch_slot()
            .expect("the caller's trampoline runs the slot");
        // A turn hands what the gate let through to the turn that
        // follows, so the three tasks start in three turns: the
        // holder, then the task that was waiting when it arrived,
        // then the callee the switch slot brought. The log after
        // each turn says so, and says it is one task per turn
        // rather than three in the first.
        store
            .internal()
            .turn(Waker::noop())
            .expect("the holder's turn");
        assert_eq!(
            entries(&log),
            vec!["holder"],
            "the holder ran alone, and the task its ended call released is \
             the next turn's"
        );
        store
            .internal()
            .turn(Waker::noop())
            .expect("the early task's turn");
        assert_eq!(
            entries(&log),
            vec!["holder", "early"],
            "and the callee behind it is the turn after that one's"
        );
        store
            .internal()
            .turn(Waker::noop())
            .expect("the callee's turn");

        assert_eq!(
            entries(&log),
            vec!["holder", "early", "callee"],
            "the callee the gate held started in arrival order, behind the \
             tasks that were already waiting"
        );
        assert_eq!(store.internal().scheduler().waiting_at_gate(), 0);
    }

    #[wcmp_macros::test]
    fn it_gives_the_instance_to_a_task_that_needs_it_exclusively() {
        let mut store = store();
        let log = log();
        let instance = instance(&store);

        start(
            &mut store.internal().context(),
            instance,
            true,
            true,
            marker(&log, "exclusive"),
        );
        start(
            &mut store.internal().context(),
            instance,
            true,
            true,
            marker(&log, "queued"),
        );
        store.internal().turn(Waker::noop()).expect("turn");

        assert_eq!(
            entries(&log),
            vec!["exclusive"],
            "the second task waits for the exclusive thread the first took"
        );
        assert_eq!(store.internal().scheduler().waiting_at_gate(), 1);
    }

    #[wcmp_macros::test]
    fn it_releases_the_instance_when_the_holding_tasks_call_ends() {
        let mut store = store();
        let log = log();
        let instance = instance(&store);

        // The first task's item ends its call the way the export
        // path ends one: it resolves the task and exits it. That
        // exit is what ends the task's implicit thread, so the
        // instance goes back while the task record is still there
        // to say which thread held it.
        start_with(
            &mut store.internal().context(),
            instance,
            true,
            true,
            |task| export_call(&log, "exclusive", task),
        );
        start(
            &mut store.internal().context(),
            instance,
            true,
            true,
            marker(&log, "queued"),
        );

        // The first turn runs the holder and opens the gate as it
        // ends; the task the gate let through is the second turn's.
        store
            .internal()
            .turn(Waker::noop())
            .expect("the holder's turn");

        assert_eq!(
            entries(&log),
            vec!["exclusive"],
            "the turn ended on the release, so the task it let through has \
             not run in it"
        );
        assert_eq!(
            store.internal().scheduler().waiting_at_gate(),
            0,
            "though the gate had already let that task go"
        );

        store
            .internal()
            .turn(Waker::noop())
            .expect("the released task's turn");

        assert_eq!(
            entries(&log),
            vec!["exclusive", "queued"],
            "the task at the gate took the instance the ended call gave back"
        );
        assert_eq!(store.internal().scheduler().waiting_at_gate(), 0);
    }

    #[wcmp_macros::test]
    fn it_releases_the_instance_when_the_holding_tasks_call_fails() {
        let mut store = store();
        let log = log();
        let instance = instance(&store);

        // The same release on the failure path: a call that trapped
        // is abandoned rather than exited, and the instance it held
        // goes back all the same.
        start_with(
            &mut store.internal().context(),
            instance,
            true,
            true,
            |task| {
                let log = log.clone();
                Item::new(
                    ItemKind::TaskStart,
                    move |store: &mut StoreContext<'_, ()>| {
                        log.lock().expect("log").push("abandoned");
                        store.internal().enter_export_task(task)?;
                        store.internal().abandon_export_task(task)
                    },
                )
            },
        );
        start(
            &mut store.internal().context(),
            instance,
            true,
            true,
            marker(&log, "queued"),
        );

        store
            .internal()
            .turn(Waker::noop())
            .expect("the holder's turn");

        assert_eq!(
            entries(&log),
            vec!["abandoned"],
            "the turn ended on the release here too, so the task it let \
             through has not run in it"
        );
        assert_eq!(
            store.internal().scheduler().waiting_at_gate(),
            0,
            "though the gate had already let that task go"
        );

        store
            .internal()
            .turn(Waker::noop())
            .expect("the released task's turn");

        assert_eq!(
            entries(&log),
            vec!["abandoned", "queued"],
            "the task at the gate took the instance the failed call gave back"
        );
        assert_eq!(store.internal().scheduler().waiting_at_gate(), 0);
    }

    /// A fresh waitable set record, with a fresh task's implicit
    /// thread parked on it: what a callback task that returned the
    /// wait word leaves behind.
    fn waiting_on_a_set(store: &Store<()>, instance: InstanceId) -> (WaitableSetId, ThreadId) {
        let (_task, set, thread) = task_waiting_on_a_set(store, instance);
        (set, thread)
    }

    /// The same, with the task named, for a test that ends it.
    fn task_waiting_on_a_set(
        store: &Store<()>,
        instance: InstanceId,
    ) -> (TaskId, WaitableSetId, ThreadId) {
        let mut guard = store.internal_ref().tables().lock().expect("tables");
        let set = guard.tasks.insert_waitable_set();
        let task = guard.tasks.create_task(None, None, instance);
        let thread = guard.tasks.task(task).expect("task record").implicit_thread;
        guard
            .tasks
            .begin_wait(set, thread)
            .expect("park the thread");
        (task, set, thread)
    }

    /// A fresh task of `instance` with nothing queued for it.
    fn task(store: &Store<()>, instance: InstanceId) -> TaskId {
        store
            .internal_ref()
            .tables()
            .lock()
            .expect("tables")
            .tasks
            .create_task(None, None, instance)
    }

    /// How many tasks of `instance` the entry gate counts as waiting
    /// to enter, which is what shuts the gate against a fresh
    /// arrival.
    fn waiting_to_enter(store: &Store<()>, instance: InstanceId) -> u32 {
        store
            .internal_ref()
            .tables()
            .lock()
            .expect("tables")
            .tasks
            .instance(instance)
            .expect("instance record")
            .waiting_to_enter
    }

    /// How many threads `set` counts as waiting on it.
    fn waiters_on(store: &Store<()>, set: WaitableSetId) -> u32 {
        store
            .internal_ref()
            .tables()
            .lock()
            .expect("tables")
            .tasks
            .waitable_set(set)
            .expect("set record")
            .num_waiting
    }

    #[wcmp_macros::test]
    fn it_drops_every_item_of_a_task_whose_record_leaves_the_store() {
        // One task's work sits in every place an item can wait, with
        // another task's work beside it in each. Ending the first
        // task takes its items and nothing else.
        let mut store = store();
        let log = log();
        let instance = instance(&store);
        let (dead, set, thread) = task_waiting_on_a_set(&store, instance);
        let live = task(&store, instance);

        // The gate is shut, so the start of each task waits at it.
        set_backpressure(&store, instance, 1);
        for (task, name) in [(dead, "dead gated"), (live, "live gated")] {
            store
                .internal()
                .context()
                .internal()
                .start_export_thread(
                    task,
                    instance,
                    true,
                    true,
                    marker(&log, name).for_task(task),
                )
                .expect("queue the task's start");
        }
        store
            .internal()
            .scheduler_mut()
            .switch_to(marker(&log, "dead switch").for_task(dead));
        for (task, name) in [(dead, "dead high"), (live, "live high")] {
            store
                .internal()
                .scheduler_mut()
                .push_high_priority(marker(&log, name).for_task(task));
        }
        for (task, name) in [(dead, "dead low"), (live, "live low")] {
            store
                .internal()
                .scheduler_mut()
                .push_low_priority(marker(&log, name).for_task(task));
        }
        store.internal().scheduler_mut().hold_for_event(
            instance,
            thread,
            set,
            EventSlot::new(),
            marker(&log, "dead held").for_task(dead),
        );
        store.internal().scheduler_mut().hold_for_exclusive(
            instance,
            EventSlot::new(),
            marker(&log, "live held").for_task(live),
        );
        assert_eq!(store.internal().scheduler().queued_items(), 9);
        assert_eq!(waiting_to_enter(&store, instance), 2);
        assert_eq!(waiters_on(&store, set), 1);

        {
            let tables = store.internal().tables_handle();
            let mut guard = tables.lock().expect("tables");
            store
                .internal()
                .scheduler_mut()
                .discard_task_items(&mut guard.tasks, dead);
        }

        assert_eq!(
            store.internal().scheduler().queued_items(),
            4,
            "the five items of the task that ended went, and the four beside them stayed"
        );
        assert_eq!(
            store.internal().scheduler().waiting_at_gate(),
            1,
            "the start the gate held for the task that ended went with it"
        );
        assert_eq!(
            waiting_to_enter(&store, instance),
            1,
            "and the gate no longer counts it, so a later call is not held behind it"
        );
        assert_eq!(
            store.internal().scheduler().held_callbacks(),
            1,
            "the held callback of the task that ended went with it"
        );
        assert_eq!(
            waiters_on(&store, set),
            0,
            "and the wait it was held for ended, so the set has no waiter left"
        );

        // What is left runs, in the order it was queued. The switch
        // slot is empty, because the one item it held was the dead
        // task's.
        while let Some(item) = store.internal().scheduler_mut().take_ready() {
            item.run(&mut store.internal().context())
                .expect("the item runs");
        }
        while let Some(item) = store.internal().scheduler_mut().take_deferred() {
            item.run(&mut store.internal().context())
                .expect("the item runs");
        }
        assert_eq!(entries(&log), vec!["live high", "live low"]);
    }

    #[wcmp_macros::test]
    fn it_sweeps_the_items_of_an_abandoned_task_only_once_the_task_has_ended() {
        // A task parked between events is not on the stack of
        // scopes, so a failure that abandons it by name ends
        // nothing: its record stays in the store and the items that
        // name it are work it is still to do. Sweeping them would be
        // the mirror of the bug the sweep fixed — a live task's
        // pending work dropped under it.
        let mut store = store();
        let log = log();
        let instance = instance(&store);
        let (parked, set, thread) = task_waiting_on_a_set(&store, instance);

        store.internal().scheduler_mut().hold_for_event(
            instance,
            thread,
            set,
            EventSlot::new(),
            marker(&log, "parked held").for_task(parked),
        );
        store
            .internal()
            .scheduler_mut()
            .push_high_priority(marker(&log, "parked ready").for_task(parked));
        assert_eq!(store.internal().scheduler().queued_items(), 2);

        store
            .internal()
            .context()
            .internal()
            .abandon_export_task(parked)
            .expect("abandon the parked task");

        assert!(
            task_is_in_the_store(&store, parked),
            "the abandon ended nothing, because the task's scope is not \
             on the stack"
        );
        assert_eq!(
            store.internal().scheduler().queued_items(),
            2,
            "so what the task has queued is still its own pending work"
        );
        assert_eq!(
            waiters_on(&store, set),
            1,
            "and the wait its held callback was held for is still on"
        );

        // The same abandon with the task's scope on the stack does
        // end it, and the sweep is the second half of that end.
        store
            .internal()
            .context()
            .internal()
            .enter_export_task(parked)
            .expect("the task becomes the current scope");
        store
            .internal()
            .context()
            .internal()
            .abandon_export_task(parked)
            .expect("abandon the entered task");

        assert!(
            !task_is_in_the_store(&store, parked),
            "the abandon ended the task this time"
        );
        assert_eq!(
            store.internal().scheduler().queued_items(),
            0,
            "so both of its items went with it"
        );
        assert_eq!(
            waiters_on(&store, set),
            0,
            "and the wait the held callback was held for ended with it"
        );
        assert!(
            entries(&log).is_empty(),
            "neither item ever ran: the sweep drops them where they wait"
        );
    }

    /// Whether the store still holds a record for `task`.
    fn task_is_in_the_store(store: &Store<()>, task: TaskId) -> bool {
        store
            .internal_ref()
            .tables()
            .lock()
            .expect("tables")
            .tasks
            .task(task)
            .is_some()
    }

    /// Give `set` a waitable that holds an event: a subtask that
    /// started, the way a callee whose parameters were lifted leaves
    /// one.
    ///
    /// The record is moved to its started state as well as given the
    /// event, because delivery reads the state off the record rather
    /// than out of the slot.
    fn fill_event(store: &Store<()>, set: WaitableSetId) {
        let mut guard = store.internal_ref().tables().lock().expect("tables");
        let subtask = guard.tasks.insert_subtask();
        let waitable = guard.tasks.subtask_waitable(subtask);
        guard
            .tasks
            .join_waitable_set(waitable, Some(set))
            .expect("the subtask joins the set");
        guard.tasks.start_subtask(subtask);
        guard
            .tasks
            .set_pending_event(waitable, Event::subtask(3, SubtaskState::Started))
            .expect("the subtask is ready");
    }

    #[wcmp_macros::test]
    fn it_holds_a_callback_item_until_its_set_holds_an_event() {
        let mut store = store();
        let log = log();
        let instance = instance(&store);
        let (set, thread) = waiting_on_a_set(&store, instance);
        let slot = EventSlot::new();
        store.internal().scheduler_mut().hold_for_event(
            instance,
            thread,
            set,
            slot.clone(),
            marker(&log, "resumed"),
        );

        let parked = store
            .internal()
            .turn(Waker::noop())
            .expect("turn with an empty set");

        assert_eq!(
            parked,
            Outcome::Idle,
            "a held item is not ready, so the turn goes idle and its driver deadlocks"
        );
        assert!(entries(&log).is_empty());
        assert_eq!(store.internal().scheduler().held_callbacks(), 1);

        fill_event(&store, set);
        let woken = store
            .internal()
            .turn(Waker::noop())
            .expect("turn with a filled set");

        assert_eq!(woken, Outcome::Idle, "the item ran and nothing is left");
        assert_eq!(entries(&log), vec!["resumed"]);
        assert_eq!(store.internal().scheduler().held_callbacks(), 0);
        assert_eq!(
            slot.take().triple(),
            (1, 3, 1),
            "the item was queued with the event the set delivered"
        );
        assert_eq!(
            store
                .internal()
                .tables()
                .lock()
                .expect("tables")
                .tasks
                .waitable_set(set)
                .expect("set record")
                .num_waiting,
            0,
            "the wait ended as the item was queued"
        );
    }

    /// Give a waitable an event first and join it to `set` after,
    /// which fills the set through the join rather than through the
    /// event.
    fn join_with_event(store: &Store<()>, set: WaitableSetId) {
        let mut guard = store.internal_ref().tables().lock().expect("tables");
        let subtask = guard.tasks.insert_subtask();
        let waitable = guard.tasks.subtask_waitable(subtask);
        guard.tasks.start_subtask(subtask);
        guard
            .tasks
            .set_pending_event(waitable, Event::subtask(4, SubtaskState::Started))
            .expect("the subtask is ready");
        guard
            .tasks
            .join_waitable_set(waitable, Some(set))
            .expect("the subtask joins the set");
    }

    #[wcmp_macros::test]
    fn it_releases_a_callback_item_when_a_waitable_holding_an_event_joins_its_set() {
        let mut store = store();
        let log = log();
        let instance = instance(&store);
        let (set, thread) = waiting_on_a_set(&store, instance);
        let slot = EventSlot::new();
        store.internal().scheduler_mut().hold_for_event(
            instance,
            thread,
            set,
            slot.clone(),
            marker(&log, "resumed"),
        );
        store.internal().turn(Waker::noop()).expect("turn");
        assert_eq!(store.internal().scheduler().held_callbacks(), 1);

        join_with_event(&store, set);
        store.internal().turn(Waker::noop()).expect("turn");

        assert_eq!(entries(&log), vec!["resumed"]);
        assert_eq!(
            slot.take().triple(),
            (1, 4, 1),
            "the join filled the set, and the item was queued with its event"
        );
    }

    #[wcmp_macros::test]
    fn it_queues_callback_items_released_together_in_the_order_they_were_held() {
        let mut store = store();
        let log = log();
        let instance = instance(&store);
        let (first_set, first_thread) = waiting_on_a_set(&store, instance);
        let (second_set, second_thread) = waiting_on_a_set(&store, instance);
        let (idle_set, idle_thread) = waiting_on_a_set(&store, instance);
        for (set, thread, name) in [
            (first_set, first_thread, "held first"),
            (idle_set, idle_thread, "never signalled"),
            (second_set, second_thread, "held second"),
        ] {
            store.internal().scheduler_mut().hold_for_event(
                instance,
                thread,
                set,
                EventSlot::new(),
                marker(&log, name),
            );
        }
        store.internal().turn(Waker::noop()).expect("turn");

        // The sets are signalled in the opposite order to the holds.
        fill_event(&store, second_set);
        fill_event(&store, first_set);
        store.internal().turn(Waker::noop()).expect("turn");

        assert_eq!(
            entries(&log),
            vec!["held first", "held second"],
            "the items the two signals released ran in the order they were held"
        );
        assert_eq!(
            store.internal().scheduler().held_callbacks(),
            1,
            "the item whose set nothing signalled is still held"
        );
    }

    #[wcmp_macros::test]
    fn it_does_not_examine_a_held_item_whose_set_was_not_signalled_since_the_last_look() {
        let mut store = store();
        let log = log();
        let instance = instance(&store);
        let (set, thread) = waiting_on_a_set(&store, instance);
        store.internal().scheduler_mut().hold_for_event(
            instance,
            thread,
            set,
            EventSlot::new(),
            marker(&log, "held"),
        );
        store.internal().turn(Waker::noop()).expect("turn");

        // The set takes on an event, and its signal is taken before
        // the turn looks, so the set holds an event the list of
        // signalled sets does not name.
        fill_event(&store, set);
        store
            .internal()
            .tables()
            .lock()
            .expect("tables")
            .tasks
            .take_signalled_sets();
        store.internal().turn(Waker::noop()).expect("turn");

        assert!(
            entries(&log).is_empty(),
            "the turn did not look at the item, though its set holds an event"
        );
        assert_eq!(store.internal().scheduler().held_callbacks(), 1);

        fill_event(&store, set);
        store.internal().turn(Waker::noop()).expect("turn");

        assert_eq!(
            entries(&log),
            vec!["held"],
            "the next signal of the set brought the item to the turn's look"
        );
    }

    #[wcmp_macros::test]
    fn it_takes_the_signalled_sets_in_a_store_that_holds_no_callback_item() {
        let mut store = store();
        let instance = instance(&store);
        for _ in 0..16 {
            let (set, _thread) = waiting_on_a_set(&store, instance);
            fill_event(&store, set);
            assert_eq!(
                store
                    .internal()
                    .tables()
                    .lock()
                    .expect("tables")
                    .tasks
                    .signalled_set_count(),
                1,
                "the event put the set on the list"
            );
            store.internal().turn(Waker::noop()).expect("turn");
            assert_eq!(
                store
                    .internal()
                    .tables()
                    .lock()
                    .expect("tables")
                    .tasks
                    .signalled_set_count(),
                0,
                "the turn took the list though no callback item was held"
            );
        }
        assert_eq!(store.internal().scheduler().held_callbacks(), 0);
    }

    #[wcmp_macros::test]
    fn it_holds_a_callback_item_until_the_exclusive_thread_is_free() {
        let mut store = store();
        let log = log();
        let instance = instance(&store);
        // Another task of the same instance holds it, which is what a
        // callback item that a turn reaches too early finds.
        let holder = start(
            &mut store.internal().context(),
            instance,
            true,
            true,
            marker(&log, "holder"),
        );
        store.internal().scheduler_mut().hold_for_exclusive(
            instance,
            EventSlot::holding(Event::none()),
            marker(&log, "deferred"),
        );

        let held = store
            .internal()
            .turn(Waker::noop())
            .expect("turn with the instance taken");

        assert_eq!(
            entries(&log),
            vec!["holder"],
            "the item that needed the instance did not run"
        );
        assert_eq!(held, Outcome::Idle);
        assert_eq!(store.internal().scheduler().held_callbacks(), 1);

        store.internal_ref().scheduler().release_exclusive_thread(
            &mut store.internal_ref().tables().lock().expect("tables").tasks,
            holder,
        );
        store
            .internal()
            .turn(Waker::noop())
            .expect("turn with it released");

        assert_eq!(
            entries(&log),
            vec!["holder", "deferred"],
            "the item ran once the holder released the instance"
        );
        assert_eq!(store.internal().scheduler().held_callbacks(), 0);
    }

    #[wcmp_macros::test]
    fn it_keeps_the_other_callbacks_held_when_one_fails_to_release() {
        let mut store = store();
        let log = log();
        let instance = instance(&store);
        // A task of the instance holds it, so neither item held for
        // the exclusive thread is ready in the turn that fails.
        let holder = start(
            &mut store.internal().context(),
            instance,
            true,
            true,
            marker(&log, "holder"),
        );
        let (set, thread) = waiting_on_a_set(&store, instance);
        store.internal().scheduler_mut().hold_for_exclusive(
            instance,
            EventSlot::holding(Event::none()),
            marker(&log, "first"),
        );
        store.internal().scheduler_mut().hold_for_event(
            instance,
            thread,
            set,
            EventSlot::new(),
            marker(&log, "broken"),
        );
        store.internal().scheduler_mut().hold_for_exclusive(
            instance,
            EventSlot::holding(Event::none()),
            marker(&log, "last"),
        );
        // The set the middle item waits on leaves the store under it,
        // so looking the set up fails. A guest cannot drop a set a
        // thread waits on, so the test ends the wait first.
        {
            let mut guard = store.internal().tables().lock().expect("tables");
            guard.tasks.end_wait(set, thread).expect("end the wait");
            guard.tasks.drop_waitable_set(set).expect("drop the set");
        }

        store
            .internal()
            .turn(Waker::noop())
            .expect_err("the set the held item named is gone");

        assert!(entries(&log).is_empty(), "the turn failed before it ran");
        assert_eq!(
            store.internal().scheduler().held_callbacks(),
            2,
            "only the item whose own release failed was given up"
        );

        store.internal_ref().scheduler().release_exclusive_thread(
            &mut store.internal_ref().tables().lock().expect("tables").tasks,
            holder,
        );
        store
            .internal()
            .turn(Waker::noop())
            .expect("turn with it released");

        assert_eq!(
            entries(&log),
            vec!["holder", "first", "last"],
            "both surviving items ran, in the order they were held"
        );
        assert_eq!(store.internal().scheduler().held_callbacks(), 0);
    }

    /// A host task's body that never completes, so the store that
    /// holds it holds a future that can still resolve.
    struct NeverReady;

    impl Future for NeverReady {
        type Output = Result<Vec<Val>>;

        fn poll(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Self::Output> {
            Poll::Pending
        }
    }

    /// Give `store` a host task whose body never completes.
    fn pending_host_task(store: &mut StoreContext<'_, ()>) {
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
            NeverReady,
        ));
    }

    #[wcmp_macros::test]
    fn it_reports_a_host_future_pending_for_a_host_task_of_the_store() {
        let mut owner = store();
        let mut store = owner.internal().context();

        assert!(
            !store.internal().scheduler().host_future_pending(),
            "a fresh store holds no future that can still resolve"
        );

        pending_host_task(&mut store);
        assert!(
            store.internal().scheduler().host_future_pending(),
            "a host task whose body wants another poll is a future that can \
             still resolve"
        );

        store.internal().scheduler_mut().take_host_tasks();
        assert!(
            !store.internal().scheduler().host_future_pending(),
            "the store gave its host tasks away, so it holds no future"
        );
    }

    #[wcmp_macros::test]
    fn it_reports_a_host_future_pending_for_a_call_a_synchronous_lower_parked() {
        let mut owner = store();
        let mut store = owner.internal().context();
        let subtask = store
            .internal()
            .tables()
            .lock()
            .expect("tables")
            .tasks
            .insert_subtask();

        store
            .internal()
            .scheduler_mut()
            .park_call(HostTask::from_future(
                subtask,
                |_store: &mut StoreContext<'_, ()>, _outcome: Result<Vec<Val>>| Ok(()),
                NeverReady,
            ));

        assert!(
            store.internal().scheduler().host_future_pending(),
            "the parked future of a synchronous lower is one of the store's \
             host tasks, so the store knows it is pending"
        );
        assert!(store.internal().scheduler().holds_host_task(subtask));
        assert!(store.internal().scheduler().is_parked_call(subtask));

        store.internal().scheduler_mut().withdraw_call(subtask);

        assert!(
            !store.internal().scheduler().host_future_pending(),
            "the lower withdrew its call, so the store holds no future"
        );
        assert!(!store.internal().scheduler().is_parked_call(subtask));
    }

    #[wcmp_macros::test]
    fn it_reports_no_host_future_pending_for_an_item_the_store_holds() {
        let mut owner = store();
        let mut store = owner.internal().context();
        let log = log();
        store
            .internal()
            .scheduler_mut()
            .push_low_priority(marker(&log, "deferred"));

        assert!(
            !store.internal().scheduler().host_future_pending(),
            "a resumption after a yield is guest work, and a turn that runs \
             it is what moves the store, not an executor's poll"
        );
    }

    #[wcmp_macros::test]
    fn it_forgets_the_waker_of_a_host_end_it_lets_go_of() {
        let mut store = store();
        let scheduler = store.internal().scheduler_mut();
        let (writer, reader) = (EndId::new(1, 0), EndId::new(2, 0));
        scheduler.set_host_end_waker(writer, Waker::noop().clone());
        scheduler.set_host_end_waker(reader, Waker::noop().clone());

        assert!(scheduler.release_host_writer(writer).is_none());
        assert!(scheduler.release_host_reader(reader).is_none());
        assert!(
            scheduler.take_host_end_waker(writer).is_none()
                && scheduler.take_host_end_waker(reader).is_none(),
            "an end let go of keeps no waker for a cancel to wake"
        );
    }
}
