//! The state of a task record.

/// The state of a task: one call into an export.
///
/// A task of a synchronous export moves straight through
/// [`TaskState::Initial`] and [`TaskState::Started`] to
/// [`TaskState::Resolved`], because its core function runs to
/// completion inside the call that started it. The two cancellation
/// states are reached only through `subtask.cancel` of the call the
/// task serves, between the start and the resolution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskState {
    /// The record exists and the task's thread has not run yet.
    Initial,
    /// The task's thread is running or has run.
    Started,
    /// Cancellation was requested and has not been delivered to the
    /// task yet.
    PendingCancel,
    /// Cancellation was delivered and the task has not resolved yet.
    /// The task may confirm it with `task.cancel`, or return a result
    /// all the same.
    CancelDelivered,
    /// The task returned its result, or was cancelled.
    Resolved,
}
