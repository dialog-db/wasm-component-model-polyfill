//! The store's ready queues, its host tasks, and its entry gate.

use std::collections::VecDeque;

use super::SuspendSeam;
use super::host_task::HostTask;
use super::instance_id::InstanceId;
use super::item::Item;
use super::task_id::TaskId;
use super::task_tables::TaskTables;

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
    host_tasks: Vec<HostTask<T>>,
    suspend_seam: SuspendSeam<T>,
}

impl<T: 'static> Scheduler<T> {
    /// Construct a scheduler with nothing queued.
    pub fn new() -> Self {
        Self {
            switch_slot: None,
            high_priority: VecDeque::new(),
            low_priority: VecDeque::new(),
            resume_after_yield: None,
            entry_gate: VecDeque::new(),
            host_tasks: Vec::new(),
            suspend_seam: SuspendSeam::new(),
        }
    }

    /// The store's one suspend capability: the seam a blocking
    /// built-in asks to suspend the current guest thread until a
    /// readiness condition holds.
    pub fn suspend_seam(&self) -> &SuspendSeam<T> {
        &self.suspend_seam
    }

    /// The store's one suspend capability, mutably, which is how a
    /// target fills its provider slot.
    pub fn suspend_seam_mut(&mut self) -> &mut SuspendSeam<T> {
        &mut self.suspend_seam
    }

    /// Give `task` to the store. A host task that joined since the
    /// last turn counts as woken, so the next turn polls it.
    pub fn push_host_task(&mut self, task: HostTask<T>) {
        self.host_tasks.push(task);
    }

    /// Take every host task out, so a turn can poll them while it
    /// holds the store. The ones that are still pending go back
    /// through [`restore_host_tasks`](Self::restore_host_tasks).
    pub fn take_host_tasks(&mut self) -> Vec<HostTask<T>> {
        core::mem::take(&mut self.host_tasks)
    }

    /// Put the host tasks that are still pending back, ahead of any
    /// task that joined while they were being polled.
    pub fn restore_host_tasks(&mut self, mut pending: Vec<HostTask<T>>) {
        pending.append(&mut self.host_tasks);
        self.host_tasks = pending;
    }

    /// How many host tasks the store holds.
    pub fn host_task_count(&self) -> usize {
        self.host_tasks.len()
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
    /// `instance`, by running `item`.
    ///
    /// This is the reference's `enter_implicit_thread`. A task of a
    /// synchronous export ignores the gate and becomes ready at once,
    /// as the reference states. A task of an `async` export waits at
    /// the gate when the instance's backpressure is set, when it
    /// needs the exclusive thread and one is set, or when tasks are
    /// already waiting — the last so that a fresh arrival cannot
    /// overtake a task that is already queued. `needs_exclusive` is
    /// the reference's `not opts.async or opts.callback`: a task
    /// lifted synchronously and a callback task each need the
    /// instance to themselves.
    pub fn enter_implicit_thread(
        &mut self,
        tables: &mut TaskTables,
        task: TaskId,
        instance: InstanceId,
        async_function: bool,
        needs_exclusive: bool,
        item: Item<T>,
    ) {
        if !async_function {
            self.high_priority.push_back(item);
            return;
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
            return;
        }
        Self::claim_exclusive(tables, task, instance, needs_exclusive);
        self.high_priority.push_back(item);
    }

    /// Release every task the entry gate can let through, in arrival
    /// order. A task whose instance is still blocked stays, and so
    /// does every task of that instance behind it, so the tasks of
    /// one instance start in the order they arrived.
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

    /// How many items the store holds, ready or held at the gate.
    pub fn queued_items(&self) -> usize {
        usize::from(self.switch_slot.is_some())
            + self.high_priority.len()
            + self.low_priority.len()
            + usize::from(self.resume_after_yield.is_some())
            + self.entry_gate.len()
    }

    /// How many tasks wait at an entry gate.
    pub fn waiting_at_gate(&self) -> usize {
        self.entry_gate.len()
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
        let Some(record) = tables.task(task) else {
            return;
        };
        let (thread, instance) = (record.implicit_thread, record.instance);
        let Some(record) = tables.instance_mut(instance) else {
            return;
        };
        if record.exclusive_thread == Some(thread) {
            record.exclusive_thread = None;
        }
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
    use core::task::Waker;
    use std::sync::{Arc, Mutex};

    use crate::engine::Engine;
    use crate::store::Store;

    use super::super::instance_id::InstanceId;
    use super::super::item::Item;
    use super::super::item_kind::ItemKind;
    use super::super::outcome::Outcome;
    use super::super::task_id::TaskId;

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
        Item::new(ItemKind::TaskStart, move |_store: &mut Store<()>| {
            log.lock().expect("log").push(name);
            Ok(())
        })
    }

    fn entries(log: &Log) -> Vec<&'static str> {
        log.lock().expect("log").clone()
    }

    /// A component instance record with nothing set.
    fn instance(store: &Store<()>) -> InstanceId {
        store.tables.lock().expect("tables").tasks.insert_instance()
    }

    /// Raise or lower the backpressure of `instance`, which is what
    /// shuts and opens its entry gate.
    fn set_backpressure(store: &Store<()>, instance: InstanceId, value: u32) {
        store
            .tables
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
        store: &mut Store<()>,
        instance: InstanceId,
        async_function: bool,
        needs_exclusive: bool,
        build: impl FnOnce(TaskId) -> Item<()>,
    ) -> TaskId {
        let task = store
            .tables
            .lock()
            .expect("tables")
            .tasks
            .create_task(None, None, instance);
        store
            .start_export_thread(task, instance, async_function, needs_exclusive, build(task))
            .expect("queue the task's start");
        task
    }

    /// Queue the start of a fresh task of `instance`.
    fn start(
        store: &mut Store<()>,
        instance: InstanceId,
        async_function: bool,
        needs_exclusive: bool,
        item: Item<()>,
    ) -> TaskId {
        start_with(store, instance, async_function, needs_exclusive, |_| item)
    }

    /// An item that runs one export call the way `Func::run_task`
    /// runs it, without a guest: the task becomes the current scope,
    /// it resolves with no result, and then it exits. Nothing else
    /// stands between a call returning and the instance going back,
    /// so a test that ends a call this way is the ordering the
    /// export path really takes.
    fn export_call(log: &Log, name: &'static str, task: TaskId) -> Item<()> {
        let log = log.clone();
        Item::new(ItemKind::TaskStart, move |store: &mut Store<()>| {
            log.lock().expect("log").push(name);
            store.enter_export_task(task)?;
            store.resolve_export_task(task, None)?;
            store
                .exit_export_task(task)?
                .expect("the call dropped every borrow it took");
            Ok(())
        })
    }

    #[test]
    fn it_runs_the_switch_slot_before_the_high_priority_queue() {
        let mut store = store();
        let log = log();
        store.scheduler.push_high_priority(marker(&log, "high"));
        store.scheduler.switch_to(marker(&log, "switch"));

        let outcome = store.turn(Waker::noop()).expect("turn");

        assert_eq!(outcome, Outcome::Idle, "both items ran and nothing is left");
        assert_eq!(
            entries(&log),
            vec!["switch", "high"],
            "the thread the scheduler must switch to next runs before fresh readiness"
        );
    }

    #[test]
    fn it_runs_the_high_priority_queue_in_the_order_items_became_ready() {
        let mut store = store();
        let log = log();
        store.scheduler.push_high_priority(marker(&log, "first"));
        store.scheduler.push_high_priority(marker(&log, "second"));

        store.turn(Waker::noop()).expect("turn");

        assert_eq!(entries(&log), vec!["first", "second"]);
    }

    #[test]
    fn it_yields_to_the_driver_before_it_runs_a_low_priority_item() {
        let mut store = store();
        let log = log();
        store.scheduler.push_low_priority(marker(&log, "low"));
        store.scheduler.push_high_priority(marker(&log, "high"));

        let first = store.turn(Waker::noop()).expect("first turn");

        assert_eq!(
            first,
            Outcome::Yield,
            "the low-priority item gives way and the turn ends"
        );
        assert_eq!(
            entries(&log),
            vec!["high"],
            "the high-priority queue drains first, and the low-priority item has not run"
        );

        let second = store.turn(Waker::noop()).expect("second turn");

        assert_eq!(second, Outcome::Idle);
        assert_eq!(
            entries(&log),
            vec!["high", "low"],
            "the resumption runs at the top of the next turn"
        );
    }

    #[test]
    fn it_lets_a_synchronous_task_past_a_shut_entry_gate() {
        let mut store = store();
        let log = log();
        let instance = instance(&store);
        set_backpressure(&store, instance, 1);

        start(&mut store, instance, false, true, marker(&log, "sync"));
        store.turn(Waker::noop()).expect("turn");

        assert_eq!(
            store.scheduler.waiting_at_gate(),
            0,
            "a task of a synchronous export never waits at the gate"
        );
        assert_eq!(entries(&log), vec!["sync"]);
    }

    #[test]
    fn it_starts_tasks_held_at_the_entry_gate_in_arrival_order() {
        let mut store = store();
        let log = log();
        let instance = instance(&store);
        set_backpressure(&store, instance, 1);

        for name in ["first", "second", "third"] {
            start(&mut store, instance, true, false, marker(&log, name));
        }

        let shut = store.turn(Waker::noop()).expect("turn with the gate shut");

        assert_eq!(
            shut,
            Outcome::Idle,
            "nothing is ready while the gate holds every task"
        );
        assert_eq!(store.scheduler.waiting_at_gate(), 3);
        assert!(entries(&log).is_empty(), "no task's thread has run");

        set_backpressure(&store, instance, 0);
        let open = store.turn(Waker::noop()).expect("turn with the gate open");

        assert_eq!(open, Outcome::Idle);
        assert_eq!(store.scheduler.waiting_at_gate(), 0);
        assert_eq!(
            entries(&log),
            vec!["first", "second", "third"],
            "the tasks start in the order they arrived at the gate"
        );
    }

    #[test]
    fn it_holds_a_task_behind_one_that_already_waits_at_the_gate() {
        let mut store = store();
        let log = log();
        let instance = instance(&store);
        set_backpressure(&store, instance, 1);
        start(&mut store, instance, true, false, marker(&log, "early"));
        set_backpressure(&store, instance, 0);

        // Nothing blocks the second task, but one is already
        // waiting, so it queues behind it rather than overtaking it.
        start(&mut store, instance, true, false, marker(&log, "late"));
        store.turn(Waker::noop()).expect("turn");

        assert_eq!(entries(&log), vec!["early", "late"]);
    }

    #[test]
    fn it_gives_the_instance_to_a_task_that_needs_it_exclusively() {
        let mut store = store();
        let log = log();
        let instance = instance(&store);

        start(&mut store, instance, true, true, marker(&log, "exclusive"));
        start(&mut store, instance, true, true, marker(&log, "queued"));
        store.turn(Waker::noop()).expect("turn");

        assert_eq!(
            entries(&log),
            vec!["exclusive"],
            "the second task waits for the exclusive thread the first took"
        );
        assert_eq!(store.scheduler.waiting_at_gate(), 1);
    }

    #[test]
    fn it_releases_the_instance_when_the_holding_tasks_call_ends() {
        let mut store = store();
        let log = log();
        let instance = instance(&store);

        // The first task's item ends its call the way the export
        // path ends one: it resolves the task and exits it. That
        // exit is what ends the task's implicit thread, so the
        // instance goes back while the task record is still there
        // to say which thread held it.
        start_with(&mut store, instance, true, true, |task| {
            export_call(&log, "exclusive", task)
        });
        start(&mut store, instance, true, true, marker(&log, "queued"));

        store.turn(Waker::noop()).expect("turn");

        assert_eq!(
            entries(&log),
            vec!["exclusive", "queued"],
            "the task at the gate took the instance the ended call gave back"
        );
        assert_eq!(store.scheduler.waiting_at_gate(), 0);
    }

    #[test]
    fn it_releases_the_instance_when_the_holding_tasks_call_fails() {
        let mut store = store();
        let log = log();
        let instance = instance(&store);

        // The same release on the failure path: a call that trapped
        // is abandoned rather than exited, and the instance it held
        // goes back all the same.
        start_with(&mut store, instance, true, true, |task| {
            let log = log.clone();
            Item::new(ItemKind::TaskStart, move |store: &mut Store<()>| {
                log.lock().expect("log").push("abandoned");
                store.enter_export_task(task)?;
                store.abandon_export_task(task)
            })
        });
        start(&mut store, instance, true, true, marker(&log, "queued"));

        store.turn(Waker::noop()).expect("turn");

        assert_eq!(
            entries(&log),
            vec!["abandoned", "queued"],
            "the task at the gate took the instance the failed call gave back"
        );
        assert_eq!(store.scheduler.waiting_at_gate(), 0);
    }
}
