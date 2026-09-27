//! The store's host tasks, and which of them a turn has to poll.

use core::task::Waker;
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Wake;

use super::host_task::HostTask;
use super::subtask_id::SubtaskId;

/// The store's host tasks, each with a waker of its own, and the
/// order in which they were woken.
///
/// A turn polls only the tasks woken since the turn before it, which
/// is the shape of `FuturesUnordered`: a store holding many host
/// calls that are waiting on something outside it pays nothing per
/// turn for the ones that were not woken.
///
/// Each task is polled with its own waker. Waking it puts the task at
/// the back of the ready queue, unless it is already there, and then
/// wakes the waker of the driver that polls the store, so the wake
/// reaches the executor as it always did and the next turn knows
/// which task it was for. The queue keeps the order the wakes came
/// in: tasks woken in one turn are polled in the next in the order
/// they were woken, never in an order a hash would give.
///
/// A task that joins the set counts as woken, so the turn after it
/// joined polls it. That is what keeps the first poll a trampoline
/// makes before it returns to the guest correct: that poll carries
/// the waker of the running turn, or one that does nothing when no
/// turn is running, rather than the task's own, and a wake sent to
/// either would otherwise never mark the task. The poll the next
/// turn makes carries the task's own waker, and every poll after it
/// does too.
///
/// The task of each call the set holds, out being polled or not, is
/// a record against the store's cap on its live records, as the host
/// task of a call is an entry of Wasmtime's table. The task of a copy
/// is not, as Wasmtime's host side of a copy is not. The set keeps
/// how many calls it holds in a number it shares with the store's
/// record tables, which count it with their own records.
pub struct HostTaskSet<T: 'static> {
    tasks: BTreeMap<u64, Entry<T>>,
    /// How many calls the set holds, shared with the store's record
    /// tables.
    calls: Arc<AtomicUsize>,
    /// How many entries have their task out being polled.
    out: usize,
    next_key: u64,
    /// The keys below this one are retired: the set let go of every
    /// task under them at once, and a task a turn had out then does
    /// not come back when its poll returns.
    retired_below: u64,
    queue: Arc<ReadyQueue>,
}

/// One host task the set holds, with the waker it is polled with.
struct Entry<T: 'static> {
    /// The task, or `None` while a turn has it out to poll.
    task: Option<HostTask<T>>,
    /// Whether the task is a call's rather than a copy's.
    call: bool,
    wake: Arc<TaskWake>,
    waker: Waker,
}

/// The half of the set a waker reaches: the keys of the woken tasks,
/// in the order they were woken, and the waker of the driver to pass
/// each wake on to.
#[derive(Default)]
struct ReadyQueue {
    woken: Mutex<VecDeque<u64>>,
    driver: Mutex<Option<Waker>>,
}

/// The waker of one host task.
struct TaskWake {
    key: u64,
    /// Whether the task's key is in the ready queue already, so a
    /// task woken twice before a turn takes it is polled once.
    queued: AtomicBool,
    queue: Arc<ReadyQueue>,
}

impl Wake for TaskWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.enqueue();
        // The driver's waker is cloned out before it is woken, so
        // that whatever the wake runs does not meet the lock held.
        let driver = self.queue.driver.lock().ok().and_then(|held| held.clone());
        if let Some(driver) = driver {
            driver.wake();
        }
    }
}

impl TaskWake {
    /// Put the task at the back of the ready queue, unless it is in
    /// the queue already.
    fn enqueue(&self) {
        if self.queued.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Ok(mut woken) = self.queue.woken.lock() {
            woken.push_back(self.key);
        }
    }
}

