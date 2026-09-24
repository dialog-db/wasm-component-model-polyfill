//! One end of a stream or a future.

use super::copy_buffer::CopyBuffer;
use super::copy_state::CopyState;
use super::end_direction::EndDirection;
use super::waitable_state::WaitableState;

/// The record of one end of a stream or a future. The reference
/// names it `CopyEnd`.
///
/// A handle-table entry for an end holds the identity of one of
/// these, as a subtask entry holds the identity of a subtask record.
/// The two ends of one stream or future share one
/// [`SharedRecord`](super::shared_record::SharedRecord), which this
/// record names by its index in the store's table of shared records.
///
/// An end is a waitable, so the record carries the waitable state a
/// guest waits on: the pending event slot, the set the end joined,
/// and the synchronous-waiter flag. The event a finished copy fills
/// the slot with is the stream or future read or write event.
pub struct CopyEnd {
    /// The waitable state: what a thread waiting on this end
    /// consults.
    pub waitable: WaitableState,
    /// How far the end is through its copies.
    pub state: CopyState,
    /// Which way the values move through the end.
    pub direction: EndDirection,
    /// The index of the shared record in the store's table of shared
    /// records. The shared record lives until both of its ends are
    /// dropped, so the index names it for as long as this record
    /// lives.
    pub shared: u32,
    /// The buffer of the copy in progress, from the moment a read or
    /// a write starts until the event that reports it is delivered.
    pub buffer: Option<CopyBuffer>,
    /// The index of the entry that names the end in the handle table
    /// of the instance that holds it, which the event the end
    /// delivers carries. A readable end that crosses a boundary takes
    /// a new index in the receiver's table, and the entry that
    /// receives it records the new one here.
    pub handle: Option<u32>,
    /// Whether the end has delivered a dropped result: it was told
    /// that the other end dropped, and so is done. Wasmtime keeps the
    /// same flag on the end's handle-table entry. A later copy on a
    /// future end traps with a message that says which way the end
    /// became done, and only this flag tells a writable future end
    /// that wrote its value from one whose reader dropped first.
    pub notified_dropped: bool,
}

impl CopyEnd {
    /// Construct an idle end with the given direction, sharing the
    /// record at `shared`: no pending event, no set, no waiter, and
    /// no copy in progress, with no dropped result delivered.
    pub fn new(direction: EndDirection, shared: u32) -> Self {
        Self {
            waitable: WaitableState::new(),
            state: CopyState::Idle,
            direction,
            shared,
            buffer: None,
            handle: None,
            notified_dropped: false,
        }
    }
}
