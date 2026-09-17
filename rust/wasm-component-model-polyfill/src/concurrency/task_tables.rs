//! The store's tables of task, subtask, thread, waitable set, and
//! instance records, with the stack of current scopes.

use crate::component::FunctionType;
use crate::error::{Error, Result, WaitableCause};
use crate::executor::ir::CanonOptions;
use crate::resource::TableId;

use super::event::Event;
use super::instance_id::InstanceId;
use super::instance_record::InstanceRecord;
use super::readiness::Readiness;
use super::record_table::RecordTable;
use super::scope::Scope;
use super::subtask::Subtask;
use super::subtask_id::SubtaskId;
use super::subtask_state::SubtaskState;
use super::task::Task;
use super::task_id::TaskId;
use super::task_state::TaskState;
use super::thread::Thread;
use super::thread_id::ThreadId;
use super::waitable_id::WaitableId;
use super::waitable_set::WaitableSet;
use super::waitable_set_id::WaitableSetId;
use super::waitable_state::WaitableState;

/// The store's tables of task, subtask, thread, waitable set, and
/// instance records, with the stack of current scopes.
///
/// A scope is a task record or a subtask record, and the top of the
/// stack is the current scope. Every borrow operation consults it: a
/// borrow lowered into a guest counts against the current task, a
/// borrow lifted out of an owning handle is lent to the current
/// scope, and the scope's exit checks that the guest dropped what it
/// was lent.
///
/// The waitable state a guest waits on lives on the records
/// themselves — a subtask carries its own — so the waitable
/// operations here take a [`WaitableId`] and reach through it.
pub struct TaskTables {
    tasks: RecordTable<Task>,
    subtasks: RecordTable<Subtask>,
    threads: RecordTable<Thread>,
    waitable_sets: RecordTable<WaitableSet>,
    instances: Vec<InstanceRecord>,
    scopes: Vec<Scope>,
}

impl TaskTables {
    /// Construct empty tables with no scope in flight.
    pub fn new() -> Self {
        Self {
            tasks: RecordTable::new(),
            subtasks: RecordTable::new(),
            threads: RecordTable::new(),
            waitable_sets: RecordTable::new(),
            instances: Vec::new(),
            scopes: Vec::new(),
        }
    }

    /// Add the record of a fresh component instance and return its
    /// identity. One instantiation calls this once per component
    /// instance it creates.
    pub fn insert_instance(&mut self) -> InstanceId {
        self.instances.push(InstanceRecord::new());
        InstanceId::from_index((self.instances.len() - 1) as u32)
    }

    /// The record of one component instance.
    pub fn instance(&self, instance: InstanceId) -> Option<&InstanceRecord> {
        self.instances.get(instance.index() as usize)
    }

    /// The record of one component instance, mutably.
    pub fn instance_mut(&mut self, instance: InstanceId) -> Option<&mut InstanceRecord> {
        self.instances.get_mut(instance.index() as usize)
    }

    /// Every component instance record in the store, in the order
    /// the instances were created.
    pub fn instances(&self) -> &[InstanceRecord] {
        &self.instances
    }

    /// Set the may-not-suspend flag of `instance` and return the
    /// value it had. `None` when the store holds no such instance.
    pub fn set_may_not_suspend(&mut self, instance: InstanceId, value: bool) -> Option<bool> {
        let record = self.instance_mut(instance)?;
        let old = record.may_not_suspend;
        record.may_not_suspend = value;
        Some(old)
    }

    /// Create a task for a call into an export of `instance`,
    /// without making it the current scope. The task's implicit
    /// thread is created with it.
    ///
    /// A driver creates the task of the call it is about to make
    /// before it queues the task's start, so that the record exists
    /// whether or not a turn ever runs the start. The scope is
    /// pushed by [`push_task_scope`](Self::push_task_scope) when the
    /// thread actually runs, because the scope stack nests with the
    /// one real stack and a queued task is not on it.
    pub fn create_task(
        &mut self,
        function: Option<FunctionType>,
        options: Option<CanonOptions>,
        instance: InstanceId,
    ) -> TaskId {
        let index = self.tasks.next_index();
        let task = TaskId::new(index, self.tasks.generation(index));
        let thread = ThreadId::from_index(self.threads.insert(Thread::new(task)));
        self.tasks
            .insert(Task::new(function, options, instance, thread));
        task
    }

