//! One call into an export.

use crate::component::FunctionType;
use crate::executor::ir::CanonOptions;
use crate::resource::TableId;
use crate::value::Val;

use super::instance_id::InstanceId;
use super::task_result::TaskResult;
use super::task_state::TaskState;
use super::thread_id::ThreadId;

/// The record of one call into an export.
///
/// A task is the scope a borrow lowered into a guest is owed to: the
/// lowering raises `num_borrows`, the guest's drop lowers it, and the
/// return of a synchronous export traps while the count is above
/// zero. It is also the scope a borrow lifted out of an owning handle
/// is lent to while a call between two components runs, which is
/// where Wasmtime keeps the same list.
///
/// A synchronous call is a task with one thread: the implicit thread
/// runs the callee's core function to its return on the one real
/// stack.
pub struct Task {
    /// The function type of the export the task is a call into.
    /// `None` for the task an adapter's enter intrinsic pushes: the
    /// adapter passes no type, and the task exists only so the
    /// intrinsics the callee reaches have a scope, which is what
    /// Wasmtime's own sync-call task carries.
    pub function: Option<FunctionType>,
    /// The canon options of the export's lift, under the same rule
    /// as `function`.
    pub options: Option<CanonOptions>,
    /// The component instance the export belongs to.
    pub instance: InstanceId,
    /// How far the call has got.
    pub state: TaskState,
    /// The borrows the task received and has not yet seen dropped.
    /// The reference names this count `num_borrows`.
    pub num_borrows: u32,
    /// The task's implicit thread, created with the record.
    pub implicit_thread: ThreadId,
    /// Every thread the task contains, its implicit thread first.
    pub threads: Vec<ThreadId>,
    /// The owning handle-table entries lent to this task, as
    /// `(table, index)`. Each lend is undone when the task's scope
    /// ends.
    pub lenders: Vec<(TableId, u32)>,
    /// Where the task's result goes.
    pub result: TaskResult,
}

impl Task {
    /// Construct a task in its initial state, running on
    /// `implicit_thread`.
    pub fn new(
        function: Option<FunctionType>,
        options: Option<CanonOptions>,
        instance: InstanceId,
        implicit_thread: ThreadId,
    ) -> Self {
        Self {
            function,
            options,
            instance,
            state: TaskState::Initial,
            num_borrows: 0,
            implicit_thread,
            threads: vec![implicit_thread],
            lenders: Vec::new(),
            result: TaskResult::Pending,
        }
    }

    /// Resolve the task with `result`: send it through the caller's
    /// channel when the task was given one, and otherwise hold it in
    /// the record for the caller on the stack.
    pub fn resolve(&mut self, result: Option<Val>) {
        self.state = TaskState::Resolved;
        match &self.result {
            TaskResult::Channel(slot) => {
                if let Ok(mut slot) = slot.lock() {
                    *slot = Some(result);
                }
            }
            _ => self.result = TaskResult::Returned(result),
        }
    }
}
