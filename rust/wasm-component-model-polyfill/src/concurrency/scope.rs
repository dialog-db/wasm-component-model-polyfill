//! One entry of the store's stack of current scopes.

use super::subtask_id::SubtaskId;
use super::task_id::TaskId;

/// One entry of the store's stack of current scopes.
///
/// The top of the stack is the current scope. A host call into an
/// export pushes the export's task; a guest call into a host
/// function pushes the subtask of that call, which stays on the
/// stack while the host side runs; an adapter's enter intrinsic
/// pushes the callee's task and its exit intrinsic pops it. The
/// stack is a stack because synchronous calls nest on the one real
/// stack.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scope {
    /// A call into an export.
    Task(TaskId),
    /// A call out through an import.
    Subtask(SubtaskId),
}