    /// Make `task` the current scope.
    pub fn push_task_scope(&mut self, task: TaskId) {
        self.scopes.push(Scope::Task(task));
    }

    /// Create a task for a call into an export of `instance` and
    /// push it as the current scope. The task's implicit thread is
    /// created with it.
    ///
    /// This is the entry for a call that is already running on the
    /// one real stack: an adapter's enter intrinsic, which pushes
    /// the callee's task from inside the caller's turn.
    pub fn push_task(
        &mut self,
        function: Option<FunctionType>,
        options: Option<CanonOptions>,
        instance: InstanceId,
    ) -> TaskId {
        let task = self.create_task(function, options, instance);
        self.scopes.push(Scope::Task(task));
        task
    }

    /// Create a subtask record for a call out through an import.
    /// The call that made it owns the record until its resolution is
    /// delivered, which is not always the call that is on the stack:
    /// an asynchronous lower leaves the subtask behind for a guest
    /// to wait on.
    pub fn insert_subtask(&mut self) -> SubtaskId {
        SubtaskId::from_index(self.subtasks.insert(Subtask::new()))
    }

    /// Create a subtask for a call out through an import and push it
    /// as the current scope.
    pub fn push_subtask(&mut self) -> SubtaskId {
        let subtask = self.insert_subtask();
        self.scopes.push(Scope::Subtask(subtask));
        subtask
    }

    /// Move a task to its started state: its thread is running.
    pub fn start_task(&mut self, task: TaskId) {
        if let Some(record) = self.task_mut(task) {
            record.state = TaskState::Started;
        }
    }

    /// Move a subtask to its started state: its parameters were
    /// lifted and the callee is running.
    pub fn start_subtask(&mut self, subtask: SubtaskId) {
        if let Some(record) = self.subtask_mut(subtask) {
            record.state = SubtaskState::Started;
        }
    }

    /// Pop the current scope, if there is one.
    pub fn pop_scope(&mut self) -> Option<Scope> {
        self.scopes.pop()
    }

    /// The current scope: the top of the stack.
    pub fn current_scope(&self) -> Option<Scope> {
        self.scopes.last().copied()
    }

    /// The scope an operation counts against when the crossing that
    /// asks for it names `scope`: the scope it named, or the current
    /// scope when it named none. This is the one rule for the scope
    /// argument a crossing hands down — a lend and a borrow both read
    /// it this way, so neither can quietly prefer the stack to the
    /// scope the crossing was built with.
    pub fn counting_scope(&self, scope: Option<Scope>) -> Option<Scope> {
        scope.or_else(|| self.current_scope())
    }

    /// The task a borrow taken during `scope` is owed to. A task
    /// scope is that task. A subtask scope is the task that made the
    /// call — the innermost task under the subtask on the stack —
    /// because a borrow lowered into a guest while a host call runs
    /// is still owed to the task that made the call, and must be
    /// dropped before that task returns. `None` when the subtask is
    /// not on the stack or nothing on the stack under it is a task,
    /// which leaves the borrow nowhere to be owed.
    pub fn borrow_task(&self, scope: Scope) -> Option<TaskId> {
        let under = match scope {
            Scope::Task(task) => return Some(task),
            Scope::Subtask(subtask) => self
                .scopes
                .iter()
                .rposition(|entry| *entry == Scope::Subtask(subtask))?,
        };
        self.scopes[..under]
            .iter()
            .rev()
            .find_map(|scope| match scope {
                Scope::Task(task) => Some(*task),
                Scope::Subtask(_) => None,
            })
    }

    /// The current task: the task the scope on top of the stack
    /// counts its borrows against, which is the innermost task on the
    /// stack.
    pub fn current_task(&self) -> Option<TaskId> {
        self.borrow_task(self.current_scope()?)
    }

    /// The current subtask, when the current scope is one.
    pub fn current_subtask(&self) -> Option<SubtaskId> {
        match self.current_scope()? {
            Scope::Subtask(subtask) => Some(subtask),
            Scope::Task(_) => None,
        }
    }

