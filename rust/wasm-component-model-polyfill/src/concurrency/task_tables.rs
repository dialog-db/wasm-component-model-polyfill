//! The store's tables of task, subtask, thread, and instance
//! records, with the stack of current scopes.

use crate::component::FunctionType;
use crate::executor::ir::CanonOptions;
use crate::resource::TableId;

use super::instance_id::InstanceId;
use super::instance_record::InstanceRecord;
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

/// The store's tables of task, subtask, thread, and instance
/// records, with the stack of current scopes.
///
/// A scope is a task record or a subtask record, and the top of the
/// stack is the current scope. Every borrow operation consults it: a
/// borrow lowered into a guest counts against the current task, a
/// borrow lifted out of an owning handle is lent to the current
/// scope, and the scope's exit checks that the guest dropped what it
/// was lent.
pub struct TaskTables {
    tasks: RecordTable<Task>,
    subtasks: RecordTable<Subtask>,
    threads: RecordTable<Thread>,
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

    /// Create a task for a call into an export of `instance` and
    /// push it as the current scope. The task's implicit thread is
    /// created with it.
    pub fn push_task(
        &mut self,
        function: Option<FunctionType>,
        options: Option<CanonOptions>,
        instance: InstanceId,
    ) -> TaskId {
        let task = TaskId::from_index(self.tasks.next_index());
        let thread = ThreadId::from_index(self.threads.insert(Thread::new(task)));
        self.tasks
            .insert(Task::new(function, options, instance, thread));
        self.scopes.push(Scope::Task(task));
        task
    }

    /// Create a subtask for a call out through an import and push it
    /// as the current scope.
    pub fn push_subtask(&mut self) -> SubtaskId {
        let subtask = SubtaskId::from_index(self.subtasks.insert(Subtask::new()));
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

    /// The current task: the innermost task on the stack. A subtask
    /// on top of it does not displace it, because a borrow lowered
    /// into a guest while a host call runs is still owed to the task
    /// that made the call.
    pub fn current_task(&self) -> Option<TaskId> {
        self.scopes.iter().rev().find_map(|scope| match scope {
            Scope::Task(task) => Some(*task),
            Scope::Subtask(_) => None,
        })
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

    /// One task record.
    pub fn task(&self, task: TaskId) -> Option<&Task> {
        self.tasks.get(task.index())
    }

    /// One task record, mutably.
    pub fn task_mut(&mut self, task: TaskId) -> Option<&mut Task> {
        self.tasks.get_mut(task.index())
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
        let record = self.tasks.remove(task.index())?;
        for thread in &record.threads {
            self.threads.remove(thread.index());
        }
        Some(record)
    }

    /// Remove a subtask record.
    pub fn remove_subtask(&mut self, subtask: SubtaskId) -> Option<Subtask> {
        self.subtasks.remove(subtask.index())
    }
}

impl Default for TaskTables {
    fn default() -> Self {
        Self::new()
    }
}
