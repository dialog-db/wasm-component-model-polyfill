//! What a suspended guest thread is waiting for.

use super::subtask_id::SubtaskId;
use super::waitable_id::WaitableId;
use super::waitable_set_id::WaitableSetId;

/// The readiness condition a thread waits on while it is suspended.
///
/// A blocking built-in splits into two parts, as the reference's
/// `Thread.wait_until` does. The try part runs in the host
/// trampoline: it records the thread's condition on the thread's
/// record and checks it once, and a condition that already holds
/// makes the built-in ready at once. The finish part runs once the
/// thread resumes, and computes what the built-in returns and writes
/// what it writes to guest memory.
///
/// A condition is data, not a closure. It names what in the store it
/// watches, and the task tables evaluate it by reading their own
/// records and nothing else: an evaluation changes nothing, polls no
/// host future, and runs no guest code, which is the property of the
/// reference's `ready_func`. That is what lets the scheduler evaluate
/// every waiting thread's condition between two items.
///
/// A thread that runs, or that is ready and has not resumed, waits
/// on nothing, and its record holds `None`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Readiness {
    /// The thread waits for an event on a waitable set, which is
    /// what `waitable-set.wait` waits for.
    WaitableSet {
        /// The set the thread waits on.
        set: WaitableSetId,
    },
    /// The thread waits for one waitable to hold an event of its
    /// own, which is what a synchronous stream or future copy or
    /// cancel waits for.
    Waitable {
        /// The end the copy or the cancel runs on.
        waitable: WaitableId,
    },
    /// The thread waits for the call a subtask records to resolve,
    /// which is what a synchronous lower waits for: the start
    /// intrinsic of a call into another component, and the call of a
    /// host `async` function. A record that is gone counts as
    /// resolved, because a call that failed takes its record away.
    Subtask {
        /// The subtask of the call the thread waits on.
        subtask: SubtaskId,
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
    /// The thread gave way and waits for its turn to resume, which
    /// is what `thread.yield` records. The condition always holds: a
    /// yield waits for nothing but its turn.
    ///
    /// A callback that returns the yield code records no condition.
    /// Its task gives its instance back, and its callback item waits
    /// in the low-priority queue, then in the resume-after-yield
    /// slot, for its turn. That wait is held by the queued item, not
    /// by a thread.
    Yielded,
}
