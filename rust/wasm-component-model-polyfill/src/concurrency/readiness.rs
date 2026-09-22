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
    ///
    /// No thread is ever recorded with this condition. The gate holds
    /// the item that would start a task's thread, not a thread that
    /// has begun, so a task the gate holds has no thread to suspend
    /// until the gate lets it through.
    #[allow(dead_code)]
    EntryGate,
    /// The thread gave way and waits for its turn to resume.
    ///
    /// No thread is ever recorded with this condition. A yield takes
    /// one of two paths, and neither records it. A `thread.yield`
    /// runs one nested turn on the yielding thread's own stack and
    /// returns, so the thread never leaves the running state to wait
    /// for its turn. A callback that returns the yield code does leave
    /// running: the task gives its instance back, and its callback
    /// item waits in the low-priority queue, then in the
    /// resume-after-yield slot, for its turn. That wait is held by the
    /// queued item, not by a thread, so the task's thread keeps no
    /// condition while the item waits.
    #[allow(dead_code)]
    Yielded,
}
