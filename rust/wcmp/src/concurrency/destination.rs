//! The buffer of a read a host producer serves.

use crate::internal::DestinationInternal;

/// The buffer of one read a host
/// [`StreamProducer`](super::StreamProducer) serves.
///
/// The name is Wasmtime's. The view is the polyfill's own, over a
/// `Vec<T>`: Wasmtime's destination also offers a direct view into
/// the guest's memory, and the runtime layer the polyfill stands on
/// offers only a copy into a memory and a copy out of one, so the
/// items a producer delivers are always held on the host until the
/// poll returns. This type is where a direct view lands if the layer
/// ever allows one.
///
/// A producer delivers items by storing a vector of them with
/// [`set_buffer`](Self::set_buffer). They reach the reader after the
/// poll returns. When the vector holds more items than the reader
/// can take, the rest stay with the end and satisfy the reader's
/// later reads, and the producer is not polled again until the
/// reader has taken all of them.
pub struct Destination<'a, T> {
    buffer: &'a mut Vec<T>,
    remaining: Option<usize>,
}

impl<T> Destination<'_, T> {
    /// How many items the read can still take: `Some` with the count
    /// when the reader is a guest, and `None` when the reader is the
    /// host.
    ///
    /// The count can be zero. A guest that reads zero items is asking
    /// whether the stream is ready to be read, which the Concurrency
    /// explainer calls stream readiness. The producer can answer
    /// [`StreamResult::Completed`](super::StreamResult::Completed) at
    /// once, or wait until it has items and answer then. The guest
    /// handles either.
    pub fn remaining(&self) -> Option<usize> {
        self.remaining
    }

    /// Store `buffer` as the items this poll delivers, in place of any
    /// the poll stored before.
    pub fn set_buffer(&mut self, buffer: Vec<T>) {
        *self.buffer = buffer;
    }

    /// Take back the vector the destination holds, leaving an empty
    /// one in its place. A producer takes it to reuse its allocation:
    /// the end hands the vector of an earlier poll back once the
    /// reader has taken every item in it.
    pub fn take_buffer(&mut self) -> Vec<T> {
        core::mem::take(self.buffer)
    }

    /// Borrow the destination again, for a producer that hands it to
    /// another one it wraps.
    pub fn reborrow(&mut self) -> Destination<'_, T> {
        Destination {
            buffer: &mut *self.buffer,
            remaining: self.remaining,
        }
    }
}

impl<'a, T> DestinationInternal<'a, T> for Destination<'a, T> {
    fn new(buffer: &'a mut Vec<T>, remaining: Option<usize>) -> Self {
        Self { buffer, remaining }
    }
}
