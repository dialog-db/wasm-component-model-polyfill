//! The store's tables of task, subtask, thread, waitable set, end,
//! shared, and instance records, with the stack of current scopes.

use std::sync::Arc;

use crate::abi::signature::Signature;
use crate::error::{CopyCause, Error, Result, WaitableCause};
use crate::executor::ir::CanonOptions;
use crate::internal::ErrorInternal;
use crate::resource::TableId;
use crate::types::ValueType;

use super::copy_end::CopyEnd;
use super::copy_state::CopyState;
use super::end_direction::EndDirection;
use super::end_id::EndId;
use super::end_kind::EndKind;
use super::event::Event;
use super::failure_channel::FailureChannel;
use super::instance_id::InstanceId;
use super::instance_record::InstanceRecord;
use super::readiness::Readiness;
use super::record_table::RecordTable;
use super::scope::Scope;
use super::shared_record::SharedRecord;
use super::subtask::Subtask;
use super::subtask_id::SubtaskId;
use super::subtask_state::SubtaskState;
use super::task::Task;
use super::task_id::TaskId;
use super::task_result::{ResultChannel, TaskResult};
use super::task_state::TaskState;
use super::thread::Thread;
use super::thread_id::ThreadId;
use super::waitable_id::WaitableId;
use super::waitable_set::WaitableSet;
use super::waitable_set_id::WaitableSetId;
use super::waitable_state::WaitableState;

/// The store's tables of task, subtask, thread, waitable set, end,
/// shared, and instance records, with the stack of current scopes.
///
/// A scope is a task record or a subtask record, and the top of the
/// stack is the current scope. Every borrow operation consults it: a
/// borrow lowered into a guest counts against the current task, a
/// borrow lifted out of an owning handle is lent to the current
/// scope, and the scope's exit checks that the guest dropped what it
/// was lent.
///
/// The waitable state a guest waits on lives on the records
/// themselves — a subtask and a stream or future end each carry
/// their own — so the waitable operations here take a [`WaitableId`]
/// and reach through it.
///
/// The tables also keep the list of waitable sets that took on an
/// event since the scheduler last looked: a waitable of the set was
/// given an event, or a waitable holding one joined it. That list is
/// how a callback item held until its set holds an event is found
/// without examining every held item on every turn.
pub struct TaskTables {
    tasks: RecordTable<Task>,
    subtasks: RecordTable<Subtask>,
    threads: RecordTable<Thread>,
    waitable_sets: RecordTable<WaitableSet>,
    ends: RecordTable<CopyEnd>,
    shared_records: RecordTable<SharedRecord>,
    instances: Vec<InstanceRecord>,
    scopes: Vec<Scope>,
    prepared_call: Option<SubtaskId>,
    signalled_sets: Vec<WaitableSetId>,
}

impl TaskTables {
    /// Construct empty tables with no scope in flight.
    pub fn new() -> Self {
        Self {
            tasks: RecordTable::new(),
            subtasks: RecordTable::new(),
            threads: RecordTable::new(),
            waitable_sets: RecordTable::new(),
            ends: RecordTable::new(),
            shared_records: RecordTable::new(),
            instances: Vec::new(),
            scopes: Vec::new(),
            prepared_call: None,
            signalled_sets: Vec::new(),
        }
    }

    /// Hold `subtask` as the call the prepare intrinsic just set up,
    /// for the start intrinsic that follows it.
    ///
    /// One slot suffices. A fused adapter calls the two intrinsics
    /// back to back, with nothing between them but the constants of
    /// the start call, so no second prepare can reach this before
    /// the start of the first has taken it out. A call the callee
    /// makes in its turn prepares from inside the start, by which
    /// time the slot is empty again.
    pub fn prepare_call(&mut self, subtask: SubtaskId) {
        self.prepared_call = Some(subtask);
    }

