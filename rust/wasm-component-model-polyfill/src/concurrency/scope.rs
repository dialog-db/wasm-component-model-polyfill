//! One entry of the store's stack of current scopes.

use super::lower_kind::LowerKind;
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
///
/// A start intrinsic that runs an `async`-typed callee from inside
/// its own frame pushes a [`NestedStart`](Self::NestedStart) mark
/// under the callee's task and takes it off when the callee returns
/// to it. The mark is not a scope: nothing counts a borrow or a lend
/// against it, and the current scope is the innermost entry that is
/// a task or a subtask. It records where the real stack holds a
/// frame that a stack switch would return to, which is what the
/// cause of a failed block reads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scope {
    /// A call into an export.
    Task(TaskId),
    /// A call out through an import.
    Subtask(SubtaskId),
    /// A thread a trampoline started from inside itself, on the real
    /// stack above the trampoline's frame. The reference runs that
    /// thread on a stack of its own and returns to the trampoline
    /// when it suspends. Without a stack switch, everything below
    /// the mark stays where it is until the thread returns.
    ///
    /// What the caller would do once control came back depends on
    /// the call. After an asynchronous lower the caller's own code
    /// goes on. After a synchronous lower the caller waits for the
    /// callee's result, and its own code goes on only once the
    /// callee has resolved.
    NestedStart {
        /// The caller's record of the call.
        subtask: SubtaskId,
        /// How the caller lowered the call.
        lower: LowerKind,
    },
}