    /// The current thread: the thread the current task is running.
    /// A synchronous task runs its implicit thread from the call
    /// that starts it to the return that ends it.
    pub fn current_thread(&self) -> Option<ThreadId> {
        Some(self.task(self.current_task()?)?.implicit_thread)
    }

    /// The whole stack of current scopes, outermost first.
    pub fn scopes(&self) -> &[Scope] {
        &self.scopes
    }

    /// The index `task` names, when a live record of that generation
    /// still sits there. `None` for the identity of a task that has
    /// ended, including one whose index another task has since taken:
    /// that is what keeps a borrow entry left behind by a failed call
    /// from reaching the later task's record.
    fn task_index(&self, task: TaskId) -> Option<u32> {
        (self.tasks.generation(task.index()) == task.generation()).then_some(task.index())
    }

    /// One task record.
    pub fn task(&self, task: TaskId) -> Option<&Task> {
        self.tasks.get(self.task_index(task)?)
    }

    /// One task record, mutably.
    pub fn task_mut(&mut self, task: TaskId) -> Option<&mut Task> {
        let index = self.task_index(task)?;
        self.tasks.get_mut(index)
    }

    /// One subtask record.
    pub fn subtask(&self, subtask: SubtaskId) -> Option<&Subtask> {
        self.subtasks.get(subtask.index())
    }

    /// One subtask record, mutably.
    pub fn subtask_mut(&mut self, subtask: SubtaskId) -> Option<&mut Subtask> {
        self.subtasks.get_mut(subtask.index())
    }

    /// One thread record.
    pub fn thread(&self, thread: ThreadId) -> Option<&Thread> {
        self.threads.get(thread.index())
    }

    /// One thread record, mutably.
    pub fn thread_mut(&mut self, thread: ThreadId) -> Option<&mut Thread> {
        self.threads.get_mut(thread.index())
    }

    /// How many task records the store holds.
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    /// How many subtask records the store holds.
    pub fn subtask_count(&self) -> usize {
        self.subtasks.len()
    }

    /// How many thread records the store holds.
    pub fn thread_count(&self) -> usize {
        self.threads.len()
    }

    /// Record that the owning entry `(table, index)` was lent to
    /// `scope`. Returns `false` when the scope's record is gone.
    pub fn add_lender(&mut self, scope: Scope, lend: (TableId, u32)) -> bool {
        match scope {
            Scope::Task(task) => match self.task_mut(task) {
                Some(record) => {
                    record.lenders.push(lend);
                    true
                }
                None => false,
            },
            Scope::Subtask(subtask) => match self.subtask_mut(subtask) {
                Some(record) => {
                    record.lenders.push(lend);
                    true
                }
                None => false,
            },
        }
    }

    /// Take the lends recorded against `scope`, so the caller can
    /// undo them on the owning entries.
    pub fn take_lenders(&mut self, scope: Scope) -> Vec<(TableId, u32)> {
        match scope {
            Scope::Task(task) => self
                .task_mut(task)
                .map(|record| std::mem::take(&mut record.lenders))
                .unwrap_or_default(),
            Scope::Subtask(subtask) => self
                .subtask_mut(subtask)
                .map(|record| std::mem::take(&mut record.lenders))
                .unwrap_or_default(),
        }
    }

    /// Remove a task record and every thread it contains.
    pub fn remove_task(&mut self, task: TaskId) -> Option<Task> {
        let index = self.task_index(task)?;
        let record = self.tasks.remove(index)?;
        for thread in &record.threads {
            self.threads.remove(thread.index());
        }
        Some(record)
    }

    /// Remove a subtask record. The subtask leaves the set it joined
    /// on its way out, whichever path removed it: a freed index is
    /// handed out again, and a membership left behind would name
    /// whichever record takes the index next.
    pub fn remove_subtask(&mut self, subtask: SubtaskId) -> Option<Subtask> {
        self.leave_waitable_set(WaitableId::Subtask(subtask));
        self.subtasks.remove(subtask.index())
    }

    // ---- waitables and waitable sets ----

