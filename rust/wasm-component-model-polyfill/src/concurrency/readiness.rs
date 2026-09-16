//! What a suspended guest thread is waiting for.

use super::waitable_set_id::WaitableSetId;

/// The readiness condition a thread waits on while it is suspended.
///
/// A thread of a synchronous call never waits: it runs on the one
/// real stack from the call that started it to the return that ends
/// it, so its condition is `None` for its whole life.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Readiness {
    /// The thread waits for an event on a waitable set.
    WaitableSet {
        /// The set the thread waits on.
        set: WaitableSetId,
    },
    /// The thread waits at its instance's entry gate for
    /// backpressure to clear or the exclusive thread to be free.
    /// Nothing waits at the gate yet.
    #[allow(dead_code)]
    EntryGate,
    /// The thread gave way and waits for its turn to resume. Nothing
    /// gives way yet.
    #[allow(dead_code)]
    Yielded,
}
