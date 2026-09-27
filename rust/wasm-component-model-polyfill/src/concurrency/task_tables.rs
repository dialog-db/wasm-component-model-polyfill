//! The store's tables of task, subtask, thread, waitable set, end,
//! shared, error-context, and instance records, with the stack of
//! current scopes.

use std::sync::Arc;

use crate::abi::signature::Signature;
use crate::error::{CopyCause, Error, Result, ThreadCause, WaitableCause};
use crate::executor::ir::CanonOptions;
use crate::internal::ErrorInternal;
use crate::resource::TableId;
use crate::types::ValueType;

use super::copy_buffer::CopyBuffer;
use super::copy_end::CopyEnd;
use super::copy_result::CopyResult;
use super::copy_state::CopyState;
use super::end_direction::EndDirection;
use super::end_id::EndId;
use super::end_kind::EndKind;
use super::error_context_id::ErrorContextId;
use super::error_context_record::ErrorContextRecord;
use super::event::Event;
use super::event_code::EventCode;
use super::failure_channel::FailureChannel;
use super::instance_id::InstanceId;
use super::instance_record::InstanceRecord;
use super::lower_kind::LowerKind;
use super::pairing::Pairing;
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
use super::thread_start::ThreadStart;
use super::waitable_id::WaitableId;
use super::waitable_set::WaitableSet;
use super::waitable_set_id::WaitableSetId;
use super::waitable_state::WaitableState;

/// The store's tables of task, subtask, thread, waitable set, end,
/// shared, error-context, and instance records, with the stack of
/// current scopes.
///
/// A scope is a task record or a subtask record, and the innermost
/// of them on the stack is the current scope. The stack also carries
/// a mark for each nested start in progress, which the cause of a
/// failed block reads. Every borrow operation consults it: a
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
///
/// The current thread is the implicit thread of the current task,
/// except while an explicit thread runs. An explicit thread runs in
/// its task's scope, pushed again for the length of the thread's run,
/// and the tables remember which entry of the stack that push was:
/// while that entry is the innermost task on the stack, the explicit
/// thread is the current thread.
///
/// The tables keep the threads that wait, too. A waiting thread's
/// record holds its readiness condition, and two lists name the
/// waiting threads: one in the order they began to wait, and one in
/// the order the scheduler found their conditions holding.
pub struct TaskTables {
    tasks: RecordTable<Task>,
    subtasks: RecordTable<Subtask>,
    threads: RecordTable<Thread>,
    waitable_sets: RecordTable<WaitableSet>,
    ends: RecordTable<CopyEnd>,
    shared_records: RecordTable<SharedRecord>,
    error_contexts: RecordTable<ErrorContextRecord>,
    instances: Vec<InstanceRecord>,
    scopes: Vec<Scope>,
    prepared_call: Option<SubtaskId>,
    signalled_sets: Vec<WaitableSetId>,
    running_threads: Vec<(usize, ThreadId)>,
    waiting: Vec<ThreadId>,
    ready: Vec<ThreadId>,
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
            error_contexts: RecordTable::new(),
            instances: Vec::new(),
            scopes: Vec::new(),
            prepared_call: None,
            signalled_sets: Vec::new(),
            running_threads: Vec::new(),
            waiting: Vec::new(),
            ready: Vec::new(),
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
    ///
    /// The task's implicit thread takes its index in the thread
    /// table of the task's instance here, as the reference registers
    /// the implicit thread once the task is past the entry gate. A
    /// task that belongs to no instance has no table to join, and a
    /// thread that already holds an index keeps it.
    pub fn start_task(&mut self, task: TaskId) {
        let Some(record) = self.task_mut(task) else {
            return;
        };
        record.state = TaskState::Started;
        let thread = record.implicit_thread;
        let _ = self.register_thread(thread);
    }

    /// Give `thread` an index in the thread table of its task's
    /// instance, and answer the index. A thread that already holds
    /// one answers it again. `None` when the thread or its task is
    /// gone, when the task belongs to no instance, or when the
    /// instance's table has no index left to hand out.
    pub fn register_thread(&mut self, thread: ThreadId) -> Option<u32> {
        let record = self.thread(thread)?;
        if let Some(index) = record.index {
            return Some(index);
        }
        let instance = self.task(record.task)?.instance?;
        let index = self.instance_mut(instance)?.threads.insert(thread)?;
        self.thread_mut(thread)?.index = Some(index);
        Some(index)
    }

    /// Take `thread` out of the thread table of its task's instance,
    /// when it holds an index there.
    fn unregister_thread(&mut self, thread: ThreadId) {
        let Some(record) = self.thread(thread) else {
            return;
        };
        let Some(index) = record.index else {
            return;
        };
        let instance = self.task(record.task).and_then(|task| task.instance);
        if let Some(instance) = instance.and_then(|instance| self.instance_mut(instance)) {
            instance.threads.remove(index, thread);
        }
        if let Some(record) = self.thread_mut(thread) {
            record.index = None;
        }
    }

    /// Create an explicit thread of `task`, suspended, that runs
    /// `start` when it starts, and answer it with its index in the
    /// thread table of the task's instance. This is the record half
    /// of `thread.new-indirect`. `None` when the task is gone, when
    /// it belongs to no instance, or when the instance's table has
    /// no index left; the thread is not created then.
    pub fn create_thread(&mut self, task: TaskId, start: ThreadStart) -> Option<(ThreadId, u32)> {
        self.task(task)?.instance?;
        let (index, generation) = self
            .threads
            .insert_with_generation(Thread::explicit(task, start));
        let thread = ThreadId::new(index, generation);
        let Some(table_index) = self.register_thread(thread) else {
            self.threads.remove(index);
            return None;
        };
        if let Some(record) = self.task_mut(task) {
            record.threads.push(thread);
        }
        Some((thread, table_index))
    }

    /// The thread at `index` of `instance`'s thread table.
    pub fn thread_at(&self, instance: InstanceId, index: u32) -> Option<ThreadId> {
        self.instance(instance)?.threads.get(index)
    }

    /// Suspend the running thread `thread`: it neither runs nor waits
    /// to run until a resume names it. This is what `thread.suspend`
    /// and the switching built-ins do to the thread that calls them.
    pub fn suspend_thread(&mut self, thread: ThreadId) -> Result<()> {
        self.thread_mut(thread)
            .ok_or_else(|| Error::internal("a suspending thread is not in the store"))?
            .suspended = true;
        Ok(())
    }

    /// Whether `thread` is suspended. A thread whose record is gone
    /// is not.
    pub fn thread_suspended(&self, thread: ThreadId) -> bool {
        self.thread(thread).is_some_and(|record| record.suspended)
    }

    /// Make the suspended thread `thread` ready, which is the record
    /// half of `thread.resume-later` and the reference's
    /// `Thread.resume_later`: the thread waits on a condition that
    /// always holds. Answers whether the thread has never run, in
    /// which case its start still has to be queued. A thread that is
    /// not suspended fails with Wasmtime's message.
    pub fn resume_later(&mut self, thread: ThreadId) -> Result<bool> {
        let record = self.thread_mut(thread).ok_or_else(|| {
            Error::internal("a thread table names a thread the store does not hold")
        })?;
        if !record.suspended {
            return Err(Error::Thread(ThreadCause::NotSuspended));
        }
        record.suspended = false;
        let never_ran = record.start.is_some();
        self.start_waiting(thread, Readiness::Yielded)?;
        Ok(never_ran)
    }

    /// Take what the explicit thread `thread` runs as it starts,
    /// together with its task. The thread is running from here on: it
    /// is not suspended, and a wait `thread.resume-later` recorded
    /// for it ends. `None` when the thread has started already, or
    /// its record is gone.
    pub fn take_thread_start(&mut self, thread: ThreadId) -> Option<(TaskId, ThreadStart)> {
        let record = self.thread_mut(thread)?;
        let start = record.start.take()?;
        record.suspended = false;
        let task = record.task;
        self.stop_waiting(thread, None);
        Some((task, start))
    }

    /// End `thread` on its own: it leaves its instance's thread table
    /// and its task's list of threads, and its record leaves the
    /// store. The task itself stays. This is the end of an explicit
    /// thread whose start function returned or failed.
    pub fn end_thread(&mut self, thread: ThreadId) {
        self.unregister_thread(thread);
        let Some(task) = self.thread(thread).map(|record| record.task) else {
            return;
        };
        if let Some(record) = self.task_mut(task) {
            record.threads.retain(|held| *held != thread);
        }
        if let Some(index) = self.thread_index(thread) {
            self.threads.remove(index);
        }
    }

    /// Whether `task` holds a thread other than its implicit thread:
    /// an explicit thread that has not ended, whether it has started
    /// or not. Such a task goes on after its implicit thread exits.
    pub fn has_explicit_threads(&self, task: TaskId) -> bool {
        self.task(task).is_some_and(|record| {
            record
                .threads
                .iter()
                .any(|thread| *thread != record.implicit_thread)
        })
    }

    /// End the implicit thread of `task` on its own, which is the
    /// reference's `unregister_thread` for a task that holds another
    /// thread: the thread leaves its instance's table and its task's
    /// list of threads, and its record leaves the store. The task
    /// stays, marked as one whose implicit thread has exited, and the
    /// end of its last thread is what ends it.
    pub fn retire_implicit_thread(&mut self, task: TaskId) {
        let Some(thread) = self.task(task).map(|record| record.implicit_thread) else {
            return;
        };
        self.waiting.retain(|waiting| *waiting != thread);
        self.ready.retain(|ready| *ready != thread);
        self.end_thread(thread);
        if let Some(record) = self.task_mut(task) {
            record.implicit_thread_exited = true;
        }
    }