    /// Create a waitable set record and return its identity. The
    /// `waitable-set.new` built-in calls this and puts the identity's
    /// index in a handle-table entry for the guest.
    pub fn insert_waitable_set(&mut self) -> WaitableSetId {
        WaitableSetId::from_index(self.waitable_sets.insert(WaitableSet::new()))
    }

    /// One waitable set record.
    pub fn waitable_set(&self, set: WaitableSetId) -> Option<&WaitableSet> {
        self.waitable_sets.get(set.index())
    }

    /// How many waitable set records the store holds.
    pub fn waitable_set_count(&self) -> usize {
        self.waitable_sets.len()
    }

    /// The waitable a subtask is. A subtask is the one kind of
    /// waitable the polyfill builds today; the features that add
    /// streams and futures name their ends the same way.
    pub fn subtask_waitable(&self, subtask: SubtaskId) -> WaitableId {
        WaitableId::Subtask(subtask)
    }

    /// The call `subtask` names returned its result: the subtask
    /// moves to its returned state. Its resolution is delivered
    /// separately, when the caller's thread takes the subtask event
    /// or a synchronous lower returns.
    pub fn subtask_returned(&mut self, subtask: SubtaskId) -> Result<()> {
        self.subtask_mut(subtask)
            .ok_or_else(|| Error::internal("subtask record is not in the store"))?
            .state = SubtaskState::Returned;
        Ok(())
    }

    /// The call `subtask` names was cancelled: the subtask moves to
    /// cancelled-before-started when the callee had not read its
    /// parameters yet, and to cancelled-before-returned when it had.
    /// Nothing cancels a call yet.
    #[allow(dead_code)]
    pub fn subtask_cancelled(&mut self, subtask: SubtaskId) -> Result<()> {
        let record = self
            .subtask_mut(subtask)
            .ok_or_else(|| Error::internal("subtask record is not in the store"))?;
        record.state = match record.state {
            SubtaskState::Starting => SubtaskState::CancelledBeforeStarted,
            _ => SubtaskState::CancelledBeforeReturned,
        };
        Ok(())
    }

    /// Record readiness on `subtask` by filling its pending event
    /// slot with the subtask event: `handle_index` is the subtask's
    /// index in the caller instance's handle table, and the second
    /// payload is the state the subtask is in now.
    pub fn record_subtask_event(&mut self, subtask: SubtaskId, handle_index: u32) -> Result<()> {
        let state = self.subtask_record(subtask)?.state;
        self.set_pending_event(
            WaitableId::Subtask(subtask),
            Event::subtask(handle_index, state),
        )
    }

    /// Record readiness on `waitable` by filling its pending event
    /// slot. A slot that already held an event is overwritten, which
    /// is what a waitable that progressed twice before either event
    /// was delivered needs: the guest sees the later state.
    pub fn set_pending_event(&mut self, waitable: WaitableId, event: Event) -> Result<()> {
        self.waitable_state_mut(waitable)?.pending_event = Some(event);
        Ok(())
    }

    /// Whether `waitable` holds a pending event.
    pub fn has_pending_event(&self, waitable: WaitableId) -> Result<bool> {
        Ok(self.waitable_state(waitable)?.pending_event.is_some())
    }

    /// Whether any waitable of `set` holds a pending event. A wait on
    /// such a set returns at once rather than blocking.
    pub fn set_has_pending_event(&self, set: WaitableSetId) -> Result<bool> {
        Ok(self.next_ready_waitable(set)?.is_some())
    }

    /// The waitable set `waitable` joined, or `None` when it has
    /// joined none.
    pub fn waitable_set_of(&self, waitable: WaitableId) -> Result<Option<WaitableSetId>> {
        Ok(self.waitable_state(waitable)?.set)
    }

    /// Move `waitable` into `set`, or out of every set when `set` is
    /// `None`. Joining removes the waitable from the set it was in
    /// before, and a waitable a thread waits on synchronously cannot
    /// join a set at all.
    pub fn join_waitable_set(
        &mut self,
        waitable: WaitableId,
        set: Option<WaitableSetId>,
    ) -> Result<()> {
        if self.waitable_state(waitable)?.synchronous_waiter {
            return Err(Error::Waitable(WaitableCause::SyncAndAsync));
        }
        // Every lookup that can fail happens before the first write.
        // A target set the store does not hold has to leave the
        // waitable in the set it already named: a waitable taken out
        // of that set by a join that then failed would go on naming
        // a set which no longer lists it.
        if let Some(set) = set {
            self.waitable_set_record(set)?;
        }
        self.leave_waitable_set(waitable);
        if let Some(set) = set {
            self.waitable_set_record_mut(set)?.waitables.push(waitable);
            self.waitable_state_mut(waitable)?.set = Some(set);
        }
        Ok(())
    }

