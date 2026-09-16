//! One call out through an import.

use crate::resource::TableId;

use super::subtask_event::SubtaskEvent;
use super::subtask_state::SubtaskState;

/// The record of one call out through an import.
///
/// A subtask is the scope a borrow lifted out of the caller's owning
/// handle is lent to: the lift raises the lend count on the owning
/// entry and records the entry here, and delivering the subtask's
/// resolution lowers those counts again. A resolution is delivered
/// when the caller's thread receives the subtask event, or when a
/// synchronous lower returns.
pub struct Subtask {
    /// How far the call has got.
    pub state: SubtaskState,
    /// The owning handle-table entries the caller lent for the call,
    /// as `(table, index)`. The reference names this list `lenders`.
    /// Emptied when the resolution is delivered.
    pub lenders: Vec<(TableId, u32)>,
    /// The event the subtask has pending for the thread waiting on
    /// it, which the reference fills when the scheduler records
    /// readiness and empties on delivery. Nothing fills it yet.
    #[allow(dead_code)]
    pub pending_event: Option<SubtaskEvent>,
    /// The waitable set the subtask joined, by its index in the
    /// store, or `None` when it has joined none. Nothing joins a
    /// waitable set yet.
    #[allow(dead_code)]
    pub waitable_set: Option<u32>,
    /// Whether a thread is waiting on this subtask synchronously.
    /// Nothing sets it yet: a synchronous call keeps its caller on
    /// the one real stack rather than recording a waiter.
    #[allow(dead_code)]
    pub synchronous_waiter: bool,
    /// Whether the caller asked for the call to be cancelled.
    /// Nothing requests cancellation yet.
    #[allow(dead_code)]
    pub cancel_requested: bool,
}

impl Subtask {
    /// Construct a subtask in its starting state: the call was made
    /// and its parameters have not been lifted yet.
    pub fn new() -> Self {
        Self {
            state: SubtaskState::Starting,
            lenders: Vec::new(),
            pending_event: None,
            waitable_set: None,
            synchronous_waiter: false,
            cancel_requested: false,
        }
    }
}

impl Default for Subtask {
    fn default() -> Self {
        Self::new()
    }
}
