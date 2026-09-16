//! The event a subtask has pending for the thread that waits on it.

use super::subtask_state::SubtaskState;

/// The event a subtask has pending for the thread that waits on it.
///
/// A subtask is a waitable: when it resolves, the scheduler records
/// readiness by filling this slot on the record, and empties the slot
/// when a thread takes delivery. The payloads of a subtask event are
/// the subtask's index in the caller's handle table and the state it
/// moved to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SubtaskEvent {
    /// The subtask's index in the caller instance's handle table.
    pub handle_index: u32,
    /// The state the subtask moved to.
    pub state: SubtaskState,
}
