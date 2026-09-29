//! One call into an export.

use std::sync::Arc;

use crate::abi::signature::Signature;
use crate::error::Result;
use crate::executor::ir::CanonOptions;
use crate::resource::TableId;
use crate::value::Val;

use super::instance_id::InstanceId;
use super::subtask_id::SubtaskId;
use super::task_result::TaskResult;
use super::task_state::TaskState;
use super::thread_id::ThreadId;

/// The record of one call into an export.
///
/// A task is the scope a borrow lowered into a guest is owed to: the
/// lowering raises `num_borrows`, the guest's drop lowers it, and the
/// return of a synchronous export traps while the count is above
/// zero. It is also the record of two kinds of call, and so the
/// scope their lends go on: a call from the host into an export,
/// whose lends come back when the task resolves, and a call between
/// two components the enter and exit intrinsics carry alone, whose
/// lends come back at the task's exit. `HandleTables::lend_to`
/// states the rule both follow.
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
    pub function: Option<Arc<Signature>>,
    /// The canon options of the export's lift, under the same rule
    /// as `function`.
    pub options: Option<Arc<CanonOptions>>,
    /// The component instance the export belongs to. `None` for the
    /// one task that belongs to no component instance: the
    /// destructor of a resource the host implements, which the host
    /// releases with no guest in sight. The reference gives every
    /// resource type an implementing component instance and lifts
    /// the destructor there; a resource the host registered has none.
    pub instance: Option<InstanceId>,
    /// How far the call has got.
    pub state: TaskState,
    /// Whether a cancellation request was delivered to the task. It
    /// stays set once the task resolves, which is what tells a
    /// second resolution of a cancelled task from a `task.cancel` in
    /// a task that was never cancelled.
    pub cancel_delivered: bool,
    /// The borrows the task received and has not yet seen dropped.
    /// The reference names this count `num_borrows`.
    pub num_borrows: u32,
    /// The task's implicit thread, created with the record.
    pub implicit_thread: ThreadId,
    /// Every thread the task contains, its implicit thread first.
    pub threads: Vec<ThreadId>,
    /// The owning handle-table entries lent to this task, as
    /// `(table, index)`. Each lend is undone when the task
    /// resolves, and again at the task's scope exit for a task that
    /// never resolved.
    pub lenders: Vec<(TableId, u32)>,
    /// Where the task's result goes.
    pub result: TaskResult,
    /// The subtask of the call this task is the callee of, for a
    /// call between two components the prepare intrinsic set up.
    /// The callee's `task.return` reaches the return function of
    /// the call through it. `None` for a call from the host and for
    /// the task an adapter's enter intrinsic pushes.
    pub subtask: Option<SubtaskId>,
    /// Whether the task has ended with its last thread. A task of a
    /// call the caller still holds a subtask entry for outlives its
    /// threads: the entry names the record, so the record stays in
    /// the store until `subtask.drop` takes the entry away. The flag
    /// is what the removal of the entry reads to know the record has
    /// nothing left to run.
    pub thread_exited: bool,
    /// Whether the task's implicit thread has exited while another
    /// thread of the task was still there. The task goes on with its
    /// explicit threads, which is the reference's rule that a task
    /// ends only when its last thread does, and the end of the last
    /// of them reads this flag to know that it ends the task.
    pub implicit_thread_exited: bool,
    /// The interned index of the result tuple the callee's lift
    /// declared, as the adapter names it at run time. A prepared
    /// call has no projected function type, so this is what the
    /// callee's `task.return` compares its own declared type
    /// against, which is the comparison Wasmtime makes for the same
    /// call. `None` for every other task, whose function type is
    /// known and compared structurally.
    pub result_tuple: Option<usize>,
}

impl Task {
    /// Construct a task in its initial state, running on
    /// `implicit_thread`.
    pub fn new(
        function: Option<Arc<Signature>>,
        options: Option<Arc<CanonOptions>>,
        instance: Option<InstanceId>,
        implicit_thread: ThreadId,
    ) -> Self {
        Self {
            function,
            options,
            instance,
            state: TaskState::Initial,
            cancel_delivered: false,
            num_borrows: 0,
            implicit_thread,
            threads: vec![implicit_thread],
            lenders: Vec::new(),
            result: TaskResult::Pending,
            subtask: None,
            thread_exited: false,
            implicit_thread_exited: false,
            result_tuple: None,
        }
    }

    /// Resolve the task with `result`: send it through the caller's
    /// channel when the task was given one, and otherwise hold it in
    /// the record for the caller on the stack.
    ///
    /// Sending through the channel wakes the caller. The caller is
    /// not on the stack — that is what the channel is for — and its
    /// future can be one a host combinator polls again only after its
    /// waker fires, so a send that did not wake would leave the call
    /// pending against the result it is waiting for. A channel whose
    /// lock a panic poisoned fails the resolution.
    pub fn resolve(&mut self, result: Option<Val>) -> Result<()> {
        self.state = TaskState::Resolved;
        match &self.result {
            TaskResult::Channel(slot) => slot.fill(result),
            _ => {
                self.result = TaskResult::Returned(result);
                Ok(())
            }
        }
    }
}
