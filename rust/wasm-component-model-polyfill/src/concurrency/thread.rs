//! One guest execution.

use super::readiness::Readiness;
use super::task_id::TaskId;

/// The record of one guest execution.
///
/// Every task has at least one thread, its implicit thread. The
/// thread carries the two context slots `context.get` and
/// `context.set` read and write: the slots moved here from one pair
/// per instantiation, because an adapter that saves them around a
/// callee must see the callee's own pair rather than one it shares
/// with the caller.
pub struct Thread {
    /// The task that contains this thread.
    pub task: TaskId,
    /// The readiness condition the thread waits on, or `None` when
    /// it is running or ready. A thread of a synchronous call never
    /// waits, so nothing sets this yet.
    #[allow(dead_code)]
    pub readiness: Option<Readiness>,
    /// The two context slots, which `context.get` reads and
    /// `context.set` writes.
    pub context: [i32; 2],
    /// The may-not-suspend flag of the callee instance as it was
    /// before this thread's task entered it, saved by the enter
    /// intrinsic and restored by the exit intrinsic. `None` when the
    /// task did not set the flag, which is the case for the task of
    /// an `async` callee. Wasmtime saves the same value in the same
    /// place.
    pub old_may_not_suspend: Option<bool>,
}

impl Thread {
    /// Construct a thread of `task`, running, with both context
    /// slots zero.
    pub fn new(task: TaskId) -> Self {
        Self {
            task,
            readiness: None,
            context: [0; 2],
            old_may_not_suspend: None,
        }
    }
}