    /// Take `waitable` out of the set it joined, if it joined one.
    /// Unlike a join this makes no checks and cannot fail: a record
    /// on its way out of the store leaves its set whatever state it
    /// is in, and a join has made its own checks by the time it
    /// spells this.
    fn leave_waitable_set(&mut self, waitable: WaitableId) {
        let Ok(state) = self.waitable_state_mut(waitable) else {
            return;
        };
        let Some(previous) = state.set.take() else {
            return;
        };
        if let Some(record) = self.waitable_sets.get_mut(previous.index()) {
            record.waitables.retain(|member| *member != waitable);
        }
    }

    /// Mark `waitable` as one a thread waits on synchronously, on its
    /// own rather than through a set. A waitable already in a set
    /// cannot take such a waiter, and neither can one that already
    /// has one.
    pub fn begin_synchronous_wait(&mut self, waitable: WaitableId) -> Result<()> {
        let state = self.waitable_state_mut(waitable)?;
        if state.set.is_some() || state.synchronous_waiter {
            return Err(Error::Waitable(WaitableCause::SyncAndAsync));
        }
        state.synchronous_waiter = true;
        Ok(())
    }

    /// The synchronous wait on `waitable` is over: the waitable can
    /// join a set again.
    pub fn end_synchronous_wait(&mut self, waitable: WaitableId) -> Result<()> {
        self.waitable_state_mut(waitable)?.synchronous_waiter = false;
        Ok(())
    }

    /// Park `thread` on `set`: the set's waiter count rises and the
    /// thread's readiness condition names the set. The scheduler
    /// suspends the thread after this, and
    /// [`end_wait`](Self::end_wait) undoes it when the thread runs
    /// again.
    pub fn begin_wait(&mut self, set: WaitableSetId, thread: ThreadId) -> Result<()> {
        // The thread is looked up before the count rises. A count
        // raised by a wait that then failed is never lowered again,
        // and every later drop of the set traps on a waiter that is
        // not there.
        if self.thread(thread).is_none() {
            return Err(Error::internal("waiting thread is not in the store"));
        }
        self.waitable_set_record_mut(set)?.num_waiting += 1;
        if let Some(record) = self.thread_mut(thread) {
            record.readiness = Some(Readiness::WaitableSet { set });
        }
        Ok(())
    }

    /// The wait [`begin_wait`](Self::begin_wait) parked `thread` for
    /// is over: the set's waiter count falls and the thread's
    /// readiness condition clears.
    pub fn end_wait(&mut self, set: WaitableSetId, thread: ThreadId) -> Result<()> {
        let record = self.waitable_set_record_mut(set)?;
        record.num_waiting = record.num_waiting.saturating_sub(1);
        if let Some(record) = self.thread_mut(thread) {
            record.readiness = None;
        }
        Ok(())
    }

    /// Drop the waitable set `set`. A set that still holds waitables
    /// traps, and so does one a thread is waiting on: the guest would
    /// otherwise strand a waitable pointing at a set that is gone, or
    /// a thread waiting for an event that can never arrive.
    pub fn drop_waitable_set(&mut self, set: WaitableSetId) -> Result<()> {
        let record = self.waitable_set_record_mut(set)?;
        if !record.waitables.is_empty() {
            return Err(Error::Waitable(WaitableCause::SetHasWaitables));
        }
        if record.num_waiting > 0 {
            return Err(Error::Waitable(WaitableCause::SetHasWaiters));
        }
        self.waitable_sets.remove(set.index());
        Ok(())
    }