    /// Whether `task` has no thread left after its implicit thread
    /// exited, so that the thread that just ended was its last and the
    /// task ends now.
    pub fn outlived_its_threads(&self, task: TaskId) -> bool {
        self.task(task)
            .is_some_and(|record| record.implicit_thread_exited && record.threads.is_empty())
    }

    /// Make the explicit thread `thread` the current thread, by
    /// pushing its task as the current scope and remembering that
    /// the push was the thread's. `None` when the thread or its task
    /// is gone, and nothing is pushed then.
    pub fn enter_thread(&mut self, thread: ThreadId) -> Option<()> {
        let task = self.thread(thread)?.task;
        self.task(task)?;
        self.scopes.push(Scope::Task(task));
        self.running_threads.push((self.scopes.len() - 1, thread));
        Some(())
    }

    /// Where on the stack of current scopes the explicit thread
    /// `thread` pushed its task's scope, while it is running.
    pub fn running_thread_position(&self, thread: ThreadId) -> Option<usize> {
        self.running_threads
            .iter()
            .rev()
            .find(|(_, running)| *running == thread)
            .map(|(position, _)| *position)
    }

    /// Forget that the explicit thread `thread` is running. The scope
    /// it pushed is popped by the caller.
    pub fn forget_running_thread(&mut self, thread: ThreadId) {
        if let Some(at) = self
            .running_threads
            .iter()
            .rposition(|(_, running)| *running == thread)
        {
            self.running_threads.remove(at);
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

    /// Pop the top entry of the stack, if there is one.
    pub fn pop_scope(&mut self) -> Option<Scope> {
        self.scopes.pop()
    }

    /// The current scope: the innermost entry of the stack that is a
    /// task or a subtask. A nested-start mark is not a scope, so the
    /// scope under it stays current until the callee pushes its own.
    pub fn current_scope(&self) -> Option<Scope> {
        self.scopes
            .iter()
            .rev()
            .find(|scope| !scope.is_mark())
            .copied()
    }

    /// Mark the stack where a trampoline starts a thread from inside
    /// itself, as a start intrinsic does for an `async`-typed callee.
    /// `subtask` is the caller's record of the call and `lower` is
    /// how the caller lowered it. The thread runs above the mark
    /// until it returns to the trampoline, which then takes the mark
    /// off with [`end_nested_start`](Self::end_nested_start).
    pub fn begin_nested_start(&mut self, subtask: SubtaskId, lower: LowerKind) {
        self.scopes.push(Scope::NestedStart { subtask, lower });
    }

    /// Take off the innermost nested-start mark. The mark is found
    /// rather than popped, so that a scope a failed call stranded
    /// above it stays for the unwind that owns it. A mark an unwind
    /// already discarded leaves nothing to take.
    pub fn end_nested_start(&mut self) {
        if let Some(at) = self
            .scopes
            .iter()
            .rposition(|scope| matches!(scope, Scope::NestedStart { .. }))
        {
            self.scopes.remove(at);
        }
    }

    /// Mark the stack where the thread built-in of `thread` starts a
    /// thread it switched to, from inside itself. The started thread
    /// runs above the mark until it returns to the built-in, which
    /// then takes the mark off with
    /// [`end_thread_switch`](Self::end_thread_switch).
    pub fn begin_thread_switch(&mut self, thread: ThreadId) {
        self.scopes.push(Scope::ThreadSwitch { thread });
    }

    /// Take off the innermost thread-switch mark, under the rule
    /// [`end_nested_start`](Self::end_nested_start) states for its
    /// own mark.
    pub fn end_thread_switch(&mut self) {
        if let Some(at) = self
            .scopes
            .iter()
            .rposition(|scope| matches!(scope, Scope::ThreadSwitch { .. }))
        {
            self.scopes.remove(at);
        }
    }

    /// Whether a frame below the current one would go on under a
    /// stack switch: the stack carries a nested-start mark whose
    /// caller would run its own code once control came back to it.
    ///
    /// An asynchronous lower's caller always would: the lower
    /// answers with the status word and the caller goes on from
    /// there. A synchronous lower's caller would only once the
    /// callee has resolved, because the lower returns the callee's
    /// result. Before that, the caller would only wait for the
    /// callee, and the wait runs the same ready work the callee's own
    /// block already ran, so it releases nothing. A record that is
    /// gone counts as resolved. A frame further down that would go
    /// on has a mark of its own.
    ///
    /// A thread-switch mark counts while the thread that switched is
    /// not suspended: it yielded to the thread it started, or a
    /// `thread.resume-later` has made it ready since. A thread that
    /// stays suspended would get control back only once something
    /// resumed it, and that is the ready work the block above it
    /// already ran.
    pub fn caller_below_goes_on(&self) -> bool {
        self.scopes.iter().any(|scope| match *scope {
            Scope::NestedStart {
                lower: LowerKind::Async,
                ..
            } => true,
            Scope::NestedStart {
                subtask,
                lower: LowerKind::Sync,
            } => self
                .subtask(subtask)
                .is_none_or(|record| record.state.resolved()),
            Scope::ThreadSwitch { thread } => {
                self.thread(thread).is_some_and(|record| !record.suspended)
            }
            Scope::Task(_) | Scope::Subtask(_) => false,
        })
    }

    /// Whether a caller below waits for the blocked callee in an
    /// instance that must not suspend, with no other thread of that
    /// instance ready.
    ///
    /// A caller below waits for the call above it when it made that
    /// call synchronously and the call has not resolved: a fused
    /// adapter's direct call, which leaves the callee's task scope
    /// right above the caller's, or a synchronous lower whose start
    /// intrinsic left a nested-start mark between the two. That wait
    /// is a block of the caller's own instance. In the reference every
    /// call runs as a thread of its own, so the blocked callee hands
    /// control back to the caller's lower, whose thread then blocks,
    /// and a sync-typed caller's `canon_lift` traps when no thread of
    /// its instance is ready. Wasmtime traps there too, through
    /// `switch_or_trap_if_may_not_suspend` on the caller's instance
    /// after a start intrinsic's callee suspended.
    ///
    /// The callers are read from the innermost out. A caller whose
    /// instance may suspend waits in turn, so its own caller decides.
    /// The search ends at a frame that would go on, which
    /// [`caller_below_goes_on`](Self::caller_below_goes_on) reads, and
    /// at a frame that is no synchronous call between two guests: a
    /// host call's subtask, or a thread built-in's switch.
    pub fn caller_below_cannot_block(&self) -> bool {
        let mut blocked_seen = false;
        for (position, scope) in self.scopes.iter().enumerate().rev() {
            match *scope {
                Scope::Task(_) if !blocked_seen => blocked_seen = true,
                Scope::Task(_) => {
                    let instance = self
                        .thread_at_depth(position + 1)
                        .and_then(|caller| self.thread_instance(caller));
                    let must_not_suspend = instance
                        .and_then(|instance| self.instance(instance))
                        .is_some_and(|record| record.may_not_suspend);
                    if let (true, Some(instance)) = (must_not_suspend, instance) {
                        return !self.other_thread_ready_in(instance);
                    }
                }
                Scope::NestedStart {
                    subtask,
                    lower: LowerKind::Sync,
                } if self
                    .subtask(subtask)
                    .is_some_and(|record| !record.state.resolved()) => {}
                Scope::NestedStart { .. } | Scope::ThreadSwitch { .. } | Scope::Subtask(_) => {
                    return false;
                }
            }
        }
        false
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
            Scope::NestedStart { .. } | Scope::ThreadSwitch { .. } => return None,
        };
        self.scopes[..under]
            .iter()
            .rev()
            .find_map(|scope| match scope {
                Scope::Task(task) => Some(*task),
                Scope::Subtask(_) | Scope::NestedStart { .. } | Scope::ThreadSwitch { .. } => None,
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
            Scope::Task(_) | Scope::NestedStart { .. } | Scope::ThreadSwitch { .. } => None,
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
    /// that starts it to the return that ends it. An explicit thread
    /// is the current thread while the scope it pushed is the
    /// innermost task on the stack, which is what
    /// [`enter_thread`](Self::enter_thread) records.
    pub fn current_thread(&self) -> Option<ThreadId> {
        let task = self.current_task()?;
        let at = self
            .scopes
            .iter()
            .rposition(|scope| matches!(scope, Scope::Task(_)));
        let explicit = self
            .running_threads
            .iter()
            .rev()
            .find(|(position, _)| Some(*position) == at)
            .map(|(_, thread)| *thread)
            .filter(|thread| {
                self.thread(*thread)
                    .is_some_and(|record| record.task == task)
            });
        match explicit {
            Some(thread) => Some(thread),
            None => Some(self.task(task)?.implicit_thread),
        }
    }

    /// Take the top of the stack of current scopes off it, from
    /// position `base` up, with the explicit threads running among
    /// those scopes. Each thread's position is counted from `base`.
    ///
    /// This is what a thread that suspends in the provider leaves the
    /// real stack with: the scopes it pushed after its entry began,
    /// which [`restore_scopes`](Self::restore_scopes) puts back on
    /// top of the stack when it resumes.
    pub fn cut_scopes(&mut self, base: usize) -> (Vec<Scope>, Vec<(usize, ThreadId)>) {
        let base = base.min(self.scopes.len());
        let scopes = self.scopes.split_off(base);
        let mut running = Vec::new();
        self.running_threads.retain(|(position, thread)| {
            if *position < base {
                return true;
            }
            running.push((position - base, *thread));
            false
        });
        (scopes, running)
    }

    /// Put `scopes` back on top of the stack of current scopes, with
    /// the explicit threads `running` among them, which is what a
    /// thread that resumes from the provider does. The positions of
    /// the threads are counted from the first of `scopes`.
    pub fn restore_scopes(&mut self, scopes: Vec<Scope>, running: Vec<(usize, ThreadId)>) {
        let base = self.scopes.len();
        self.scopes.extend(scopes);
        self.running_threads.extend(
            running
                .into_iter()
                .map(|(position, thread)| (base + position, thread)),
        );
    }

    /// Record whether `thread` runs on a stack of its own, under the
    /// provider. A thread that does can suspend in a shim.
    pub fn set_own_stack(&mut self, thread: ThreadId, own: bool) {
        if let Some(record) = self.thread_mut(thread) {
            record.own_stack = own;
        }
    }

    /// Whether `thread` runs on a stack of its own, under the
    /// provider.
    pub fn on_own_stack(&self, thread: ThreadId) -> bool {
        self.thread(thread).is_some_and(|record| record.own_stack)
    }

    /// Record which thread's frame `thread` goes back to when it
    /// suspends through the provider, as it starts or resumes there.
    /// `depth` is how deep the stack of current scopes was before
    /// `thread`'s own scopes went on it, so the thread current at that
    /// depth is the one whose frame starts or resumes it.
    ///
    /// That thread is recorded only when it belongs to the same
    /// instance as `thread` and the instance may not suspend. Its
    /// frame is then a block of the instance's own call, which runs
    /// the ready threads of the instance and nothing else, as the
    /// reference's `canon_lift` does once a thread of a sync-typed
    /// call's instance blocks. Any other frame records nothing.
    pub fn note_returns_to(&mut self, thread: ThreadId, depth: usize) {
        let instance = self.thread_instance(thread);
        let returns_to = self.thread_at_depth(depth).filter(|starter| {
            *starter != thread
                && instance.is_some()
                && self.thread_instance(*starter) == instance
                && instance
                    .and_then(|instance| self.instance(instance))
                    .is_some_and(|record| record.may_not_suspend)
        });
        if let Some(record) = self.thread_mut(thread) {
            record.returns_to = returns_to;
        }
    }

    /// The thread `thread` goes back to when it suspends through the
    /// provider, when that is a block of its own instance's call, as
    /// [`note_returns_to`](Self::note_returns_to) recorded it.
    pub fn returns_to(&self, thread: ThreadId) -> Option<ThreadId> {
        self.thread(thread).and_then(|record| record.returns_to)
    }

    /// The instance `thread`'s task belongs to.
    fn thread_instance(&self, thread: ThreadId) -> Option<InstanceId> {
        let task = self.thread(thread)?.task;
        self.task(task)?.instance
    }

    /// The thread that was current while the stack of current scopes
    /// was `depth` deep, read as [`current_thread`](Self::current_thread)
    /// reads the whole stack.
    fn thread_at_depth(&self, depth: usize) -> Option<ThreadId> {
        let scopes = &self.scopes[..depth.min(self.scopes.len())];
        let at = scopes
            .iter()
            .rposition(|scope| matches!(scope, Scope::Task(_)))?;
        let Scope::Task(task) = scopes[at] else {
            return None;
        };
        let explicit = self
            .running_threads
            .iter()
            .rev()
            .find(|(position, _)| *position == at)
            .map(|(_, thread)| *thread)
            .filter(|thread| {
                self.thread(*thread)
                    .is_some_and(|record| record.task == task)
            });
        match explicit {
            Some(thread) => Some(thread),
            None => Some(self.task(task)?.implicit_thread),
        }
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
            Scope::NestedStart { .. } | Scope::ThreadSwitch { .. } => false,
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
            Scope::NestedStart { .. } | Scope::ThreadSwitch { .. } => Vec::new(),
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
            self.unregister_thread(thread);
            if let Some(index) = self.thread_index(thread) {
                self.threads.remove(index);
            }
            self.waiting.retain(|waiting| *waiting != thread);
            self.ready.retain(|ready| *ready != thread);
        }
    }

    // ---- error contexts ----

    /// Create an error-context record holding `debug_message`, named
    /// by one handle, and return its identity. The
    /// `error-context.new` built-in calls this and puts the
    /// identity's index in a handle-table entry for the guest.
    pub fn insert_error_context(&mut self, debug_message: String) -> ErrorContextId {
        let (index, generation) = self
            .error_contexts
            .insert_with_generation(ErrorContextRecord::new(debug_message));
        ErrorContextId::new(index, generation)
    }

    /// One error-context record. `None` once the record the identity
    /// was minted for is gone, under the rule
    /// [`task_index`](Self::task_index) states.
    pub fn error_context(&self, context: ErrorContextId) -> Option<&ErrorContextRecord> {
        self.error_contexts.get(self.error_context_index(context)?)
    }

    /// Take one handle away from the error context `context`: its
    /// count drops by one, and the record leaves the store when the
    /// count reaches zero. A handle-table entry names a live record,
    /// so an identity that names none is an internal failure.
    pub fn release_error_context(&mut self, context: ErrorContextId) -> Result<()> {
        let index = self
            .error_context_index(context)
            .ok_or_else(|| Error::internal("an error-context handle named no record"))?;
        let record = self
            .error_contexts
            .get_mut(index)
            .ok_or_else(|| Error::internal("an error-context handle named no record"))?;
        record.handle_count = record
            .handle_count
            .checked_sub(1)
            .ok_or_else(|| Error::internal("an error-context record counted no handle"))?;
        if record.handle_count == 0 {
            self.error_contexts.remove(index);
        }
        Ok(())
    }

    /// How many error-context records the store holds.
    pub fn error_context_count(&self) -> usize {
        self.error_contexts.len()
    }

    /// The index `context` names, under the rule
    /// [`task_index`](Self::task_index) states.
    fn error_context_index(&self, context: ErrorContextId) -> Option<u32> {
        (self.error_contexts.generation(context.index()) == context.generation())
            .then_some(context.index())
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
    /// thread starts waiting with a readiness condition that names
    /// the set. The scheduler suspends the thread after this, and
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
        self.start_waiting(thread, Readiness::WaitableSet { set })?;
        Ok(())
    }

    /// The wait [`begin_wait`](Self::begin_wait) parked `thread` for
    /// is over: the set's waiter count falls and the thread stops
    /// waiting.
    pub fn end_wait(&mut self, set: WaitableSetId, thread: ThreadId) -> Result<()> {
        let record = self.waitable_set_record_mut(set)?;
        record.num_waiting = record.num_waiting.saturating_sub(1);
        self.stop_waiting(thread, None);
        Ok(())
    }

    // ---- waiting threads ----

    /// Record that `thread` waits until `readiness` holds, which is
    /// what the try part of a blocking built-in sets up. A thread that
    /// waited on nothing joins the back of the list of waiting
    /// threads.
    ///
    /// Answers the condition the thread waited on before, which is
    /// `None` unless this block nests inside another block of the
    /// same thread. [`stop_waiting`](Self::stop_waiting) puts it
    /// back.
    pub fn start_waiting(
        &mut self,
        thread: ThreadId,
        readiness: Readiness,
    ) -> Result<Option<Readiness>> {
        let record = self
            .thread_mut(thread)
            .ok_or_else(|| Error::internal("waiting thread is not in the store"))?;
        let previous = record.readiness.replace(readiness);
        if previous.is_none() {
            self.waiting.push(thread);
        }
        Ok(previous)
    }

    /// The wait [`start_waiting`](Self::start_waiting) recorded for
    /// `thread` is over: the thread resumed, or its block failed. Its
    /// record takes back `previous`, the condition it waited on
    /// before, and a thread that now waits on nothing leaves both
    /// lists.
    pub fn stop_waiting(&mut self, thread: ThreadId, previous: Option<Readiness>) {
        if let Some(record) = self.thread_mut(thread) {
            record.readiness = previous;
        }
        if previous.is_none() {
            self.waiting.retain(|waiting| *waiting != thread);
            self.ready.retain(|ready| *ready != thread);
        }
    }

    /// Whether `readiness` holds.
    ///
    /// The evaluation reads these tables and nothing else. It changes
    /// nothing, polls no host future, and runs no guest code, which
    /// is the property of the reference's `ready_func`: the tables
    /// are borrowed shared, and they hold neither a host future nor a
    /// guest function. A record the condition names that cannot be
    /// read answers `false`, except a subtask record that is gone,
    /// which a call that failed takes away with it.
    pub fn readiness_holds(&self, readiness: Readiness) -> bool {
        match readiness {
            Readiness::WaitableSet { set } => self.set_has_pending_event(set).unwrap_or(false),
            Readiness::Waitable { waitable } => self.has_pending_event(waitable).unwrap_or(false),
            Readiness::Subtask { subtask } => self
                .subtask(subtask)
                .is_none_or(|record| record.state.resolved()),
            Readiness::EntryGate => false,
            Readiness::Yielded => true,
            Readiness::Resumed { thread } => !self.thread_suspended(thread),
            Readiness::Planned => true,
        }
    }

    /// Whether `thread` waits and its readiness condition holds,
    /// which is the reference's `Thread.ready`. A thread that runs,
    /// or whose record is gone, is not ready.
    pub fn thread_ready(&self, thread: ThreadId) -> bool {
        self.thread(thread)
            .and_then(|record| record.readiness)
            .is_some_and(|readiness| self.readiness_holds(readiness))
    }

    /// Evaluate the condition of every waiting thread, which the
    /// scheduler does between two items, and note the threads that
    /// became ready since the last evaluation.
    ///
    /// A thread that became ready joins the back of the ready list,
    /// so the list keeps the order in which the threads became ready.
    /// Threads that became ready together, between the same two
    /// items, join it in the order they began to wait. A thread on
    /// the list whose condition no longer holds leaves it: another
    /// thread took what it waited for, and it joins again at the back
    /// once its condition holds again.
    ///
    /// Only the ready list changes here. Each condition is evaluated
    /// through [`readiness_holds`](Self::readiness_holds), which
    /// changes nothing.
    pub fn note_ready_threads(&mut self) {
        if self.waiting.is_empty() {
            return;
        }
        let ready: Vec<ThreadId> = self
            .waiting
            .iter()
            .copied()
            .filter(|thread| self.thread_ready(*thread))
            .collect();
        self.ready.retain(|thread| ready.contains(thread));
        for thread in ready {
            if !self.ready.contains(&thread) {
                self.ready.push(thread);
            }
        }
    }

    /// The waiting threads whose conditions held when the scheduler
    /// last evaluated them and still hold, in the order they became
    /// ready. Threads that became ready together resume in the order
    /// they became ready, so this is the order a waiting thread
    /// resumes in.
    pub fn ready_threads(&self) -> Vec<ThreadId> {
        self.ready
            .iter()
            .copied()
            .filter(|thread| self.thread_ready(*thread))
            .collect()
    }

    /// Whether a thread of `instance` other than the current one waits
    /// on a condition that holds: a thread the reference's `canon_lift`
    /// could run once a thread of that instance blocks.
    pub fn other_thread_ready_in(&self, instance: InstanceId) -> bool {
        let current = self.current_thread();
        self.waiting.iter().any(|thread| {
            Some(*thread) != current
                && self.thread_instance(*thread) == Some(instance)
                && self.thread_ready(*thread)
        })
    }

    /// The threads that wait, in the order they began to wait.
    pub fn waiting_threads(&self) -> &[ThreadId] {
        &self.waiting
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
    /// subtask a thread waits on synchronously. A stream or future
    /// end follows the rules [`drop_end`](Self::drop_end) states. The
    /// waitable leaves the set it joined on its way out.
    pub fn drop_waitable(&mut self, waitable: WaitableId) -> Result<()> {
        // An end a thread waits on synchronously is copying, and the
        // end's own busy check names that the way Wasmtime does, so
        // an end goes to its own rules before the waiter is looked at.
        if let Some((kind, end)) = waitable.end() {
            return self.drop_end(kind, end);
        }
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
            _ => Err(Error::internal("waitable kind has no record in the store")),
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

    /// Create one stream or future whose writable end the host serves,
    /// of kind `host`: the records [`insert_ends`](Self::insert_ends)
    /// creates, with the shared record marking the writable end as
    /// the host's. Answers the readable end and then the writable
    /// end.
    ///
    /// Neither end enters a handle table here. The host holds the
    /// readable end until it lowers it into a call or pipes it, and
    /// the writable end never enters a table: the scheduler holds the
    /// producer that serves it.
    pub fn insert_host_ends(
        &mut self,
        payload: Option<ValueType>,
        host: EndKind,
    ) -> (EndId, EndId) {
        let (readable, writable) = self.insert_ends(payload);
        let shared = self.end_mut(readable).map(|record| {
            record.held_by_host = true;
            record.shared
        });
        if let Some(shared) = shared
            && let Some(record) = self.shared_records.get_mut(shared)
        {
            record.host = Some(host);
        }
        (readable, writable)
    }

    /// Whether `end` is a readable end the host holds: one it created
    /// or lifted out of a guest, and has not lowered, piped, or
    /// closed since. The end record's `held_by_host` flag answers it,
    /// and it is the one check the lower, the pipe, and the close of
    /// a host value make beyond the end's presence.
    ///
    /// A host value names its end by identity, and cloning a
    /// [`StreamAny`](crate::StreamAny) or a
    /// [`FutureAny`](crate::FutureAny), or converting one into a
    /// reader, copies the identity. So once one value lowers, pipes,
    /// or closes the end, another may try to use it again. The
    /// specification is silent on that: its `lower_stream` and
    /// `lower_future` assert only the value's type and add a new
    /// readable end sharing the stream or future, and what the host
    /// holds is the host's to define. So the polyfill follows
    /// Wasmtime, whose values are the same copies of an id.
    ///
    /// Wasmtime checks no holder. Its lower (`lower_transmit_to_index`
    /// in `futures_and_streams.rs`), its pipe (`set_consumer`), its
    /// close (`host_drop_reader`), and its conversion to an untyped
    /// value (`transmit_origin`) each look the id up in the store's
    /// table and fail only when nothing is there, with the table's
    /// "resource not present". The entry leaves the table only when
    /// both ends have dropped, when the host closes a stream or
    /// future it created, and when a pipe finds the writer dropped or
    /// runs to its end. A value's own close leaves it naming nothing
    /// too, because the close replaces the value's id with
    /// `u32::MAX`. The polyfill fails every such use with the
    /// not-present cause and Wasmtime's message, and a value it
    /// closed holds an identity that names no end.
    ///
    /// While the entry is there, Wasmtime's use succeeds, and the
    /// polyfill follows it where the result is one Wasmtime's own
    /// code treats as sound:
    ///
    /// - A conversion of a reader to an untyped value checks only
    ///   that the end is in the store, so it succeeds whoever holds
    ///   the end, and the new value is one more copy. The conversion
    ///   the other way checks only the payload type.
    /// - A close of an end already dropped, by a close through
    ///   another value or by the guest it was lowered into, while a
    ///   guest still holds the writable end, succeeds and changes
    ///   nothing. Wasmtime drops the end a second time: the reader
    ///   stays dropped, and the writer is told again of a drop it was
    ///   told of. When that first notice is still pending, Wasmtime
    ///   merges the two and nothing is observable. When the writer
    ///   took it, Wasmtime sets the notice again, which a waitable
    ///   set then reports. The polyfill does not: its end drops once,
    ///   and that drop is what tells the writer.
    /// - A pipe or a close of an end the host piped to a consumer
    ///   while a guest holds the writable end, when no write of that
    ///   end is in flight, as
    ///   [`consumer_at_rest`](Self::consumer_at_rest) answers. A
    ///   second pipe replaces the consumer: Wasmtime's `set_consumer`
    ///   on a writer that is open (`WriteState::Open`) puts the new
    ///   consumer in the reading side's place
    ///   (`ReadState::HostReady`), which drops the old one unpolled,
    ///   and the next write polls the new one. A close drops the end
    ///   as a guest's drop does: Wasmtime's `host_drop_reader` on an
    ///   open writer drops the reading side, and the consumer with
    ///   it, unpolled, and gives the writer the dropped result.
    ///
    /// Everywhere else the polyfill refuses what Wasmtime lets
    /// through. Wasmtime lets it through into a state that its own
    /// code rejects as a bug of its own, with `bail_bug!`, which
    /// panics in a debug build and traps in a release build, either
    /// at the next ordinary step or when the timing goes one way. The
    /// polyfill cannot reproduce a bug as a behaviour, so it refuses
    /// the use that leads there:
    ///
    /// - A lower after a pipe, a close, or a lower of another value.
    ///   The guest's new entry names an end a consumer serves, an end
    ///   dropped already, or an end another entry names, and a read
    ///   through it fails Wasmtime's check that the reading side is
    ///   open (`guest_read`, "expected `ReadState::Open`"). That holds
    ///   for a stream the host created and piped to a consumer of its
    ///   own too.
    /// - A pipe after a lower: the guest that holds the end then
    ///   fails its read the same way.
    /// - A pipe or a close after a pipe, while a guest's write the
    ///   consumer serves is in flight. Wasmtime's consumer is then
    ///   settling the write in a task of its own. A second pipe
    ///   starts a second such task for the same write, both settle
    ///   it, and the second to finish fails "expected
    ///   `WriteState::GuestReady`" (`pipe_from_guest`). A close drops
    ///   the reading side under the first task, whose next poll fails
    ///   "unexpected read state" (`set_consumer`).
    /// - A close after a lower: the guest's read then finds the
    ///   reading side dropped, and fails as above.
    /// - A pipe after a close: Wasmtime opens the reading side again,
    ///   for a writer that was told the reader dropped. The polyfill's
    ///   drop of an end is final, as the specification's is: its
    ///   writer never sees a dropped reader come back. An end whose
    ///   consumer finished dropped the same way.
    ///
    /// One refusal is the polyfill's own: a pipe or a close after a
    /// pipe of a stream or future the host created, to a consumer of
    /// its own. That pipe joins the producer and the consumer in the
    /// host task it starts, and no later call reaches into that task
    /// to replace or drop the consumer. Wasmtime's second pipe there
    /// fails "unexpected invocation of `produce`", and its close
    /// deletes the stream under the task that copies.
    ///
    /// Those fail with the not-held cause, or as an invalid handle
    /// for a lower. An end that goes back into a guest and comes back
    /// out is the host's again, and so is every value that names it,
    /// as in Wasmtime.
    pub fn held_by_host(&self, end: EndId) -> bool {
        self.end(end)
            .is_some_and(|record| record.direction == EndDirection::Readable && record.held_by_host)
    }

    /// Whether `end` is a readable end the host piped to a consumer
    /// while a guest held the writable end, and no write of that end
    /// is in flight, so a later pipe may replace the consumer and a
    /// close may drop it, as [`held_by_host`](Self::held_by_host)
    /// states. The consumer then waits in the scheduler, which a
    /// later call reaches. Neither end has dropped.
    pub fn consumer_at_rest(&self, end: EndId) -> bool {
        let Some(record) = self.end(end) else {
            return false;
        };
        let Some(shared) = self.shared_records.get(record.shared) else {
            return false;
        };
        record.direction == EndDirection::Readable
            && !record.held_by_host
            && !shared.dropped
            && shared
                .host
                .is_some_and(|host| direction_of(host) == EndDirection::Readable)
            && !self.write_in_flight(end)
    }

    /// Whether the writable end of `reader`'s stream or future has a
    /// write in progress that nothing has answered yet. Wasmtime
    /// holds such a write as `WriteState::GuestReady`; a write whose
    /// completion waits only to be delivered is over for it.
    pub fn write_in_flight(&self, reader: EndId) -> bool {
        self.shared_record(reader)
            .and_then(|record| self.end(record.writable))
            .is_some_and(|record| record.state.busy() && record.waitable.pending_event.is_none())
    }

    /// The payload of the stream or future of `end`, a readable end in
    /// the store, whoever holds it: a conversion to an untyped value
    /// checks no more, as [`held_by_host`](Self::held_by_host)
    /// states.
    ///
    /// Fails with the not-present cause as
    /// [`readable_end`](Self::readable_end) does.
    pub fn readable_payload(&self, end: EndId, kind: EndKind) -> Result<Option<ValueType>> {
        self.readable_end(end, kind)?;
        Ok(self
            .shared_record(end)
            .ok_or_else(|| Error::internal("a readable end has no shared record"))?
            .payload
            .clone())
    }

    /// The other end of `end`'s stream or future when the host serves
    /// it, while the host can still copy through it: the writable end
    /// of a stream or future the host created, whose producer serves
    /// reads, or the readable end of one the host piped to a consumer,
    /// which serves writes. Neither end has been dropped.
    pub fn host_counterpart(&self, end: EndId) -> Option<EndId> {
        let record = self.shared_record(end)?;
        let host = record.end_of(direction_of(record.host?));
        (host != end && !record.dropped).then_some(host)
    }

    /// Make the host the server of `reader`, a readable end of kind
    /// `kind` the host holds, as the reading side of a consumer the
    /// host piped it to. Afterwards a copy on the writable end polls
    /// the host's consumer rather than waiting for a guest to read.
    ///
    /// Fails with the not-held cause when `reader` is no readable end
    /// the host holds: the end is gone, a guest's table holds it, or
    /// it was piped or closed already.
    pub fn serve_reader(&mut self, reader: EndId, kind: EndKind) -> Result<()> {
        if !self.held_by_host(reader) {
            return Err(Error::Copy(CopyCause::NotHeldByHost { kind }));
        }
        if let Some(shared) = self.end(reader).map(|record| record.shared)
            && let Some(record) = self.shared_records.get_mut(shared)
        {
            record.host = Some(kind);
        }
        Ok(())
    }

    /// Close `reader`, a readable end of kind `kind`, because a host
    /// value that names it closed it, and answer the host end the
    /// caller lets go of: the writable end whose producer the host
    /// served, or `reader` itself when a consumer served it.
    ///
    /// When the host holds the end, it drops, and the rules of
    /// [`drop_end`](Self::drop_end) tell the writable end: a write in
    /// progress completes with the dropped result, and a later write
    /// sees it at once. A stream or future the host created takes its
    /// writable end with it, as a guest's drop of such a readable end
    /// does: nobody is left to read what the producer would produce,
    /// and both records leave the store.
    ///
    /// When the host piped the end to a consumer while a guest holds
    /// the writable end, and no write is in flight, as
    /// [`consumer_at_rest`](Self::consumer_at_rest) answers, the end
    /// drops the same way and tells the idle writer, and the caller
    /// lets the consumer go.
    ///
    /// When the end was dropped already, by an earlier close through
    /// another value that names it or by the guest the host lowered
    /// it into, and its record stays because a guest still holds the
    /// writable end, the close does nothing and succeeds, as
    /// [`held_by_host`](Self::held_by_host) states Wasmtime's does.
    ///
    /// Fails with the not-present cause when the store holds no
    /// readable end under `reader`, and with the not-held cause when
    /// the end lives on in a guest's table, or with a consumer that
    /// serves a write in flight or that the host's own pipe holds.
    pub fn close_host_reader(&mut self, reader: EndId, kind: EndKind) -> Result<Option<EndId>> {
        if self.consumer_at_rest(reader) {
            self.drop_end(kind, reader)?;
            return Ok(Some(reader));
        }
        let record = self.readable_end(reader, kind)?;
        if !record.held_by_host {
            return if record.dropped {
                Ok(None)
            } else {
                Err(Error::Copy(CopyCause::NotHeldByHost { kind }))
            };
        }
        let writer = self.host_counterpart(reader);
        self.end_record_mut(reader)?.held_by_host = false;
        self.drop_end(kind, reader)?;
        if let Some(writer) = writer {
            self.release_host_end(writer)?;
        }
        Ok(writer)
    }

    /// The record of `end`, a readable end of kind `kind` in the
    /// store, whoever holds it. This is the lookup Wasmtime makes of
    /// a host value's end before it lowers, pipes, closes, or
    /// converts the value, and the only check it makes there.
    ///
    /// Fails with the not-present cause when the store holds no
    /// readable end under `end`: both ends dropped, the host closed a
    /// stream or future it created, or `end` is what a value holds
    /// after its own close.
    pub fn readable_end(&self, end: EndId, kind: EndKind) -> Result<&CopyEnd> {
        self.end(end)
            .filter(|record| record.direction == EndDirection::Readable)
            .ok_or(Error::Copy(CopyCause::HostEndNotPresent { kind }))
    }

    /// The host moved `count` values through `host`, the end it
    /// serves, to or from the buffer of the guest's copy on the other
    /// end: that buffer records `count` more values of progress, and
    /// the copy completes. The completion's event reports the whole
    /// progress when it is delivered, as a guest's pending copy does.
    /// A consumer's poll records the items it takes as it takes them,
    /// so the write it serves completes with `count` zero.
    ///
    /// A copy being cancelled that moved nothing completes with the
    /// cancelled result instead, which a future's delivery reports as
    /// it is: the producer or consumer answered the cancel without the
    /// value, and the future can be copied again. A stream's
    /// completion is reported as cancelled at delivery anyway.
    pub fn finish_host_copy(&mut self, host: EndId, count: u32) -> Result<()> {
        let record = self.shared_record_at(self.end_record(host)?.shared)?;
        let kind = host_kind(record)?;
        let other = record.end_of(direction_of(counterpart(kind)));
        let record = self.end_record_mut(other)?;
        let cancelling = record.state == CopyState::Cancelling;
        let buffer = record
            .buffer
            .as_mut()
            .ok_or_else(|| Error::internal("a host end finished a copy that is not in progress"))?;
        buffer.progress += count;
        let result = if cancelling && buffer.progress == 0 {
            CopyResult::Cancelled
        } else {
            CopyResult::Completed
        };
        self.notify_copy(counterpart(kind), other, result)
    }

    /// Record that a poll of `reader`, the readable end the host
    /// serves through a consumer, took `count` more values out of the
    /// buffer of the guest's write on the writable end. The write goes
    /// on until the consumer answers ready, and the next poll's source
    /// starts after these values.
    pub fn record_host_take(&mut self, reader: EndId, count: u32) -> Result<()> {
        let writer = self
            .shared_record_at(self.end_record(reader)?.shared)?
            .writable;
        let buffer = self
            .end_record_mut(writer)?
            .buffer
            .as_mut()
            .ok_or_else(|| Error::internal("a host consumer took values from no write"))?;
        buffer.progress += count;
        Ok(())
    }

    /// Let go of `end`, an end the host serves or holds, once it will
    /// copy nothing more: a producer's stream ended or its future's
    /// value was delivered, a consumer's stream is over or its future's
    /// value was taken, the guest dropped the other end, or a pipe
    /// between two of the host's own ends finished. The end drops as a
    /// guest's end drops, and the rules of [`drop_end`](Self::drop_end)
    /// tell the other end. The end is done first, because a future's
    /// writable end that has not written would otherwise refuse to
    /// drop, and one the host lets go of never will write.
    ///
    /// The kind of the end follows from the kind of the end the host
    /// serves: `end` is that end, or the other end of the same stream
    /// or future.
    pub fn release_host_end(&mut self, end: EndId) -> Result<()> {
        let record = self.end_record(end)?;
        let direction = record.direction;
        let host = host_kind(self.shared_record_at(record.shared)?)?;
        let kind = if direction_of(host) == direction {
            host
        } else {
            counterpart(host)
        };
        self.end_record_mut(end)?.state = CopyState::Done;
        self.drop_end(kind, end)
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
    /// shared record and the end itself dropped and leaves both end
    /// records in the store; dropping the second removes the shared
    /// record and both end records.
    ///
    /// The first drop also tells the other end. A copy of it that is
    /// the pending side completes with the dropped result and the
    /// progress it made, which is the reference's `drop` of its shared
    /// stream. An other end that is idle and not done is given the
    /// dropped result with nothing moved, so a guest waiting on it in
    /// a set learns of the drop before it copies again. A stream end
    /// that holds the completed event of a copy not yet delivered has
    /// that event turned into the dropped result, and the event keeps
    /// the progress the copy made, because the count is read when the
    /// event is delivered. A future end keeps an event it holds, even
    /// as the pending side: the event reports that the one value
    /// moved, and a drop after it cannot undo that. Those rules are
    /// the ones the reference's current `End.drop`, `stream_event`,
    /// and `future_event` state and Wasmtime's `update_event` follows,
    /// where the reference the conformance corpus is drawn from tells
    /// a pending copy alone.
    ///
    /// A done end is not told, as the reference's current `End.drop`
    /// skips it. Wasmtime sets a dropped event on a done future end
    /// there, as the event of its waitable, so `waitable-set.wait` and
    /// `waitable-set.poll` deliver it when the end is in a set. The
    /// difference is observable: in such a set the polyfill reports no
    /// event for the end, where Wasmtime reports the dropped result.
    pub fn drop_end(&mut self, kind: EndKind, end: EndId) -> Result<()> {
        let record = self.end_record(end)?;
        if record.state.busy() {
            return Err(Error::Copy(CopyCause::BusyEnd { kind }));
        }
        if kind == EndKind::FutureWritable && record.state != CopyState::Done {
            return Err(Error::Copy(CopyCause::FutureWriteEndNotWritten));
        }
        let shared = record.shared;
        let direction = record.direction;
        self.leave_waitable_set(WaitableId::from_end(kind, end));
        let first = !self.shared_record_at(shared)?.dropped;
        if first {
            self.end_record_mut(end)?.dropped = true;
            let record = self
                .shared_records
                .get_mut(shared)
                .ok_or_else(|| Error::internal("shared record is not in the store"))?;
            record.dropped = true;
            let other = match direction {
                EndDirection::Readable => record.writable,
                EndDirection::Writable => record.readable,
            };
            let other_kind = counterpart(kind);
            let other_pending = record.pending.is_some_and(|pending| pending != direction);
            if other_pending {
                record.pending = None;
            }
            // A future end keeps the event it holds, pending side or
            // not: its copy completed with the one value, and the
            // drop that came after cannot take the value back.
            let other_record = self.end_record(other)?;
            if is_future(other_kind) && other_record.waitable.pending_event.is_some() {
                return Ok(());
            }
            if other_pending || other_record.state != CopyState::Done {
                return self.notify_copy(other_kind, other, CopyResult::Dropped);
            }
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
        let taken = self.waitable_state_mut(waitable)?.pending_event.take();
        match (waitable.end(), taken) {
            (Some((kind, end)), Some(event)) => self.deliver_copy_event(kind, end, event).map(Some),
            (_, taken) => Ok(taken),
        }
    }

    // ---- copies ----

    /// Start a copy on `end`, an end of kind `kind`, over `buffer`,
    /// and pair it with the other end of its stream or future. The
    /// rules are the reference's `SharedStreamImpl.read` and `write`,
    /// which its `SharedFutureImpl` narrows to one value:
    ///
    /// - A copy whose other end was dropped completes at once with the
    ///   dropped result.
    /// - The first copy with no pending side becomes the pending side.
    /// - A copy that finds the other end pending, started by the same
    ///   component instance, fails with the intra-instance cause
    ///   unless the payload is a number type or absent. That is the
    ///   reference's temporary rule, and it comes before any count is
    ///   looked at, so a copy that asks for nothing, or that finds
    ///   the pending side full, fails too. The end is left as it was.
    ///   An end the host serves, a producer's writable end or a
    ///   consumer's readable end, starts no copy and holds no buffer,
    ///   so it is never the pending side this compares.
    /// - A copy that finds the other end pending with room left moves
    ///   the smaller of the two remaining counts at once, which the
    ///   caller does with the [`Pairing::Move`] it is handed. A copy
    ///   that asked for nothing completes at once with nothing moved
    ///   instead, which makes it a readiness probe.
    /// - A copy that finds the pending side full completes it and
    ///   takes its place. A write that asked for nothing, against a
    ///   pending read that asked for nothing, completes at once
    ///   instead and leaves the read pending.
    ///
    /// A future's copy always asks for one value, so it never probes
    /// and never finds the pending side full: a copy that meets a
    /// pending one moves the value and completes both, and each end is
    /// done once its event is delivered.
    ///
    /// The end moves to `copying` and keeps the buffer until the event
    /// that reports its copy is delivered.
    pub fn start_copy(&mut self, kind: EndKind, end: EndId, buffer: CopyBuffer) -> Result<Pairing> {
        let (remain, zero_length) = (buffer.remain(), buffer.is_zero_length());
        let shared = self.end_record(end)?.shared;
        let record = self.shared_record_at(shared)?;
        if !record.dropped
            && !buffer.number_or_none
            && let Some(pending) = record.pending
            && let Some(other_buffer) = &self.end_record(record.end_of(pending))?.buffer
            && other_buffer.instance == buffer.instance
        {
            return Err(Error::Copy(CopyCause::IntraInstanceNonNumber));
        }
        let record = self.end_record_mut(end)?;
        record.state = CopyState::Copying;
        record.buffer = Some(buffer);
        let (shared, direction) = (record.shared, record.direction);
        let record = self.shared_record_at(shared)?;
        if record.dropped {
            self.notify_copy(kind, end, CopyResult::Dropped)?;
            return Ok(Pairing::Settled);
        }
        let Some(pending) = record.pending else {
            self.shared_record_at_mut(shared)?.pending = Some(direction);
            return Ok(Pairing::Settled);
        };
        if pending == direction {
            return Err(Error::internal(
                "an idle end's own direction is the pending side of its stream",
            ));
        }
        let (other, other_kind) = (record.end_of(pending), counterpart(kind));
        let other_buffer = self
            .end_record(other)?
            .buffer
            .as_ref()
            .ok_or_else(|| Error::internal("the pending side of a stream holds no buffer"))?;
        let (other_remain, other_zero_length) =
            (other_buffer.remain(), other_buffer.is_zero_length());
        if other_remain > 0 {
            if remain == 0 {
                self.notify_copy(kind, end, CopyResult::Completed)?;
                return Ok(Pairing::Settled);
            }
            let count = remain.min(other_remain);
            let (writer, reader) = match direction {
                EndDirection::Readable => (other, end),
                EndDirection::Writable => (end, other),
            };
            return Ok(Pairing::Move {
                writer,
                reader,
                count,
            });
        }
        if direction == EndDirection::Writable && zero_length && other_zero_length {
            self.notify_copy(kind, end, CopyResult::Completed)?;
            return Ok(Pairing::Settled);
        }
        self.shared_record_at_mut(shared)?.pending = Some(direction);
        self.notify_copy(other_kind, other, CopyResult::Completed)?;
        Ok(Pairing::Settled)
    }

    /// The values the [`Pairing::Move`] of a copy on `end`, an end of
    /// kind `kind`, named have moved: both buffers record `count` more
    /// values of progress. The copy on `end` completes. The pending
    /// copy completes too, but stays the pending side, and its buffer
    /// keeps taking values from later copies until its event is
    /// delivered, which then reports the total. That is the
    /// reference's reclaim rule.
    pub fn finish_move(&mut self, kind: EndKind, end: EndId, count: u32) -> Result<()> {
        let shared = self.end_record(end)?.shared;
        let record = self.shared_record_at(shared)?;
        let other = match direction_of(kind) {
            EndDirection::Readable => record.writable,
            EndDirection::Writable => record.readable,
        };
        for side in [end, other] {
            let buffer = self
                .end_record_mut(side)?
                .buffer
                .as_mut()
                .ok_or_else(|| Error::internal("a moving copy's end holds no buffer"))?;
            buffer.progress += count;
        }
        self.notify_copy(counterpart(kind), other, CopyResult::Completed)?;
        self.notify_copy(kind, end, CopyResult::Completed)
    }

    /// Cancel the copy in progress on `end`, an end of kind `kind`,
    /// which the reference's `cancel_copy` does once its checks pass.
    /// The end moves to `cancelling`. An end that already holds the
    /// event of its copy keeps it: the copy completed, or found the
    /// other end dropped, before the cancel, and the event reports the
    /// progress the copy made. A stream's completion is reported as
    /// cancelled once the end is `cancelling`, as delivery states, and
    /// a future's as completed.
    ///
    /// An end that holds no event and whose other end the host serves
    /// through a producer or a consumer waits on the host to answer
    /// the cancel. That is the reference's `End.cancel` when the other
    /// end has no owner, which may leave the event to the host, and
    /// Wasmtime's `cancel_read` against a host writer and
    /// `cancel_write` against a host reader, which tell the producer
    /// or consumer to finish and wake it. The end stays the pending
    /// side, and the end the host serves is answered, for the caller
    /// to wake: its next poll is asked to finish, and the copy then
    /// completes with the progress made, as cancelled on a stream, and
    /// as cancelled on a future that moved no value.
    ///
    /// Any other end that holds no event is the pending side of its
    /// stream or future. It stops being it and is given the cancelled
    /// result, which is the reference's `cancel` of its shared record.
    /// Its buffer is given back when the event is delivered, and the
    /// event then reports the progress made so far.
    ///
    /// Afterwards the end holds an event unless its copy waits on the
    /// host. The caller then waits for the event or reports the copy
    /// blocked. A second cancel while the first waits finds the end
    /// `cancelling` and traps, as the reference's `cancel_copy` does.
    /// Wasmtime allows it and waits again; the polyfill keeps the
    /// reference's trap.
    pub fn cancel_copy(&mut self, kind: EndKind, end: EndId) -> Result<Option<EndId>> {
        let host_end = self.host_counterpart(end);
        let record = self.end_record_mut(end)?;
        record.state = CopyState::Cancelling;
        if record.waitable.pending_event.is_some() {
            return Ok(None);
        }
        if host_end.is_some() {
            return Ok(host_end);
        }
        let (shared, direction) = (record.shared, record.direction);
        let record = self.shared_record_at_mut(shared)?;
        if record.pending != Some(direction) {
            return Err(Error::internal(
                "a copying end with no event is neither the pending side nor a host end's reader",
            ));
        }
        record.pending = None;
        self.notify_copy(kind, end, CopyResult::Cancelled)
            .map(|()| None)
    }

    /// Give `end`, an end of kind `kind`, the event of a finished
    /// copy with `result`. The event carries the end's index; the
    /// count the copy moved is read when the event is delivered,
    /// because a pending copy's buffer can take more values between
    /// now and then.
    fn notify_copy(&mut self, kind: EndKind, end: EndId, result: CopyResult) -> Result<()> {
        let handle = self.end_record(end)?.handle.unwrap_or(0);
        self.set_pending_event(
            WaitableId::from_end(kind, end),
            Event::copy(event_code(kind), handle, result.pack(0)),
        )
    }

    /// Finish the delivery of `event`, just taken from `end`, an end
    /// of kind `kind`: what the reference's `stream_event` and
    /// `future_event` do when a copy's event is taken. The count the
    /// copy moved goes into a stream's packed result, where a future's
    /// always counts zero, the buffer is given back, and the end moves
    /// on: to `done` after a dropped result, which the end records it
    /// was told, and for a future after a completed one too, and to
    /// `idle` otherwise. An end still the pending side of its stream
    /// or future stops being it, which is the reclaim of its buffer.
    /// The index is the one the end has now.
    ///
    /// A stream end whose copy is being cancelled reports a completed
    /// copy as cancelled, with the same progress: the cancel ended the
    /// copy, whatever it moved first. A future end reports its
    /// completion as it is, because its one value moved. Both are what
    /// Wasmtime's cancel returns and what the reference's current
    /// `stream_event` and `future_event` deliver.
    fn deliver_copy_event(&mut self, kind: EndKind, end: EndId, event: Event) -> Result<Event> {
        let [index, packed] = event.payloads();
        let record = self.end_record_mut(end)?;
        let result = match CopyResult::from_packed(packed) {
            Some(CopyResult::Completed)
                if !is_future(kind) && record.state == CopyState::Cancelling =>
            {
                Some(CopyResult::Cancelled)
            }
            result => result,
        };
        let packed = match (record.buffer.take(), result) {
            (Some(_), Some(result)) if is_future(kind) => result.pack(0),
            (Some(buffer), Some(result)) => result.pack(buffer.progress),
            _ => packed,
        };
        record.state = match result {
            Some(CopyResult::Dropped) => CopyState::Done,
            Some(CopyResult::Completed) if is_future(kind) => CopyState::Done,
            _ => CopyState::Idle,
        };
        if result == Some(CopyResult::Dropped) {
            record.notified_dropped = true;
        }
        let index = record.handle.unwrap_or(index);
        let (shared, direction) = (record.shared, record.direction);
        if let Some(record) = self.shared_records.get_mut(shared)
            && record.pending == Some(direction)
        {
            record.pending = None;
        }
        Ok(Event::copy(event.code(), index, packed))
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

    /// One end record, mutably.
    fn end_record_mut(&mut self, end: EndId) -> Result<&mut CopyEnd> {
        self.end_mut(end)
            .ok_or_else(|| Error::internal("end record is not in the store"))
    }

    /// The shared record at `index`, or the internal error when there
    /// is none.
    fn shared_record_at(&self, index: u32) -> Result<&SharedRecord> {
        self.shared_records
            .get(index)
            .ok_or_else(|| Error::internal("shared record is not in the store"))
    }

    /// The shared record at `index`, mutably.
    fn shared_record_at_mut(&mut self, index: u32) -> Result<&mut SharedRecord> {
        self.shared_records
            .get_mut(index)
            .ok_or_else(|| Error::internal("shared record is not in the store"))
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

/// The kind of the other end of the stream or future an end of `kind`
/// belongs to.
fn counterpart(kind: EndKind) -> EndKind {
    match kind {
        EndKind::StreamReadable => EndKind::StreamWritable,
        EndKind::StreamWritable => EndKind::StreamReadable,
        EndKind::FutureReadable => EndKind::FutureWritable,
        EndKind::FutureWritable => EndKind::FutureReadable,
    }
}

/// The kind of the end the host serves on `record`, or the internal
/// error when the host serves neither.
fn host_kind(record: &SharedRecord) -> Result<EndKind> {
    record
        .host
        .ok_or_else(|| Error::internal("a host end's stream or future was not made by the host"))
}

/// Which way the values move through an end of `kind`.
fn direction_of(kind: EndKind) -> EndDirection {
    match kind {
        EndKind::StreamReadable | EndKind::FutureReadable => EndDirection::Readable,
        EndKind::StreamWritable | EndKind::FutureWritable => EndDirection::Writable,
    }
}

/// Whether an end of `kind` belongs to a future rather than a stream.
fn is_future(kind: EndKind) -> bool {
    matches!(kind, EndKind::FutureReadable | EndKind::FutureWritable)
}

/// The code of the event a copy on an end of `kind` delivers.
fn event_code(kind: EndKind) -> EventCode {
    match kind {
        EndKind::StreamReadable => EventCode::StreamRead,
        EndKind::StreamWritable => EventCode::StreamWrite,
        EndKind::FutureReadable => EventCode::FutureRead,
        EndKind::FutureWritable => EventCode::FutureWrite,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    fn it_keeps_the_scope_under_a_nested_start_mark_current() {
        // The mark sits between a caller's task and the callee's.
        // Before the callee pushes its task, the caller's task is
        // still what borrows and lends count against.
        let mut tables = TaskTables::new();
        let instance = tables.insert_instance();
        let caller = tables.push_task(None, None, instance);
        let subtask = tables.insert_subtask();
        tables.begin_nested_start(subtask, LowerKind::Async);

        assert_eq!(tables.current_scope(), Some(Scope::Task(caller)));
        assert_eq!(tables.current_task(), Some(caller));

        let callee = tables.push_task(None, None, instance);
        assert_eq!(tables.current_task(), Some(callee));
        assert_eq!(
            tables.scopes(),
            &[
                Scope::Task(caller),
                Scope::NestedStart {
                    subtask,
                    lower: LowerKind::Async
                },
                Scope::Task(callee)
            ]
        );
    }

    #[wcmp_macros::test]
    fn it_counts_an_asynchronous_lowers_caller_as_one_that_goes_on() {
        // The lower answers with the status word, so the caller's own
        // code runs on whatever the callee has done.
        let mut tables = TaskTables::new();
        assert!(!tables.caller_below_goes_on());
        let subtask = tables.insert_subtask();
        tables.begin_nested_start(subtask, LowerKind::Async);
        assert!(tables.caller_below_goes_on());
    }

    #[wcmp_macros::test]
    fn it_counts_a_synchronous_lowers_caller_as_one_that_goes_on_once_the_callee_resolved() {
        // Before the callee resolves, the caller would only wait for
        // it. Once it has, the lower returns the result and the
        // caller's own code goes on.
        let mut tables = TaskTables::new();
        let subtask = tables.insert_subtask();
        tables.begin_nested_start(subtask, LowerKind::Sync);
        assert!(!tables.caller_below_goes_on());

        tables
            .subtask_returned(subtask)
            .expect("the callee returns");
        assert!(tables.caller_below_goes_on());
    }

    #[wcmp_macros::test]
    fn it_counts_a_thread_switch_as_one_that_goes_on_while_the_switching_thread_is_not_suspended() {
        // A thread that yielded to the thread it started would go on
        // once control came back to it. One that suspended would not,
        // until a resume made it ready again.
        let mut tables = TaskTables::new();
        let instance = tables.insert_instance();
        let task = tables.push_task(None, None, instance);
        tables.start_task(task);
        let thread = tables.current_thread().expect("the task's implicit thread");

        tables.begin_thread_switch(thread);
        assert_eq!(tables.current_scope(), Some(Scope::Task(task)));
        assert!(tables.caller_below_goes_on());

        tables.suspend_thread(thread).expect("the thread suspends");
        assert!(!tables.caller_below_goes_on());

        assert!(
            !tables
                .resume_later(thread)
                .expect("the thread is suspended"),
            "the thread has run, so it has no start to queue"
        );
        assert!(tables.caller_below_goes_on());

        tables.end_thread_switch();
        assert_eq!(tables.scopes(), &[Scope::Task(task)]);
        assert!(!tables.caller_below_goes_on());
    }

    #[wcmp_macros::test]
    fn it_counts_an_asynchronous_lower_below_a_synchronous_one() {
        // A frame further down that would go on has a mark of its own.
        let mut tables = TaskTables::new();
        let outer = tables.insert_subtask();
        let inner = tables.insert_subtask();
        tables.begin_nested_start(outer, LowerKind::Async);
        tables.begin_nested_start(inner, LowerKind::Sync);
        assert!(tables.caller_below_goes_on());
    }

    #[wcmp_macros::test]
    fn it_takes_off_the_innermost_mark_and_leaves_what_a_failure_stranded_above_it() {
        // A callee that failed without popping its scope leaves it
        // above the mark. Ending the nested start takes the mark
        // alone, so the unwind that owns the stranded scope still
        // finds it.
        let mut tables = TaskTables::new();
        let instance = tables.insert_instance();
        let caller = tables.push_task(None, None, instance);
        let outer = tables.insert_subtask();
        let inner = tables.insert_subtask();
        tables.begin_nested_start(outer, LowerKind::Async);
        tables.begin_nested_start(inner, LowerKind::Sync);
        let stranded = tables.push_task(None, None, instance);

        tables.end_nested_start();
        assert_eq!(
            tables.scopes(),
            &[
                Scope::Task(caller),
                Scope::NestedStart {
                    subtask: outer,
                    lower: LowerKind::Async
                },
                Scope::Task(stranded)
            ]
        );
        tables.end_nested_start();
        assert!(!tables.caller_below_goes_on());
        assert_eq!(
            tables.scopes(),
            &[Scope::Task(caller), Scope::Task(stranded)]
        );
        tables.end_nested_start();
        assert_eq!(
            tables.scopes().len(),
            2,
            "a mark that is gone leaves nothing to take"
        );
    }

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

    /// The buffer of a copy of `length` values on a stream that
    /// carries none, under canon options that name no runtime slot.
    fn empty_buffer(length: u32) -> CopyBuffer {
        use crate::abi::runtime_state::AbiRuntimeState;
        use crate::executor::ir::{DataModel, StringEncoding};
        use std::sync::Mutex;
        CopyBuffer {
            payload: None,
            options: Arc::new(CanonOptions {
                instance: 0,
                memory: None,
                realloc: None,
                post_return: None,
                async_: true,
                callback: None,
                string_encoding: StringEncoding::Utf8,
                data_model: DataModel::LinearMemory,
            }),
            abi_state: Arc::new(Mutex::new(AbiRuntimeState::with_slabs(
                0,
                0,
                0,
                0,
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ))),
            instance: InstanceId::from_index(0),
            number_or_none: true,
            pointer: 0,
            length,
            progress: 0,
        }
    }

    /// The buffer of a copy of `length` values of a payload that is
    /// not a number, started by the instance at `instance`.
    fn text_buffer(instance: u32, length: u32) -> CopyBuffer {
        CopyBuffer {
            payload: Some(ValueType::Primitive(crate::types::PrimitiveType::String)),
            instance: InstanceId::from_index(instance),
            number_or_none: false,
            ..empty_buffer(length)
        }
    }

    #[wcmp_macros::test]
    fn it_refuses_a_same_instance_copy_of_a_non_number_payload_whatever_the_counts() {
        for (pending, pending_length, arriving, arriving_length) in [
            (EndKind::StreamReadable, 1, EndKind::StreamWritable, 0),
            (EndKind::StreamWritable, 1, EndKind::StreamReadable, 0),
            (EndKind::StreamReadable, 0, EndKind::StreamWritable, 1),
            (EndKind::StreamReadable, 2, EndKind::StreamWritable, 2),
        ] {
            let mut tables = TaskTables::new();
            let (readable, writable) = tables.insert_ends(None);
            let end_of = |kind| match kind {
                EndKind::StreamReadable => readable,
                _ => writable,
            };
            assert_eq!(
                tables
                    .start_copy(pending, end_of(pending), text_buffer(3, pending_length))
                    .expect("the first copy starts"),
                Pairing::Settled
            );
            let refused = tables
                .start_copy(arriving, end_of(arriving), text_buffer(3, arriving_length))
                .expect_err("a copy from the pending side's instance is refused");
            assert!(
                matches!(refused, Error::Copy(CopyCause::IntraInstanceNonNumber)),
                "{arriving:?} of {arriving_length} against {pending:?} of {pending_length}: \
                 {refused:?}"
            );
            let record = tables.end(end_of(arriving)).expect("the arriving end");
            assert!(
                record.state == CopyState::Idle && record.buffer.is_none(),
                "the refused copy left its end as it was"
            );
        }
    }

    #[wcmp_macros::test]
    fn it_lets_a_zero_length_copy_of_a_non_number_payload_probe_another_instance() {
        let mut tables = TaskTables::new();
        let (readable, writable) = tables.insert_ends(None);
        tables
            .start_copy(EndKind::StreamReadable, readable, text_buffer(3, 1))
            .expect("the read starts");
        assert_eq!(
            tables
                .start_copy(EndKind::StreamWritable, writable, text_buffer(4, 0))
                .expect("a write from another instance probes"),
            Pairing::Settled
        );
    }

    #[wcmp_macros::test]
    fn it_lets_a_same_instance_copy_of_a_number_payload_meet_a_pending_one() {
        let mut tables = TaskTables::new();
        let (readable, writable) = tables.insert_ends(None);
        tables
            .start_copy(EndKind::StreamReadable, readable, empty_buffer(1))
            .expect("the read starts");
        assert_eq!(
            tables
                .start_copy(EndKind::StreamWritable, writable, empty_buffer(0))
                .expect("a zero-length write of the same instance probes"),
            Pairing::Settled
        );
    }

    #[wcmp_macros::test]
    fn it_leaves_a_guest_copy_against_a_host_served_end_to_the_host_whatever_the_payload() {
        // The end the host serves, a producer's writable end or a
        // consumer's readable end, never starts a copy, so it is never
        // the pending side and never holds a buffer. A guest's copy
        // against it therefore finds no pending side: it is not
        // paired, so it takes neither the byte path nor the path
        // through values, and the same-instance rule, which compares
        // two buffers, has nothing to compare. The host's producer or
        // consumer serves it instead.
        for (host, guest) in [
            (EndKind::StreamWritable, EndKind::StreamReadable),
            (EndKind::StreamReadable, EndKind::StreamWritable),
        ] {
            for buffer in [empty_buffer(4), text_buffer(3, 0), text_buffer(3, 4)] {
                let mut tables = TaskTables::new();
                let (readable, writable) = tables.insert_host_ends(buffer.payload.clone(), host);
                let (host_end, guest_end) = match host {
                    EndKind::StreamWritable => (writable, readable),
                    _ => (readable, writable),
                };
                assert_eq!(
                    tables
                        .start_copy(guest, guest_end, buffer)
                        .expect("the guest's copy starts"),
                    Pairing::Settled
                );
                assert_eq!(tables.host_counterpart(guest_end), Some(host_end));
                assert!(
                    tables.end(host_end).is_some_and(|end| end.buffer.is_none()),
                    "the host's end holds no buffer"
                );
                let shared = tables.end(guest_end).expect("the guest's end").shared;
                assert_eq!(
                    tables
                        .shared_records
                        .get(shared)
                        .expect("the record")
                        .pending,
                    Some(direction_of(guest)),
                    "the guest's copy is the pending side"
                );
            }
        }
    }

    #[wcmp_macros::test]
    fn it_turns_the_undelivered_completion_of_an_end_that_is_not_pending_into_a_drop() {
        // A full pending read is completed and replaced as the pending
        // side by a write. The write then leaves the copy, as a cancel
        // of it will, so the writable end is idle while the read still
        // holds its completed event. That is the one shape in which
        // the end a drop reaches holds an event without being the
        // pending side.
        let mut tables = TaskTables::new();
        let (readable, writable) = tables.insert_ends(None);
        let read = WaitableId::from_end(EndKind::StreamReadable, readable);
        let write = WaitableId::from_end(EndKind::StreamWritable, writable);
        assert_eq!(
            tables
                .start_copy(EndKind::StreamReadable, readable, empty_buffer(2))
                .expect("the read starts"),
            Pairing::Settled
        );
        let pairing = tables
            .start_copy(EndKind::StreamWritable, writable, empty_buffer(2))
            .expect("the write starts");
        assert_eq!(
            pairing,
            Pairing::Move {
                writer: writable,
                reader: readable,
                count: 2
            }
        );
        tables
            .finish_move(EndKind::StreamWritable, writable, 2)
            .expect("the move finishes");
        tables
            .take_pending_event(write)
            .expect("the write's event")
            .expect("the write completed");
        tables
            .start_copy(EndKind::StreamWritable, writable, empty_buffer(3))
            .expect("a second write replaces the full read");
        let shared = tables.end(writable).expect("the writable end").shared;
        tables
            .shared_records
            .get_mut(shared)
            .expect("the record")
            .pending = None;
        let record = tables.end_record_mut(writable).expect("the writable end");
        record.state = CopyState::Idle;
        record.buffer = None;

        tables
            .drop_end(EndKind::StreamWritable, writable)
            .expect("the idle writable end drops");

        let event = tables
            .take_pending_event(read)
            .expect("the read's event")
            .expect("the read holds an event");
        assert_eq!(
            event.payloads()[1],
            CopyResult::Dropped.pack(2),
            "the completion became the dropped result and kept the two it moved"
        );
        assert_eq!(
            tables.end(readable).map(|end| end.state),
            Some(CopyState::Done)
        );
    }
}
