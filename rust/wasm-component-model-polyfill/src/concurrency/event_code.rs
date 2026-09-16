//! The code of one event.

/// The code of one event: what happened, and therefore what the two
/// payloads beside it mean.
///
/// The codes and their numbers are the reference's `EventCode`. A
/// guest reads the number: the built-ins that deliver an event write
/// it as the result of a wait or a poll, and the callback protocol
/// passes it back in the same encoding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum EventCode {
    /// Nothing was ready. A poll of a set that holds no event
    /// answers with this code and two zero payloads.
    None = 0,
    /// A subtask made progress. The payloads are the subtask's index
    /// in the caller instance's handle table and the state it moved
    /// to.
    Subtask = 1,
    /// A read on a stream end finished a copy. The payloads are the
    /// end's index in the handle table and the copy result.
    StreamRead = 2,
    /// A write on a stream end finished a copy, under the same rule
    /// as [`EventCode::StreamRead`].
    StreamWrite = 3,
    /// A read on a future end finished a copy, under the same rule as
    /// [`EventCode::StreamRead`].
    FutureRead = 4,
    /// A write on a future end finished a copy, under the same rule
    /// as [`EventCode::StreamRead`].
    FutureWrite = 5,
    /// The task the waiting thread belongs to was cancelled. Both
    /// payloads are zero.
    TaskCancelled = 6,
}

impl EventCode {
    /// The number the guest sees for this code.
    pub fn value(self) -> u32 {
        self as u32
    }
}