    /// Drop the waitable `waitable` and the record it names. A
    /// subtask whose resolution was not delivered traps, because the
    /// handles the call borrowed are still lent out; so does a
    /// waitable a thread waits on synchronously. The waitable leaves
    /// the set it joined on its way out.
    pub fn drop_waitable(&mut self, waitable: WaitableId) -> Result<()> {
        let state = self.waitable_state(waitable)?;
        if state.synchronous_waiter {
            return Err(Error::Waitable(WaitableCause::SyncAndAsync));
        }
        match waitable {
            WaitableId::Subtask(subtask) => {
                let record = self.subtask_record(subtask)?;
                if !record.resolve_delivered {
                    return Err(Error::Waitable(WaitableCause::SubtaskNotResolved));
                }
                self.remove_subtask(subtask);
                Ok(())
            }
            // The feature that adds streams and futures removes
            // their end records here.
            _ => Err(Error::internal("waitable kind has no record in the store")),
        }
    }

    /// The waitable of `set` that holds the oldest pending event, in
    /// the order the waitables joined the set. Workspace-internal:
    /// the store's delivery operations spell it.
    pub fn next_ready_waitable(&self, set: WaitableSetId) -> Result<Option<WaitableId>> {
        let record = self.waitable_set_record(set)?;
        for waitable in &record.waitables {
            if self.has_pending_event(*waitable)? {
                return Ok(Some(*waitable));
            }
        }
        Ok(None)
    }

    /// Take the event pending on `waitable`, leaving its slot empty.
    ///
    /// Taking the event of a subtask that has resolved is half of an
    /// operation: the same delivery gives the caller back the
    /// handles the call borrowed, and only the handle tables hold
    /// those. Emptying the slot on its own would lose the one notice
    /// the caller gets, leave the handles lent for good, and trap
    /// every later drop of the subtask. So a resolution that has not
    /// been delivered refuses the take, and the paired operation on
    /// the handle tables — which delivers the resolution first — is
    /// the only way to take such an event.
    pub fn take_pending_event(&mut self, waitable: WaitableId) -> Result<Option<Event>> {
        if let WaitableId::Subtask(subtask) = waitable {
            let record = self.subtask_record(subtask)?;
            if record.state.resolved() && !record.resolve_delivered {
                return Err(Error::internal(
                    "a subtask's event was taken before its resolution was delivered",
                ));
            }
        }
        Ok(self.waitable_state_mut(waitable)?.pending_event.take())
    }

    /// One subtask record, or the internal error when the identity
    /// names none.
    fn subtask_record(&self, subtask: SubtaskId) -> Result<&Subtask> {
        self.subtask(subtask)
            .ok_or_else(|| Error::internal("subtask record is not in the store"))
    }

    /// One waitable set record, or the internal error when the
    /// identity names none.
    fn waitable_set_record(&self, set: WaitableSetId) -> Result<&WaitableSet> {
        self.waitable_set(set)
            .ok_or_else(|| Error::internal("waitable set record is not in the store"))
    }

    /// One waitable set record, mutably.
    fn waitable_set_record_mut(&mut self, set: WaitableSetId) -> Result<&mut WaitableSet> {
        self.waitable_sets
            .get_mut(set.index())
            .ok_or_else(|| Error::internal("waitable set record is not in the store"))
    }

    /// The waitable state on the record `waitable` names.
    fn waitable_state(&self, waitable: WaitableId) -> Result<&WaitableState> {
        match waitable {
            WaitableId::Subtask(subtask) => Ok(&self.subtask_record(subtask)?.waitable),
            // The feature that adds streams and futures answers with
            // the state on their end records.
            _ => Err(Error::internal("waitable kind has no record in the store")),
        }
    }

    /// The waitable state on the record `waitable` names, mutably.
    fn waitable_state_mut(&mut self, waitable: WaitableId) -> Result<&mut WaitableState> {
        match waitable {
            WaitableId::Subtask(subtask) => match self.subtasks.get_mut(subtask.index()) {
                Some(record) => Ok(&mut record.waitable),
                None => Err(Error::internal("subtask record is not in the store")),
            },
            // The feature that adds streams and futures answers with
            // the state on their end records.
            _ => Err(Error::internal("waitable kind has no record in the store")),
        }
    }
}

impl Default for TaskTables {
    fn default() -> Self {
        Self::new()
    }
}
