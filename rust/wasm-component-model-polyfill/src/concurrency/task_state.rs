//! The state of a task record.

/// The state of a task: one call into an export.
///
/// A task of a synchronous export moves straight through
/// [`TaskState::Initial`] and [`TaskState::Started`] to
/// [`TaskState::Resolved`], because its core function runs to
/// completion inside the call that started it. The two cancellation
/// states exist for the tasks of `async` exports, which can be asked
/// to cancel between the two.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskState {
    /// The record exists and the task's thread has not run yet.
    Initial,
    /// The task's thread is running or has run.
    Started,
    /// Cancellation was requested and has not been delivered to the
    /// task yet. Reserved for the tasks of `async` exports; a
    /// synchronous task never reaches it.
    #[allow(dead_code)]
    PendingCancel,
    /// Cancellation was delivered and the task has not resolved yet.
    /// Reserved for the tasks of `async` exports; a synchronous task
    /// never reaches it.
    #[allow(dead_code)]
    CancelDelivered,
    /// The task returned its result, or was cancelled.
    Resolved,
}
