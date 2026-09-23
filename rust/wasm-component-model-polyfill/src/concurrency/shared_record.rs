//! The state the two ends of one stream or future share.

use crate::types::ValueType;

use super::end_direction::EndDirection;
use super::end_id::EndId;

/// The state the two ends of one stream or future share.
///
/// Each stream or future is one of these in the store, and each of
/// its two ends is a [`CopyEnd`](super::copy_end::CopyEnd) that names
/// it. The reference keeps the same state on its `SharedStreamImpl`
/// and `SharedFutureImpl`.
///
/// Dropping one end marks the record dropped and leaves both end
/// records in the store; dropping the other end removes the record
/// and both ends with it. A copy on the end that is left sees the
/// mark and completes with the dropped result.
pub struct SharedRecord {
    /// The type of each value the stream or future carries, or `None`
    /// for one that carries no values. A built-in that names an end
    /// was declared with a type, and its payload must equal this one.
    pub payload: Option<ValueType>,
    /// Whether either end has been dropped.
    pub dropped: bool,
    /// The end whose copy is waiting for the other end to start one,
    /// if any. Nothing starts a copy yet, so nothing sets this.
    #[allow(dead_code)]
    pub pending: Option<EndDirection>,
    /// The readable end.
    pub readable: EndId,
    /// The writable end.
    pub writable: EndId,
}

impl SharedRecord {
    /// Construct the shared record of a fresh stream or future whose
    /// two ends are `readable` and `writable`: nothing dropped and
    /// nothing pending.
    pub fn new(payload: Option<ValueType>, readable: EndId, writable: EndId) -> Self {
        Self {
            payload,
            dropped: false,
            pending: None,
            readable,
            writable,
        }
    }
}
