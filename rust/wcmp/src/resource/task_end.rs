//! What one attempt to end a task achieved.
//!
//! A task is ended by name, and the name is not a promise: the
//! store's records refuse to end a task whose scope is not on the
//! stack, and there is no current task to end when the stack is
//! empty. [`TaskEnd`] is the answer, and its caller needs it because
//! ending a task has a second half that lives outside these records
//! — the scheduler's sweep of whatever the task still has queued.

/// What ending a task achieved: whether the task ended at all, and
/// what the exit's borrow check found when it did.
///
/// The distinction is load-bearing. The scheduler's sweep of a
/// task's queued items is the second half of ending it, so it must
/// run when the task ended and only then: an item that names a task
/// whose record is still live is that task's pending work, and
/// dropping it would take the work of a task that is still to run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskEnd {
    /// The task ended: its scope was popped, the lends recorded
    /// against it were undone, and its last thread ended. The
    /// count is what the borrow check found — the borrows the guest
    /// was lowered and did not drop — and is zero on a path that
    /// makes no such check.
    ///
    /// The task's record is gone with its threads, save in the one
    /// case the records keep it: a guest callee whose caller still
    /// holds a subtask entry leaves a record behind for that entry
    /// to name, and `subtask.drop` is what finally takes it. Such a
    /// task has ended all the same — its threads ended — so the
    /// sweep belongs to it too.
    Ended(u32),
    /// Nothing ended: the task named was not one this end could take
    /// off the stack, so its record, its threads, and everything it
    /// has queued are untouched.
    Untouched,
}

impl TaskEnd {
    /// Whether the task ended, which is what says the sweep of its
    /// queued items must run.
    pub fn ended(&self) -> bool {
        matches!(self, Self::Ended(_))
    }

    /// What the exit's borrow check found: `Err(count)` when the
    /// guest was lowered `count` borrows during the task and dropped
    /// none of them, which is the trap the reference raises. A task
    /// that did not end owes nothing, because nothing of it was
    /// checked.
    pub fn borrows(&self) -> Result<(), u32> {
        match self {
            Self::Ended(count) if *count > 0 => Err(*count),
            _ => Ok(()),
        }
    }
}