impl<T: 'static> HostTaskSet<T> {
    /// An empty set.
    pub fn new() -> Self {
        Self {
            tasks: BTreeMap::new(),
            calls: Arc::new(AtomicUsize::new(0)),
            out: 0,
            next_key: 0,
            retired_below: 0,
            queue: Arc::new(ReadyQueue::default()),
        }
    }

    /// Give `task` to the set, and answer the key it is held under.
    /// It counts as woken, so the next turn polls it.
    pub fn push(&mut self, task: HostTask<T>) -> u64 {
        let key = self.next_key;
        self.next_key += 1;
        let wake = Arc::new(TaskWake {
            key,
            queued: AtomicBool::new(false),
            queue: self.queue.clone(),
        });
        wake.enqueue();
        let waker = Waker::from(wake.clone());
        let call = task.subtask().is_some();
        if call {
            self.calls.fetch_add(1, Ordering::AcqRel);
        }
        self.tasks.insert(
            key,
            Entry {
                call,
                task: Some(task),
                wake,
                waker,
            },
        );
        key
    }

    /// The number of calls the set holds, out being polled or not,
    /// which the set keeps as it changes. The store's record tables
    /// hold it to count the calls against the cap on live records.
    pub fn call_count(&self) -> Arc<AtomicUsize> {
        self.calls.clone()
    }

    /// Take `entry`, which has left the set, off the shared number
    /// of calls when it is a call's.
    fn forget(&self, entry: &Entry<T>) {
        if entry.call {
            self.calls.fetch_sub(1, Ordering::AcqRel);
        }
    }

    /// Record `waker` as the waker of the driver that polls the
    /// store, which is where every task's wake is passed on to.
    pub fn watch(&self, waker: &Waker) {
        if let Ok(mut driver) = self.queue.driver.lock()
            && driver.as_ref().is_none_or(|kept| !kept.will_wake(waker))
        {
            *driver = Some(waker.clone());
        }
    }

    /// Take out every task woken since the last take, in the order
    /// they were woken, each with its key and the waker to poll it
    /// with. A task woken again after this, while it is out or once
    /// it is back, is taken by the first take that finds it back.
    ///
    /// A take can find the key of a task that is out: a turn nested
    /// inside the poll of a host task takes while the outer turn has
    /// that task out. Such a key stays at the front of the queue,
    /// still marked as queued, so the take after the task is back
    /// polls it. Dropping the key instead would leave the mark set
    /// with no key behind it, and every later wake of the task would
    /// stop at the mark.
    ///
    /// Each task handed out goes back through
    /// [`restore`](Self::restore) when it is still pending, and
    /// leaves through [`complete`](Self::complete) when it is not.
    /// While it is out it does not count as one the set holds.
    pub fn take_woken(&mut self) -> Vec<(u64, Waker, HostTask<T>)> {
        let woken = match self.queue.woken.lock() {
            Ok(mut woken) => core::mem::take(&mut *woken),
            Err(_) => return Vec::new(),
        };
        let mut taken = Vec::with_capacity(woken.len());
        let mut kept = Vec::new();
        for key in woken {
            // A key with no task behind it is a wake that came after
            // the task left the set, and there is nothing to poll.
            let Some(entry) = self.tasks.get_mut(&key) else {
                continue;
            };
            let Some(task) = entry.task.take() else {
                kept.push(key);
                continue;
            };
            // The mark comes off before the poll, so a wake the poll
            // itself sends queues the task for the next take.
            entry.wake.queued.store(false, Ordering::Release);
            self.out += 1;
            taken.push((key, entry.waker.clone(), task));
        }
        // The kept keys were woken before any wake that came in since
        // the queue was taken, so they go back ahead of those.
        if !kept.is_empty()
            && let Ok(mut woken) = self.queue.woken.lock()
        {
            for key in kept.into_iter().rev() {
                woken.push_front(key);
            }
        }
        taken
    }

    /// Put back a task [`take_woken`](Self::take_woken) handed out
    /// under `key`, which is still pending.
    pub fn restore(&mut self, key: u64, task: HostTask<T>) {
        if let Some(entry) = self.tasks.get_mut(&key)
            && entry.task.is_none()
        {
            entry.task = Some(task);
            self.out = self.out.saturating_sub(1);
        }
    }

    /// Let go of the task [`take_woken`](Self::take_woken) handed out
    /// under `key`, which has completed.
    pub fn complete(&mut self, key: u64) {
        let Some(entry) = self.tasks.remove(&key) else {
            return;
        };
        self.forget(&entry);
        if entry.task.is_none() {
            self.out = self.out.saturating_sub(1);
        }
    }

    /// Take out the task held under `key`, woken or not, with the
    /// waker to poll it with. `None` when the set holds no task under
    /// the key, or when a turn has it out already.
    ///
    /// The task's key leaves the ready queue and its mark comes off,
    /// as [`take_woken`](Self::take_woken) takes them off: a wake the
    /// poll sends queues the task for the next take, and a wake that
    /// came before it is answered by this poll. The task goes back
    /// through [`restore`](Self::restore) or leaves through
    /// [`complete`](Self::complete), as a task that one hands out
    /// does.
    pub fn take(&mut self, key: u64) -> Option<(Waker, HostTask<T>)> {
        let entry = self.tasks.get_mut(&key)?;
        let task = entry.task.take()?;
        if let Ok(mut woken) = self.queue.woken.lock() {
            woken.retain(|queued| *queued != key);
        }
        entry.wake.queued.store(false, Ordering::Release);
        self.out += 1;
        Some((entry.waker.clone(), task))
    }

    /// Take the task held under `key` out of the set for good,
    /// without polling it. `None` when the set holds no task under
    /// the key, or when a turn has it out.
    pub fn remove(&mut self, key: u64) -> Option<HostTask<T>> {
        // A task a turn has out stays: the turn puts it back or lets
        // it go itself.
        self.tasks.get(&key)?.task.as_ref()?;
        let entry = self.tasks.remove(&key)?;
        self.forget(&entry);
        entry.task
    }

    /// Whether the set holds the host task of `subtask`, woken or not.
    /// A task that is out being polled is not counted, as
    /// [`len`](Self::len) does not count it.
    pub fn holds(&self, subtask: SubtaskId) -> bool {
        self.tasks.values().any(|entry| {
            entry
                .task
                .as_ref()
                .is_some_and(|task| task.subtask() == Some(subtask))
        })
    }

    /// Take every task the set holds out, woken or not, in the order
    /// they joined.
    pub fn take_all(&mut self) -> Vec<HostTask<T>> {
        let mut taken = Vec::with_capacity(self.len());
        let calls = &self.calls;
        self.tasks.retain(|_, entry| match entry.task.take() {
            Some(task) => {
                if entry.call {
                    calls.fetch_sub(1, Ordering::AcqRel);
                }
                taken.push(task);
                false
            }
            None => true,
        });
        taken
    }

    /// Take every task the set holds out, as
    /// [`take_all`](Self::take_all) does, and retire every key given
    /// out so far. A task a turn has out being polled stays out: the
    /// turn finds its key retired when the poll returns, and lets the
    /// task go rather than putting it back. A task that joins after
    /// this is held as any other.
    pub fn retire_all(&mut self) -> Vec<HostTask<T>> {
        self.retired_below = self.next_key;
        self.take_all()
    }

    /// Whether the task given out under `key` was retired by
    /// [`retire_all`](Self::retire_all).
    pub fn is_retired(&self, key: u64) -> bool {
        key < self.retired_below
    }

    /// How many host tasks the set holds, woken or not. A task that
    /// is out being polled is not counted.
    pub fn len(&self) -> usize {
        self.tasks.len() - self.out
    }

    /// Whether the set holds no host task.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<T: 'static> Default for HostTaskSet<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use core::future::pending;

    use crate::error::Result;
    use crate::store::StoreContext;
    use crate::value::Val;

    use super::super::subtask_id::SubtaskId;
    use super::*;

    /// A host task that never completes. The set never polls what it
    /// holds, so the task only has to be one.
    fn parked(index: u32) -> HostTask<()> {
        HostTask::from_future(
            SubtaskId::new(index, 0),
            |_store: &mut StoreContext<'_, ()>, _outcome: Result<Vec<Val>>| Ok(()),
            pending::<Result<Vec<Val>>>(),
        )
    }

    /// Take the woken tasks, as a turn would, and put each back as
    /// still pending. Answers the keys of what the take handed out,
    /// with the waker of each.
    fn take_and_restore(set: &mut HostTaskSet<()>) -> Vec<(u64, Waker)> {
        let taken = set.take_woken();
        let mut keys = Vec::with_capacity(taken.len());
        for (key, waker, task) in taken {
            set.restore(key, task);
            keys.push((key, waker));
        }
        keys
    }

    /// The keys alone.
    fn keys(taken: &[(u64, Waker)]) -> Vec<u64> {
        taken.iter().map(|(key, _)| *key).collect()
    }

    #[wcmp_macros::test]
    fn it_polls_a_task_woken_while_out_on_the_take_after_it_is_back() {
        let mut set = HostTaskSet::new();
        set.push(parked(0));
        set.push(parked(1));

        // The outer turn has both tasks out, and the first is woken
        // while it is out.
        let out = set.take_woken();
        let (first, waker, _) = &out[0];
        let first = *first;
        waker.wake_by_ref();

        // A turn nested inside the outer turn's poll takes before the
        // task is back.
        assert!(
            set.take_woken().is_empty(),
            "the nested take had nothing it could hand out"
        );

        for (key, _, task) in out {
            set.restore(key, task);
        }
        let taken = take_and_restore(&mut set);
        assert_eq!(
            keys(&taken),
            vec![first],
            "the take after the task was back handed out the task the nested \
             take found out"
        );
        assert!(
            take_and_restore(&mut set).is_empty(),
            "the key was taken once, so a take with no wake in between hands \
             out nothing"
        );

        taken[0].1.wake_by_ref();
        assert_eq!(
            keys(&take_and_restore(&mut set)),
            vec![first],
            "a later wake of the task still reached the next take"
        );
        assert_eq!(set.len(), 2);
    }

    #[wcmp_macros::test]
    fn it_keeps_a_key_found_out_ahead_of_the_wakes_that_came_after_it() {
        let mut set = HostTaskSet::new();
        set.push(parked(0));
        set.push(parked(1));
        let taken = take_and_restore(&mut set);
        let (first, second) = (taken[0].0, taken[1].0);

        // The outer turn has the first task out, and it is woken
        // while out. A nested take finds it out, and then the second
        // task is woken.
        taken[0].1.wake_by_ref();
        let (key, waker, task) = set.take_woken().remove(0);
        assert_eq!(key, first);
        waker.wake_by_ref();
        assert!(set.take_woken().is_empty());
        taken[1].1.wake_by_ref();
        set.restore(key, task);

        assert_eq!(
            keys(&take_and_restore(&mut set)),
            vec![first, second],
            "the key the nested take kept went back ahead of the later wake"
        );
    }
}
