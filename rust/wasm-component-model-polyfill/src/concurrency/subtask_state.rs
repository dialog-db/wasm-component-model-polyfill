//! The state of a subtask record.

/// The state of a subtask: one call out through an import.
///
/// The last three states are the resolved ones: the call produced a
/// result or was cancelled, and its lenders can be released.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubtaskState {
    /// The call was made and its parameters have not been lifted
    /// yet.
    Starting,
    /// The parameters were lifted and the callee is running.
    Started,
    /// The callee returned its result.
    Returned,
    /// The call was cancelled before the callee read its parameters:
    /// the cancellation built-ins reach this state, and so does a
    /// call abandoned while lifting its parameters.
    CancelledBeforeStarted,
    /// The call was cancelled after the callee read its parameters
    /// and before it returned: the cancellation built-ins reach this
    /// state, and so does a call abandoned once the host side has
    /// begun.
    CancelledBeforeReturned,
}

impl SubtaskState {
    /// Whether the call has produced its outcome, by returning or by
    /// being cancelled. A resolved subtask can deliver its
    /// resolution, which releases the handles it borrowed. Nothing
    /// asks yet: a synchronous lower delivers the resolution as it
    /// returns rather than testing for one.
    #[allow(dead_code)]
    pub fn resolved(self) -> bool {
        matches!(
            self,
            Self::Returned | Self::CancelledBeforeStarted | Self::CancelledBeforeReturned
        )
    }
}
