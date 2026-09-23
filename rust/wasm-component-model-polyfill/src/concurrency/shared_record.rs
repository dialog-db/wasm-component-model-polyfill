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
/// and both ends with it. The first drop tells the end that is left:
/// a copy of it that is pending completes with the dropped result,
/// an idle end is given the dropped result with nothing moved, and a
/// stream end whose completed copy has not been delivered has its
/// event turned into the dropped result with the same progress. A
/// later copy on the end that is left sees the mark and completes
/// with the dropped result at once.
pub struct SharedRecord {
    /// The type of each value the stream or future carries, or `None`
    /// for one that carries no values. A built-in that names an end
    /// was declared with a type, and its payload must equal this one.
    pub payload: Option<ValueType>,
    /// Whether either end has been dropped.
    pub dropped: bool,
    /// The end whose copy is waiting for the other end to start one,
    /// if any. The reference calls its buffer `pending_buffer`. The
    /// end stays pending, and its buffer keeps taking values from the
    /// other end's copies, until the event that reports its copy is
    /// delivered.
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

    /// The end the values move through in `direction`.
    pub fn end_of(&self, direction: EndDirection) -> EndId {
        match direction {
            EndDirection::Readable => self.readable,
            EndDirection::Writable => self.writable,
        }
    }
}
