//! What a suspended guest thread is waiting for.

/// The readiness condition a thread waits on while it is suspended.
///
/// A thread of a synchronous call never waits: it runs on the one
/// real stack from the call that started it to the return that ends
/// it, so its condition is `None` for its whole life. The variants
/// are the conditions the concurrency features suspend on; nothing
/// constructs one yet.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Readiness {
    /// The thread waits for an event on the waitable set at this
    /// index in the store.
    WaitableSet {
        /// The waitable set's index in the store.
        index: u32,
    },
    /// The thread waits at its instance's entry gate for
    /// backpressure to clear or the exclusive thread to be free.
    EntryGate,
    /// The thread gave way and waits for its turn to resume.
    Yielded,
}
