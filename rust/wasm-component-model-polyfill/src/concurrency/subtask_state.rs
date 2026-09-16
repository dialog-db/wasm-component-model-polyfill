//! The state of a subtask record.

/// The state of a subtask: one call out through an import.
///
/// The last three states are the resolved ones: the call produced a
/// result or was cancelled, and its lenders can be released. The
/// numbers are the reference's, because a guest reads them: they are
/// the payload a subtask event carries beside the subtask's handle
/// index, and the low four bits of the status word an asynchronous
/// lower returns.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum SubtaskState {
    /// The call was made and its parameters have not been lifted
    /// yet.
    Starting = 0,
    /// The parameters were lifted and the callee is running.
    Started = 1,
    /// The callee returned its result.
    Returned = 2,
    /// The call was cancelled before the callee read its parameters:
    /// the cancellation built-ins reach this state, and so does a
    /// call abandoned while lifting its parameters.
    CancelledBeforeStarted = 3,
    /// The call was cancelled after the callee read its parameters
    /// and before it returned: the cancellation built-ins reach this
    /// state, and so does a call abandoned once the host side has
    /// begun.
    CancelledBeforeReturned = 4,
}

impl SubtaskState {
    /// Whether the call has produced its outcome, by returning or by
    /// being cancelled. A resolved subtask can deliver its
    /// resolution, which releases the handles it borrowed.
    pub fn resolved(self) -> bool {
        matches!(
            self,
            Self::Returned | Self::CancelledBeforeStarted | Self::CancelledBeforeReturned
        )
    }

    /// The number the guest sees for this state.
    pub fn value(self) -> u32 {
        self as u32
    }
}