    /// Take the call the prepare intrinsic set up, which is what a
    /// start intrinsic runs. `None` when no prepare preceded it.
    pub fn take_prepared_call(&mut self) -> Option<SubtaskId> {
        self.prepared_call.take()
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

    /// Drop every instance record from `kept` on, which is how an
    /// instantiation that failed leaves the store's records as it
    /// found them.
    ///
    /// Only a tail can leave the list, because an identity is an
    /// index into it and the records that follow one would move. A
    /// failed instantiation has a tail to take back: it reserves
    /// every record it needs before any fallible step of its plan
    /// runs, and nothing else in the runtime adds one, so the
    /// records it reserved are the last ones in the list. `kept` is
    /// the length the list had before it reserved them, and a
    /// longer list is left alone.
    pub fn truncate_instances(&mut self, kept: usize) {
        self.instances.truncate(kept);
    }

    /// Set the may-not-suspend flag of `instance` and return the
    /// value it had. `None` when the store holds no such instance.
    pub fn set_may_not_suspend(&mut self, instance: InstanceId, value: bool) -> Option<bool> {
        let record = self.instance_mut(instance)?;
        let old = record.may_not_suspend;
        record.may_not_suspend = value;
        Some(old)
    }

    /// Mark the instance of `task` as one whose threads may not
    /// suspend, for the length of the call the task runs, and save
    /// the flag's old value on the task's implicit thread. The
    /// task's own exit puts the saved value back, which is where
    /// Wasmtime saves and restores it too.
    ///
    /// Four callers hold the flag this way: an adapter's enter
    /// intrinsic for a synchronous call between two components, a
    /// host call into a synchronous export, the task a core
    /// module's start function runs in, and the task a resource
    /// destructor runs as. Each is a call that must
    /// return before its instance may block. Answers `None` when the
    /// store holds no such task or no such instance, or when the task
    /// belongs to no instance, and holds nothing in that case.
    pub fn hold_may_not_suspend(&mut self, task: TaskId) -> Option<()> {
        let (instance, thread) = self
            .task(task)
            .map(|record| (record.instance, record.implicit_thread))?;
        let instance = instance?;
        let old = self.set_may_not_suspend(instance, true)?;
        self.thread_mut(thread)?.old_may_not_suspend = Some(old);
        Some(())
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
        function: Option<Arc<Signature>>,
        options: Option<Arc<CanonOptions>>,
        instance: InstanceId,
    ) -> TaskId {
        self.create(function, options, Some(instance))
    }

    /// Create a task, whether or not it belongs to a component
    /// instance, with its implicit thread.
    fn create(
        &mut self,
        function: Option<Arc<Signature>>,
        options: Option<Arc<CanonOptions>>,
        instance: Option<InstanceId>,
    ) -> TaskId {
        let index = self.tasks.next_index();
        let task = TaskId::new(index, self.tasks.generation(index));
        let (thread_index, thread_generation) =
            self.threads.insert_with_generation(Thread::new(task));
        let thread = ThreadId::new(thread_index, thread_generation);
        let inserted = self
            .tasks
            .insert(Task::new(function, options, instance, thread));
        debug_assert_eq!(
            inserted, index,
            "the task took the index its identity was minted against"
        );
        task
    }

    /// Give `task` a channel to resolve through and hand the caller
    /// its half.
    ///
    /// A task whose caller is on the stack leaves its result in the
    /// record. A task whose caller is not — a host call into an
    /// asynchronous export, whose task outlives the call — resolves
    /// through this channel instead, and the call's driver watches it.
    /// `None` when the store holds no such task.
    pub fn attach_result_channel(&mut self, task: TaskId) -> Option<ResultChannel> {
        let channel: ResultChannel = ResultChannel::default();
        self.task_mut(task)?.result = TaskResult::Channel(channel.clone());
        Some(channel)
    }

    /// Give `task` a channel to fail through and hand the caller its
    /// half.
    ///
    /// The channel is where a failure of the task reaches the call
    /// that started it, whether the failure came from the item that
    /// ran the task or from work the store ran for it in a later
    /// turn. `None` when the store holds no such task.
    pub fn attach_failure_channel(&mut self, task: TaskId) -> Option<FailureChannel> {
        let channel: FailureChannel = FailureChannel::default();
        self.task_mut(task)?.failure = Some(channel.clone());
        Some(channel)
    }

    /// The half of `task`'s failure channel the store fills, when
    /// the call that started the task left one.
    pub fn failure_channel(&self, task: TaskId) -> Option<FailureChannel> {
        self.task(task)?.failure.clone()
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
        function: Option<Arc<Signature>>,
        options: Option<Arc<CanonOptions>>,
        instance: InstanceId,
    ) -> TaskId {
        let task = self.create_task(function, options, instance);
        self.scopes.push(Scope::Task(task));
        task
    }

    /// Create a task that belongs to no component instance and push
    /// it as the current scope, with its implicit thread.
    ///
    /// The one such task is the destructor of a resource the host
    /// implements: the host releases the handle with no guest on the
    /// stack, and the destructor is a closure of its own rather than
    /// a core function of some instance.
    pub fn push_task_without_instance(&mut self) -> TaskId {
        let task = self.create(None, None, None);
        self.scopes.push(Scope::Task(task));
        task
    }

    /// Create a subtask record for a call out through an import.
    /// The call that made it owns the record until its resolution is
    /// delivered, which is not always the call that is on the stack:
    /// an asynchronous lower leaves the subtask behind for a guest
    /// to wait on.
    pub fn insert_subtask(&mut self) -> SubtaskId {
        let (index, generation) = self.subtasks.insert_with_generation(Subtask::new());
        SubtaskId::new(index, generation)
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
    ///
    /// A subtask the caller already holds a handle for takes on the
    /// subtask event as it starts, because the caller has an entry
    /// to be told about. That is the callee the entry gate held: the
    /// lower returned `STARTING` with the index, and the callee's
    /// parameters were lifted in a later turn. A subtask started
    /// while the lower is still on the stack has no entry yet, and
    /// the status word the lower returns is what tells the caller.
    pub fn start_subtask(&mut self, subtask: SubtaskId) {
        if let Some(record) = self.subtask_mut(subtask) {
            record.state = SubtaskState::Started;
        }
        if let Some(index) = self.subtask_handle(subtask) {
            let _ = self.record_subtask_event(subtask, index);
        }
    }

    /// Record that `subtask` sits at `index` in the caller
    /// instance's handle table. The handle tables make the entry and
    /// tell the record here, so that a later event knows the index
    /// it carries and the removal of the entry knows what to take
    /// with it.
    pub fn set_subtask_handle(&mut self, subtask: SubtaskId, index: u32) {
        if let Some(record) = self.subtask_mut(subtask) {
            record.handle = Some(index);
        }
    }

    /// Where `subtask` sits in the caller instance's handle table,
    /// or `None` when the caller holds no entry for it.
    pub fn subtask_handle(&self, subtask: SubtaskId) -> Option<u32> {
        self.subtask(subtask).and_then(|record| record.handle)
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

    /// The scope a handle lent for the call in flight counts
    /// against, read off the stack alone: the subtask of the call
    /// the current task is the callee of, when a prepare intrinsic
    /// set one up, and the current scope otherwise.
    ///
    /// A lend lives on the record of the call it was made for, so
    /// that it comes back when that call's caller takes delivery of
    /// the result. A fused adapter's borrow transfer runs while the
    /// callee's task is on top of the stack, and the record of the
    /// call the caller lent for is the subtask that task names, so
    /// reading the stack alone would credit the callee's task with a
    /// lend the caller made.
    pub fn lending_scope(&self) -> Option<Scope> {
        match self.current_scope()? {
            Scope::Task(task) => match self.task(task).and_then(|record| record.subtask) {
                Some(subtask) => Some(Scope::Subtask(subtask)),
                None => Some(Scope::Task(task)),
            },
            scope => Some(scope),
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

    /// The index `subtask` names, under the rule
    /// [`task_index`](Self::task_index) states: `None` once the call
    /// the identity was minted for has ended, so a lend recorded
    /// against a subtask scope that is over cannot reach whichever
    /// call took the index.
    fn subtask_index(&self, subtask: SubtaskId) -> Option<u32> {
        (self.subtasks.generation(subtask.index()) == subtask.generation())
            .then_some(subtask.index())
    }

    /// The index `thread` names, under the rule
    /// [`task_index`](Self::task_index) states: `None` once the
    /// thread the identity was minted for has ended, so a context
    /// slot addressed through a stale identity cannot land on
    /// whichever thread took the index.
    fn thread_index(&self, thread: ThreadId) -> Option<u32> {
        (self.threads.generation(thread.index()) == thread.generation()).then_some(thread.index())
    }

    /// The index `set` names, under the rule
    /// [`task_index`](Self::task_index) states: `None` once the set
    /// the identity was minted for is gone, so a waitable or a
    /// parked thread left naming a dropped set cannot reach whichever
    /// set took the index.
    fn waitable_set_index(&self, set: WaitableSetId) -> Option<u32> {
        (self.waitable_sets.generation(set.index()) == set.generation()).then_some(set.index())
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
        self.subtasks.get(self.subtask_index(subtask)?)
    }

    /// One subtask record, mutably.
    pub fn subtask_mut(&mut self, subtask: SubtaskId) -> Option<&mut Subtask> {
        let index = self.subtask_index(subtask)?;
        self.subtasks.get_mut(index)
    }

    /// One thread record.
    pub fn thread(&self, thread: ThreadId) -> Option<&Thread> {
        self.threads.get(self.thread_index(thread)?)
    }

    /// One thread record, mutably.
    pub fn thread_mut(&mut self, thread: ThreadId) -> Option<&mut Thread> {
        let index = self.thread_index(thread)?;
        self.threads.get_mut(index)
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
    ///
    /// A guest callee whose caller still holds a subtask entry keeps
    /// its record: the entry names it, and `subtask.drop` is what
    /// takes it away. Such a task loses its threads here, because
    /// its implicit thread has exited, and the flag that says so is
    /// what [`remove_subtask`](Self::remove_subtask) reads when the
    /// entry goes. The record is left in the store and `None` comes
    /// back, which is what a caller that removed nothing sees.
    pub fn remove_task(&mut self, task: TaskId) -> Option<Task> {
        let index = self.task_index(task)?;
        if self.entry_holds_task(task) {
            self.end_threads_of(task);
            if let Some(record) = self.tasks.get_mut(index) {
                record.thread_exited = true;
            }
            return None;
        }
        self.end_threads_of(task);
        self.tasks.remove(index)
    }

    /// Remove a subtask record. The subtask leaves the set it joined
    /// on its way out, whichever path removed it: a freed index is
    /// handed out again, and a membership left behind would name
    /// whichever record takes the index next.
    ///
    /// A guest callee's task record leaves with it once the callee's
    /// implicit thread has exited, which is the other half of the
    /// rule [`remove_task`](Self::remove_task) states. A host call
    /// has no callee task, so its record is the subtask's alone.
    pub fn remove_subtask(&mut self, subtask: SubtaskId) -> Option<Subtask> {
        let index = self.subtask_index(subtask)?;
        self.leave_waitable_set(WaitableId::Subtask(subtask));
        let record = self.subtasks.remove(index)?;
        if let Some(callee) = record.callee
            && self.task(callee).is_some_and(|callee| callee.thread_exited)
            && let Some(index) = self.task_index(callee)
        {
            self.tasks.remove(index);
        }
        Some(record)
    }

    /// Whether a subtask entry the caller still holds names `task`
    /// as its callee.
    fn entry_holds_task(&self, task: TaskId) -> bool {
        self.task(task)
            .and_then(|record| record.subtask)
            .and_then(|subtask| self.subtask(subtask))
            .is_some_and(|record| record.handle.is_some())
    }

    /// Remove every thread record `task` contains, and empty its
    /// list of them.
    fn end_threads_of(&mut self, task: TaskId) {
        let Some(threads) = self
            .task_mut(task)
            .map(|record| std::mem::take(&mut record.threads))
        else {
            return;
        };
        for thread in threads {
            if let Some(index) = self.thread_index(thread) {
                self.threads.remove(index);
            }
        }
    }

    // ---- waitables and waitable sets ----

    /// Create a waitable set record and return its identity. The
    /// `waitable-set.new` built-in calls this and puts the identity's
    /// index in a handle-table entry for the guest.
    pub fn insert_waitable_set(&mut self) -> WaitableSetId {
        let (index, generation) = self
            .waitable_sets
            .insert_with_generation(WaitableSet::new());
        WaitableSetId::new(index, generation)
    }

    /// One waitable set record.
    pub fn waitable_set(&self, set: WaitableSetId) -> Option<&WaitableSet> {
        self.waitable_sets.get(self.waitable_set_index(set)?)
    }

    /// One waitable set record, mutably.
    fn waitable_set_mut(&mut self, set: WaitableSetId) -> Option<&mut WaitableSet> {
        let index = self.waitable_set_index(set)?;
        self.waitable_sets.get_mut(index)
    }

    /// How many waitable set records the store holds.
    pub fn waitable_set_count(&self) -> usize {
        self.waitable_sets.len()
    }

    /// How many waitable sets are on the list of sets signalled since
    /// the scheduler last looked.
    pub fn signalled_set_count(&self) -> usize {
        self.signalled_sets.len()
    }

    /// The waitable a subtask is. A stream or future end is named
    /// the same way, through [`WaitableId::from_end`].
    pub fn subtask_waitable(&self, subtask: SubtaskId) -> WaitableId {
        WaitableId::Subtask(subtask)
    }

    /// The call `subtask` names returned its result: the subtask
    /// moves to its returned state. Its resolution is delivered
    /// separately, when the caller's thread takes the subtask event
    /// or a synchronous lower returns.
    ///
    /// A subtask the caller holds a handle for takes on the subtask
    /// event as it resolves, which is the reference's `on_resolve`
    /// reaching `on_progress`. That is what a caller which already
    /// took the `STARTED` event and went back to waiting is waiting
    /// for: nothing else would fill the slot again, and delivery
    /// rebuilding the state saves only a caller whose `STARTED` was
    /// never taken.
    pub fn subtask_returned(&mut self, subtask: SubtaskId) -> Result<()> {
        self.subtask_mut(subtask)
            .ok_or_else(|| Error::internal("subtask record is not in the store"))?
            .state = SubtaskState::Returned;
        if let Some(index) = self.subtask_handle(subtask) {
            self.record_subtask_event(subtask, index)?;
        }
        Ok(())
    }

    /// The call `subtask` names was cancelled: the subtask moves to
    /// cancelled-before-started when the callee had not read its
    /// parameters yet, and to cancelled-before-returned when it had.
    /// A host task whose body failed after the call returned to the
    /// guest reaches this; the cancellation built-ins that also will
    /// are not built yet.
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
    ///
    /// The set the waitable joined, if it joined one, goes on the
    /// list of signalled sets.
    pub fn set_pending_event(&mut self, waitable: WaitableId, event: Event) -> Result<()> {
        let state = self.waitable_state_mut(waitable)?;
        state.pending_event = Some(event);
        if let Some(set) = state.set {
            self.signal_waitable_set(set);
        }
        Ok(())
    }

    /// Put `set` on the list of sets that took on an event since the
    /// scheduler last looked, unless it is on the list already.
    fn signal_waitable_set(&mut self, set: WaitableSetId) {
        let Some(record) = self.waitable_set_mut(set) else {
            return;
        };
        if !record.signalled {
            record.signalled = true;
            self.signalled_sets.push(set);
        }
    }

    /// Take the list of sets that took on an event since the last
    /// take, in the order they were first signalled. A set on the
    /// list may hold no event by now — a poll can have taken it — so
    /// the list says where to look and not what will be found.
    pub fn take_signalled_sets(&mut self) -> Vec<WaitableSetId> {
        let signalled = core::mem::take(&mut self.signalled_sets);
        for set in &signalled {
            if let Some(record) = self.waitable_set_mut(*set) {
                record.signalled = false;
            }
        }
        signalled
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
            let state = self.waitable_state_mut(waitable)?;
            state.set = Some(set);
            // A waitable that brings an event with it fills the set
            // as surely as an event given to a member does.
            if state.pending_event.is_some() {
                self.signal_waitable_set(set);
            }
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
        if let Some(record) = self.waitable_set_mut(previous) {
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
        // A dropped set leaves the list of signalled sets with it, so
        // the list names only sets that are still there.
        if record.signalled {
            self.signalled_sets.retain(|signalled| *signalled != set);
        }
        if let Some(index) = self.waitable_set_index(set) {
            self.waitable_sets.remove(index);
        }
        Ok(())
    }

    /// Drop the waitable `waitable` and the record it names. A
    /// subtask whose resolution was not delivered traps, because the
    /// handles the call borrowed are still lent out; so does a
    /// waitable a thread waits on synchronously. A stream or future
    /// end follows the rules [`drop_end`](Self::drop_end) states. The
    /// waitable leaves the set it joined on its way out.
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
            _ => match waitable.end() {
                Some((kind, end)) => self.drop_end(kind, end),
                None => Err(Error::internal("waitable kind has no record in the store")),
            },
        }
    }

    // ---- stream and future ends ----

    /// Create one stream or future: its shared record, carrying
    /// `payload`, and its two end records, both idle. Answers the
    /// readable end and then the writable end.
    ///
    /// The `stream.new` and `future.new` built-ins call this and put
    /// the two identities in handle-table entries for the guest.
    pub fn insert_ends(&mut self, payload: Option<ValueType>) -> (EndId, EndId) {
        // Each end names the shared record by its index, and the
        // shared record names both ends, so the shared record is
        // built against the index it is about to take.
        let shared = self.shared_records.next_index();
        let (index, generation) = self
            .ends
            .insert_with_generation(CopyEnd::new(EndDirection::Readable, shared));
        let readable = EndId::new(index, generation);
        let (index, generation) = self
            .ends
            .insert_with_generation(CopyEnd::new(EndDirection::Writable, shared));
        let writable = EndId::new(index, generation);
        let inserted = self
            .shared_records
            .insert(SharedRecord::new(payload, readable, writable));
        debug_assert_eq!(
            inserted, shared,
            "the shared record took the index its ends were built against"
        );
        (readable, writable)
    }

    /// The index `end` names, under the rule
    /// [`task_index`](Self::task_index) states: `None` once the end
    /// the identity was minted for is gone, so an entry left naming
    /// a removed end cannot reach whichever end took the index.
    fn end_index(&self, end: EndId) -> Option<u32> {
        (self.ends.generation(end.index()) == end.generation()).then_some(end.index())
    }

    /// One end record.
    pub fn end(&self, end: EndId) -> Option<&CopyEnd> {
        self.ends.get(self.end_index(end)?)
    }

    /// One end record, mutably.
    pub fn end_mut(&mut self, end: EndId) -> Option<&mut CopyEnd> {
        let index = self.end_index(end)?;
        self.ends.get_mut(index)
    }

    /// The shared record of `end`: the state its stream or future
    /// shares with the other end.
    pub fn shared_record(&self, end: EndId) -> Option<&SharedRecord> {
        self.shared_records.get(self.end(end)?.shared)
    }

    /// How many end records the store holds.
    pub fn end_count(&self) -> usize {
        self.ends.len()
    }

    /// How many shared records the store holds: one per stream or
    /// future that has an end left.
    pub fn shared_record_count(&self) -> usize {
        self.shared_records.len()
    }

    /// Drop `end`, an end of kind `kind`, once its handle-table entry
    /// is gone. The reference's `drop` makes the same checks, and the
    /// traps carry Wasmtime's messages:
    ///
    /// - An end that is copying or cancelling a copy traps with the
    ///   busy cause of its kind.
    /// - A writable future end that has not written its value traps,
    ///   so that a reader always gets one. A writable future end whose
    ///   reader dropped is done and drops cleanly.
    ///
    /// The end leaves the set it joined, as the reference's drop of a
    /// waitable does. Dropping the first end of a pair marks the
    /// shared record dropped and leaves both end records in the
    /// store; dropping the second removes the shared record and both
    /// end records.
    pub fn drop_end(&mut self, kind: EndKind, end: EndId) -> Result<()> {
        let record = self.end_record(end)?;
        if record.state.busy() {
            return Err(Error::Copy(CopyCause::BusyEnd { kind }));
        }
        if kind == EndKind::FutureWritable && record.state != CopyState::Done {
            return Err(Error::Copy(CopyCause::FutureWriteEndNotWritten));
        }
        let shared = record.shared;
        self.leave_waitable_set(WaitableId::from_end(kind, end));
        let record = self
            .shared_records
            .get_mut(shared)
            .ok_or_else(|| Error::internal("shared record is not in the store"))?;
        if !record.dropped {
            record.dropped = true;
            return Ok(());
        }
        let record = self
            .shared_records
            .remove(shared)
            .ok_or_else(|| Error::internal("shared record is not in the store"))?;
        for end in [record.readable, record.writable] {
            if let Some(index) = self.end_index(end) {
                self.ends.remove(index);
            }
        }
        Ok(())
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
    ///
    /// The state a subtask event carries is read here rather than
    /// when the slot was filled, because delivery is what the guest
    /// observes: a subtask that started and then returned before
    /// anything took its event delivers `RETURNED` alone, and the
    /// `STARTED` it passed through is never seen.
    pub fn take_pending_event(&mut self, waitable: WaitableId) -> Result<Option<Event>> {
        if let WaitableId::Subtask(subtask) = waitable {
            let record = self.subtask_record(subtask)?;
            if record.state.resolved() && !record.resolve_delivered {
                return Err(Error::internal(
                    "a subtask's event was taken before its resolution was delivered",
                ));
            }
            let state = record.state;
            let taken = self.waitable_state_mut(waitable)?.pending_event.take();
            return Ok(taken.map(|event| Event::subtask(event.payloads()[0], state)));
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
        self.waitable_set_mut(set)
            .ok_or_else(|| Error::internal("waitable set record is not in the store"))
    }

    /// One end record, or the internal error when the identity names
    /// none.
    fn end_record(&self, end: EndId) -> Result<&CopyEnd> {
        self.end(end)
            .ok_or_else(|| Error::internal("end record is not in the store"))
    }

    /// The waitable state on the record `waitable` names.
    fn waitable_state(&self, waitable: WaitableId) -> Result<&WaitableState> {
        match waitable {
            WaitableId::Subtask(subtask) => Ok(&self.subtask_record(subtask)?.waitable),
            WaitableId::StreamReadable(end)
            | WaitableId::StreamWritable(end)
            | WaitableId::FutureReadable(end)
            | WaitableId::FutureWritable(end) => Ok(&self.end_record(end)?.waitable),
        }
    }

    /// The waitable state on the record `waitable` names, mutably.
    fn waitable_state_mut(&mut self, waitable: WaitableId) -> Result<&mut WaitableState> {
        match waitable {
            WaitableId::Subtask(subtask) => match self.subtask_mut(subtask) {
                Some(record) => Ok(&mut record.waitable),
                None => Err(Error::internal("subtask record is not in the store")),
            },
            WaitableId::StreamReadable(end)
            | WaitableId::StreamWritable(end)
            | WaitableId::FutureReadable(end)
            | WaitableId::FutureWritable(end) => match self.end_mut(end) {
                Some(record) => Ok(&mut record.waitable),
                None => Err(Error::internal("end record is not in the store")),
            },
        }
    }
}

impl Default for TaskTables {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    fn it_resolves_a_stale_subtask_identity_to_no_record() {
        // A call out through an import ends and its record index is
        // handed out again. The identity the ended call was made
        // with must not reach the call that took the index, because
        // a subtask is the scope a lend is recorded against.
        let mut tables = TaskTables::new();
        let ended = tables.insert_subtask();
        assert!(tables.remove_subtask(ended).is_some(), "the call ends");

        let later = tables.insert_subtask();
        assert_eq!(
            later.index(),
            ended.index(),
            "the freed index is handed out again"
        );
        assert_ne!(later, ended, "the generation tells the two calls apart");

        assert!(
            tables.subtask(ended).is_none(),
            "the ended call's identity names no record"
        );
        assert!(
            !tables.add_lender(Scope::Subtask(ended), (TableId::fresh(), 0)),
            "so no lend can be recorded against it"
        );
        assert!(
            tables
                .subtask(later)
                .expect("the later call")
                .lenders
                .is_empty(),
            "and the call that took the index was lent nothing"
        );
    }

    #[wcmp_macros::test]
    fn it_resolves_a_stale_thread_identity_to_no_record() {
        // A task's threads go with its record, and the next task
        // takes the freed thread index. The ended thread's identity
        // must not address the later thread's context slots.
        let mut tables = TaskTables::new();
        let instance = tables.insert_instance();
        let ended = tables.create_task(None, None, instance);
        let ended_thread = tables.task(ended).expect("the task").implicit_thread;
        assert!(tables.remove_task(ended).is_some(), "the task ends");

        let later = tables.create_task(None, None, instance);
        let later_thread = tables.task(later).expect("the task").implicit_thread;
        assert_eq!(
            later_thread.index(),
            ended_thread.index(),
            "the freed index is handed out again"
        );
        assert_ne!(
            later_thread, ended_thread,
            "the generation tells the two threads apart"
        );

        assert!(
            tables.thread(ended_thread).is_none(),
            "the ended thread's identity names no record"
        );
        assert!(
            tables.thread_mut(ended_thread).is_none(),
            "so nothing can be written through it"
        );
        assert_eq!(
            tables
                .thread(later_thread)
                .expect("the later thread")
                .context,
            [0, 0],
            "and the thread that took the index kept its own slots"
        );
    }

    #[wcmp_macros::test]
    fn it_strikes_a_dropped_set_from_the_signalled_sets() {
        let mut tables = TaskTables::new();
        let kept = tables.insert_waitable_set();
        let dropped = tables.insert_waitable_set();
        for set in [kept, dropped] {
            let subtask = tables.insert_subtask();
            let waitable = tables.subtask_waitable(subtask);
            tables
                .join_waitable_set(waitable, Some(set))
                .expect("the subtask joins the set");
            tables
                .set_pending_event(waitable, Event::none())
                .expect("the subtask is ready");
            // The waitable leaves so that the set can be dropped; the
            // signal the event raised stays.
            tables
                .join_waitable_set(waitable, None)
                .expect("the subtask leaves the set");
        }
        assert_eq!(tables.signalled_set_count(), 2);

        tables
            .drop_waitable_set(dropped)
            .expect("a set nothing is in drops");

        assert_eq!(
            tables.take_signalled_sets(),
            vec![kept],
            "the dropped set left the list and the other set stayed on it"
        );
    }

    #[wcmp_macros::test]
    fn it_resolves_a_stale_waitable_set_identity_to_no_record() {
        // A dropped set's index is handed out again. The dropped
        // set's identity must not name the set that took it, or a
        // waitable left naming the old set would join the new one.
        let mut tables = TaskTables::new();
        let dropped = tables.insert_waitable_set();
        tables
            .drop_waitable_set(dropped)
            .expect("a set nothing is in drops");

        let later = tables.insert_waitable_set();
        assert_eq!(
            later.index(),
            dropped.index(),
            "the freed index is handed out again"
        );
        assert_ne!(later, dropped, "the generation tells the two sets apart");

        assert!(
            tables.waitable_set(dropped).is_none(),
            "the dropped set's identity names no record"
        );
        let subtask = tables.insert_subtask();
        let waitable = tables.subtask_waitable(subtask);
        assert!(
            tables.join_waitable_set(waitable, Some(dropped)).is_err(),
            "so nothing can join it"
        );
        assert!(
            tables
                .waitable_set(later)
                .expect("the later set")
                .waitables
                .is_empty(),
            "and the set that took the index lists nothing"
        );
        assert_eq!(
            tables.waitable_set_of(waitable).expect("the waitable"),
            None,
            "the refused join left the waitable naming no set"
        );
    }

    /// A prepared call between two components, as the records hold
    /// it: the callee's task, the caller's subtask, and the two
    /// pointing at each other.
    fn prepared_call(tables: &mut TaskTables) -> (TaskId, SubtaskId) {
        let instance = tables.insert_instance();
        let task = tables.create_task(None, None, instance);
        let subtask = tables.insert_subtask();
        if let Some(record) = tables.subtask_mut(subtask) {
            record.callee = Some(task);
        }
        if let Some(record) = tables.task_mut(task) {
            record.subtask = Some(subtask);
        }
        (task, subtask)
    }

    #[wcmp_macros::test]
    fn it_removes_a_guest_callees_task_record_when_no_entry_names_it() {
        // A call the caller was never given an entry for — the
        // synchronous lower, whose subtask resolves before the lower
        // returns. Nothing names the callee's record once its thread
        // has exited, so it leaves at once.
        let mut tables = TaskTables::new();
        let (task, _subtask) = prepared_call(&mut tables);

        assert!(tables.remove_task(task).is_some(), "the task's record left");
        assert!(tables.task(task).is_none());
        assert_eq!(tables.thread_count(), 0, "with its implicit thread");
    }

    #[wcmp_macros::test]
    fn it_keeps_a_guest_callees_task_record_until_its_entry_is_gone() {
        // The caller holds an entry for the call, so the callee's
        // record outlives its thread: the entry names it, and the
        // drop of the entry is what takes it away.
        let mut tables = TaskTables::new();
        let (task, subtask) = prepared_call(&mut tables);
        tables.set_subtask_handle(subtask, 1);

        assert!(
            tables.remove_task(task).is_none(),
            "the thread exited and nothing was removed"
        );
        assert!(
            tables.task(task).is_some(),
            "the record stands while the entry names it"
        );
        assert_eq!(
            tables.thread_count(),
            0,
            "its implicit thread exited all the same"
        );

        tables.remove_subtask(subtask);

        assert!(
            tables.task(task).is_none(),
            "the entry's removal took the callee's record with it"
        );
        assert_eq!(tables.task_count(), 0);
    }

    #[wcmp_macros::test]
    fn it_keeps_a_guest_callees_task_record_whose_thread_is_still_running() {
        // The other order: the entry goes while the callee's thread
        // is still running, which a callback export that returned its
        // result and went on waiting leaves. The record stays until
        // the thread exits, and leaves then.
        let mut tables = TaskTables::new();
        let (task, subtask) = prepared_call(&mut tables);
        tables.set_subtask_handle(subtask, 1);

        tables.remove_subtask(subtask);

        assert!(
            tables.task(task).is_some(),
            "the callee's thread has not exited yet"
        );

        assert!(
            tables.remove_task(task).is_some(),
            "and the record leaves when it does"
        );
    }

    #[wcmp_macros::test]
    fn it_keeps_both_ends_until_the_second_one_drops() {
        let mut tables = TaskTables::new();
        let (readable, writable) = tables.insert_ends(None);
        assert_eq!(tables.end_count(), 2);
        assert_eq!(tables.shared_record_count(), 1);
        assert_eq!(
            tables.end(readable).map(|end| end.direction),
            Some(EndDirection::Readable)
        );
        assert_eq!(
            tables.end(writable).map(|end| end.direction),
            Some(EndDirection::Writable)
        );

        tables
            .drop_end(EndKind::StreamWritable, writable)
            .expect("an idle writable stream end drops");
        assert!(
            tables
                .shared_record(readable)
                .is_some_and(|record| record.dropped),
            "the first drop marks the shared record"
        );
        assert_eq!(tables.end_count(), 2, "and leaves both ends in the store");

        tables
            .drop_end(EndKind::StreamReadable, readable)
            .expect("an idle readable stream end drops");
        assert_eq!(tables.end_count(), 0, "the second drop frees both ends");
        assert_eq!(tables.shared_record_count(), 0, "and the shared record");
        assert!(tables.end(readable).is_none());
    }

    #[wcmp_macros::test]
    fn it_takes_an_end_out_of_its_set_when_the_end_drops() {
        let mut tables = TaskTables::new();
        let set = tables.insert_waitable_set();
        let (readable, _) = tables.insert_ends(None);
        let waitable = WaitableId::from_end(EndKind::FutureReadable, readable);
        tables
            .join_waitable_set(waitable, Some(set))
            .expect("the end joins the set");

        tables
            .drop_waitable(waitable)
            .expect("an idle readable future end drops");

        assert!(
            tables
                .waitable_set(set)
                .is_some_and(|record| record.waitables.is_empty()),
            "the dropped end left the set"
        );
        tables
            .drop_waitable_set(set)
            .expect("so the set drops cleanly");
    }
}
